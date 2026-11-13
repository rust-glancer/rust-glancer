//! Prepares expansion syntax and completes source-file requests for declaration collection.
//!
//! Parsed expansion payloads enter the shared collector with generated source identities. Loaded
//! includes and child modules enter that same collector with real ItemTree identities. This module
//! retains only the pending operation needed between requesting a real file and collecting it.

use std::sync::Arc;

use anyhow::{Context as _, Result};
use rg_ir_model::{CrateRef, FileId, ModuleId};
use rg_item_tree::{ItemTreeRef, ItemTrees};
use rg_macro_runtime::ExpansionSyntax;
use rg_parse::ModuleFileContext;

use super::{
    ItemOrder, MacroCallOrigin, MacroCallPlacement, generated_tree::GeneratedSourceLowering,
};
use crate::{
    ItemSource, MacroSourceFileRequest,
    build::{
        MacroSourceFileResolution, MacroSourceFileResolutions, collect::DeclarationCollector,
        crates::CrateBuilder,
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
    pub(crate) call_site: rg_tt::Span,
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
        state: &mut CrateBuilder,
        package: &ItemTrees,
        resolutions: &MacroSourceFileResolutions,
        expansion: ExpansionSyntax,
        macro_name: Option<&str>,
    ) -> Result<bool> {
        let timer = metric::TIMING_PARSE_GENERATED_SOURCES.start_timer();
        // Collection needs parsed declarations. Extra checks on literals and function bodies
        // belong to diagnostics; they should not reject the expansion's declarations.
        let errors = expansion.parse.parse_errors();
        timer.finish();
        if !errors.is_empty() {
            let name = macro_name.unwrap_or("<unknown>");
            metric::MACRO_CALLS_FAILED.inc();
            metric::GENERATED_SOURCE_PARSE_FAILURES.inc();
            metric::FAILED_PARSE_BY_NAME.inc(name);
            anyhow::bail!("macro expansion syntax has errors: {errors:?}");
        }
        metric::GENERATED_SOURCES_PARSED.inc();
        let source = GeneratedSourceLowering::lower(&self, expansion)
            .context("lower macro expansion declarations")?;
        let source = state.def_map_builder.alloc_generated_source(source);
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
/// The `mod` declaration is already retained as an item. Its ID and the call site's module
/// context are enough to finish collection once the file is available.
#[derive(Debug, Clone)]
pub(crate) struct PendingGeneratedModule {
    pub(crate) request: MacroSourceFileRequest,
    pub(crate) parent_module: ModuleId,
    pub(crate) source: ItemSource,
    pub(crate) order: ItemOrder,
    pub(crate) module_file_context: Arc<ModuleFileContext>,
    pub(crate) origin: MacroCallOrigin,
}

/// Pending collection for a builtin `include!` that named a file we have yet to acquire.
///
/// The macro directive is complete. This pending operation owns the remaining file collection.
/// The origin retains ancestry for nested macros and placement for splicing the loaded items into
/// the module or associated owner.
#[derive(Debug, Clone)]
pub(crate) struct PendingInclude {
    pub(super) request: MacroSourceFileRequest,
    pub(super) origin: GeneratedOrigin,
}

/// Collect each answered module or include without replaying its surrounding expansion.
pub(crate) fn apply_pending_macro_source_files(
    package: &ItemTrees,
    state: &mut CrateBuilder,
    resolutions: &MacroSourceFileResolutions,
) -> Result<()> {
    let pending = std::mem::take(&mut state.pending_generated_modules);
    for pending in pending {
        match resolutions
            .get(&pending.request)
            .expect("pending module request should be answered")
        {
            MacroSourceFileResolution::Missing | MacroSourceFileResolution::Module { .. } => {
                // File contents use their own crate for `$crate`, even when the module
                // declaration came from a macro expansion.
                DeclarationCollector::new(state, package, resolutions, pending.origin, None)
                    .collect_pending_module(pending)?;
            }
            MacroSourceFileResolution::Include { .. } => {
                unreachable!("module request received an include answer")
            }
        }
    }
    let pending = std::mem::take(&mut state.pending_includes);
    for pending in pending {
        match resolutions
            .get(&pending.request)
            .expect("pending include request should be answered")
        {
            MacroSourceFileResolution::Missing => {}
            MacroSourceFileResolution::Include { file_id } => {
                // Collection failure finishes this include just as it does an ordinary
                // expansion. Source loading has answered the request, so there is no retry.
                let _ = DeclarationCollector::new(
                    state,
                    package,
                    resolutions,
                    MacroCallOrigin::Generated {
                        parent_call: pending.origin.parent_call,
                    },
                    None,
                )
                .collect_file(
                    pending.origin.module,
                    *file_id,
                    pending.origin.order,
                    pending.origin.placement,
                    pending.origin.module_file_context,
                );
            }
            MacroSourceFileResolution::Module { .. } => {
                unreachable!("include request received a module answer")
            }
        }
    }
    Ok(())
}
