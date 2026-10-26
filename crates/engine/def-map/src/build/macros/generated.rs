//! Prepares expansion syntax and completes source-file requests for declaration collection.
//!
//! Parsed expansion payloads enter the shared collector with generated source identities. Loaded
//! includes and child modules enter that same collector with real ItemTree identities. This module
//! retains only the pending operation needed between requesting a real file and collecting it.

use std::sync::Arc;

use anyhow::{Context as _, Result};
use rg_ir_model::{CrateRef, FileId, ModuleId, Span};
use rg_item_tree::{ItemTreeDb, ItemTreeRef};
use rg_macro_runtime::ExpansionSyntax;
use rg_parse::ModuleFileContext;
use rg_text::NameInterner;

use super::{
    ItemOrder, MacroCallOrigin, MacroCallPlacement, MacroExpansionApplyResult,
    generated_tree::GeneratedSourceLowering,
};
use crate::{
    ItemSource, MacroSourceFileRequest,
    build::{
        MacroSourceFileResolution, MacroSourceFileResolutions,
        collect::{CrateState, DeclarationCollector},
        finalize::FinalizeCrateStates,
    },
    profile::metric,
};

/// Call-site identity used for every item produced by one macro expansion.
///
/// Placement and module-file context also stay call-site-relative. Placement keeps associated
/// output in its trait or impl, while the file context makes an out-of-line child module belong
/// beside the invocation rather than the macro definition.
#[derive(Debug, Clone)]
pub(crate) struct GeneratedOrigin {
    pub(crate) module: ModuleId,
    pub(crate) source: ItemTreeRef,
    pub(crate) file_id: FileId,
    pub(crate) span: Span,
    pub(crate) order: ItemOrder,
    pub(crate) placement: MacroCallPlacement,
    pub(crate) dollar_crate: Option<CrateRef>,
    pub(crate) parent_call: usize,
    pub(crate) module_file_context: Arc<ModuleFileContext>,
}

impl GeneratedOrigin {
    /// Lower one parsed expansion into transient item data, then collect it like written items.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn collect_syntax(
        self,
        state: &mut CrateState,
        interner: &mut NameInterner,
        item_tree: &ItemTreeDb,
        resolutions: &MacroSourceFileResolutions,
        expansion: ExpansionSyntax,
        macro_name: Option<&str>,
    ) -> Result<MacroExpansionApplyResult> {
        let timer = metric::TIMING_PARSE_GENERATED_SOURCES.start_timer();
        let errors = expansion.parse.errors();
        timer.finish();
        if !errors.is_empty() {
            let name = macro_name.unwrap_or("<unknown>");
            metric::MACRO_CALLS_FAILED.inc();
            metric::GENERATED_SOURCE_PARSE_FAILURES.inc();
            metric::FAILED_PARSE_BY_NAME.inc(name);
            anyhow::bail!("macro expansion syntax has errors: {errors:?}");
        }
        metric::GENERATED_SOURCES_PARSED.inc();
        let source = GeneratedSourceLowering::lower(
            state.crate_ref.package,
            &self,
            expansion,
            interner,
            state.edition,
        )
        .context("lower macro expansion declarations")?;
        let source = state.def_map_builder.alloc_generated_source(source);
        let package = item_tree
            .package(state.crate_ref.package.0)
            .context("generated source package should exist")?;
        let timer = metric::TIMING_COLLECT_GENERATED_ITEMS.start_timer();
        let result = DeclarationCollector::new(
            state,
            package,
            resolutions,
            MacroCallOrigin::Generated {
                parent_call: self.parent_call,
            },
            self.dollar_crate,
        )
        .collect_expansion(self, source);
        timer.finish();
        result
    }
}

/// Pending collection for one generated out-of-line module whose real file is project-owned.
///
/// The `mod` declaration is already retained in DefMap's synthetic macro-output arena. Completing
/// collection therefore needs only that item id and the context that would have been used if the
/// real file resolution had been available immediately.
#[derive(Debug, Clone)]
pub(crate) struct PendingGeneratedModule {
    pub(crate) request: MacroSourceFileRequest,
    pub(crate) parent_module: ModuleId,
    pub(crate) source: ItemSource,
    pub(crate) order: ItemOrder,
    pub(crate) module_file_context: Arc<ModuleFileContext>,
    pub(crate) origin: MacroCallOrigin,
}

/// Pending collection for one builtin `include!` call produced by another macro.
///
/// ItemTree could not lower a payload for a call that did not exist in original source. The macro
/// directive and its call placement are already retained, so collection only needs to splice the
/// project-lowered file into the module or associated owner and mark the directive complete.
#[derive(Debug, Clone)]
pub(crate) struct PendingGeneratedInclude {
    pub(super) request: MacroSourceFileRequest,
    pub(super) call_id: usize,
    pub(super) origin: GeneratedOrigin,
}

/// Collect each answered module or include without replaying its surrounding expansion.
pub(crate) fn apply_pending_macro_source_files(
    item_tree: &ItemTreeDb,
    states: &mut FinalizeCrateStates,
    resolutions: &MacroSourceFileResolutions,
) -> Result<()> {
    for (package_slot, states) in states.iter_dirty_mut_enumerated() {
        let package = item_tree
            .package(package_slot)
            .context("pending source package should exist")?;
        for state in states {
            let pending = std::mem::take(&mut state.pending_generated_modules);
            for pending in pending {
                match resolutions
                    .get(&pending.request)
                    .expect("pending module request should be answered")
                {
                    MacroSourceFileResolution::Missing => {}
                    MacroSourceFileResolution::Module { .. } => {
                        // File contents use their own crate for `$crate`, even when the module
                        // declaration came from a macro expansion.
                        DeclarationCollector::new(
                            state,
                            package,
                            resolutions,
                            pending.origin,
                            None,
                        )
                        .collect_pending_module(pending)?;
                    }
                    MacroSourceFileResolution::Include { .. } => {
                        unreachable!("module request received an include answer")
                    }
                }
            }
            let pending = std::mem::take(&mut state.pending_generated_includes);
            for pending in pending {
                let success = match resolutions
                    .get(&pending.request)
                    .expect("pending include request should be answered")
                {
                    MacroSourceFileResolution::Missing => false,
                    MacroSourceFileResolution::Include { file_id } => DeclarationCollector::new(
                        state,
                        package,
                        resolutions,
                        MacroCallOrigin::Generated {
                            parent_call: pending.call_id,
                        },
                        None,
                    )
                    .collect_file(
                        pending.origin.module,
                        *file_id,
                        pending.origin.order,
                        pending.origin.placement,
                        pending.origin.module_file_context,
                    )
                    .is_ok(),
                    MacroSourceFileResolution::Module { .. } => {
                        unreachable!("include request received a module answer")
                    }
                };
                if let Some(directive) = state.macro_directives.get_mut(pending.call_id) {
                    directive.state = if success {
                        super::MacroDirectiveState::Expanded
                    } else {
                        super::MacroDirectiveState::Failed
                    };
                }
            }
        }
    }
    Ok(())
}
