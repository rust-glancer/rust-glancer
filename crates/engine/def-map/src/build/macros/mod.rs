//! Expands item-shaped declarative macros and builtin derives during def-map construction.
//!
//! Declarative expansion is tied to import resolution: a call may need imports to find its definition,
//! and its generated items may add new imports or new macros. This module prepares queued calls,
//! executes expansion jobs, and collects the generated syntax according to the call's placement.
//! Module-position output enters the caller's module; associated output
//! replaces the invocation inside its trait or impl, and nested calls preserve that owner slot.
//!
//! For example, `make_types!();` at module scope contributes ordinary module items, while
//! `impl User { make_methods!(); }` contributes methods or associated consts to that `impl User`.
//! If `make_methods!` expands to another macro call, that nested call stays in the same impl slot.
//!
//! Builtin derives run after imports and declarative expansion have settled. They add only impl
//! headers, so the settled module scopes stay valid and need no further import-resolution passes.
//!
//! Most generated items can be spliced immediately. A generated `mod child;` still needs its child
//! file, while a generated `include!(...)` needs a real file whose items retain the include call's
//! placement. For those two source edges, the collector keeps the corresponding semantic
//! pending operation and emits a source request. The injected loader captures and lowers the file
//! before construction continues the same macro queue.

use std::sync::Arc;

use rg_ir_model::{CrateRef, FileId, ModuleId, ModuleRef, Span};
use rg_item_tree::{BuiltinMacroItem, ItemTreeRef, MacroUseSelector};
use rg_parse::ModuleFileContext;
use rg_text::Name;
use rg_tt::TopSubtree;

mod attempts;
mod builtin_derive;
mod expand;
mod generated;
mod generated_tree;
mod limits;
mod resolve;
mod textual_scope;

pub(super) use self::{
    attempts::{
        MacroExpansionAttempt, MacroExpansionScan, apply_expansion_attempts,
        collect_expansion_attempts,
    },
    builtin_derive::BuiltinDeriveExpansion,
    expand::expand_expansion_attempts,
    generated::{
        GeneratedOrigin, PendingGeneratedInclude, PendingGeneratedModule,
        apply_pending_macro_source_files,
    },
    limits::{MAX_MACRO_EXPANSION_DEPTH, PendingMacroExpansionLimitReport},
    textual_scope::TextualMacroScopes,
};

#[derive(Debug, Clone)]
pub(super) struct MacroDirective {
    pub(super) call: MacroCallSite,
    pub(super) origin: MacroCallOrigin,
    pub(super) state: MacroDirectiveState,
}

impl MacroDirective {
    /// Count only this invocation's ancestors. Retries and work from sibling invocations do not
    /// consume its recursion allowance, and walking the existing links needs no depth table.
    fn reaches_recursion_limit(&self, directives: &[Self]) -> bool {
        let mut current = self;
        for _ in 0..MAX_MACRO_EXPANSION_DEPTH {
            let MacroCallOrigin::Generated { parent_call } = current.origin else {
                return false;
            };
            current = &directives[parent_call];
        }
        true
    }
}

/// Says whether a queued macro call came from source or from another queued call.
///
/// Generated calls point back into the same append-only directive list. That one index is enough
/// to reconstruct a diagnostic chain without retaining a second expansion graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MacroCallOrigin {
    Source,
    Generated { parent_call: usize },
}

/// Says where a successful item-shaped expansion is spliced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MacroCallPlacement {
    /// Ordinary module-position output, such as `make_types!();`, enters the caller's module.
    ModuleItems,
    /// Output from `impl User { make_methods!(); }` replaces the call inside that trait or impl.
    AssociatedItems { call_source: crate::ItemSource },
}

/// Worklist state for an item-shaped macro call seen during def-map construction.
///
/// Scheduling only needs to know when a call can be attempted again. Profiling records expansion
/// results separately; successful, failed, and skipped calls all finish their work in this queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MacroDirectiveState {
    /// The call has not yet been attempted.
    Pending,
    /// Lookup failed; retry only after imports have been refreshed.
    WaitingForImports,
    /// This invocation's ancestry reached the recursion limit. Keep the reason until the bounded
    /// diagnostic is assembled; unrelated pending calls may still resolve and expand.
    RecursionLimitReached,
    /// No further attempt is needed, including when source loading owns the remaining collection.
    Complete,
}

#[derive(Debug, Clone)]
pub(super) struct MacroDefinitionRecord {
    pub(super) order: ItemOrder,
}

/// Legacy `#[macro_use] extern crate ...` request used by unqualified macro fallback lookup.
///
/// We intentionally model this as a small compatibility bridge: exported root macros from the
/// source crate are consulted only after textual and ordinary module-scope lookup fail. That covers
/// the common legacy dependency pattern without turning the current macro engine into a full Rust
/// 2015 macro prelude model.
pub(super) struct MacroUseImport {
    pub(super) module: ModuleId,
    pub(super) source_module: ModuleRef,
    pub(super) selector: MacroUseSelector,
    pub(super) order: ItemOrder,
}

/// One queued item-shaped macro call plus the call-site state needed after ItemTree lowering.
///
/// `module` supplies the scope used to resolve the call and any nested macros, while `placement`
/// says whether successful output enters that module or replaces an associated-item call. The
/// `module_file_context` also follows the caller: a macro imported from another crate must still
/// resolve a generated `mod child;` next to the invocation, not next to its definition.
#[derive(Debug, Clone)]
pub(super) struct MacroCallSite {
    pub(super) module: ModuleId,
    pub(super) source: ItemTreeRef,
    pub(super) path: Option<String>,
    pub(super) callee: Option<Name>,
    pub(super) args: Option<TopSubtree>,
    pub(super) builtin: Option<BuiltinMacroItem>,
    pub(super) dollar_crate: Option<CrateRef>,
    pub(super) file_id: FileId,
    pub(super) span: Span,
    pub(super) order: ItemOrder,
    /// Destination retained by nested calls and pending source collection.
    pub(super) placement: MacroCallPlacement,
    /// Logical filesystem base inherited by declarations emitted from this call.
    pub(super) module_file_context: Arc<ModuleFileContext>,
}

impl MacroCallSite {
    fn identity(&self) -> &str {
        self.path
            .as_deref()
            .or_else(|| self.callee.as_ref().map(Name::as_str))
            .unwrap_or("<unknown>")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct ItemOrder(Vec<usize>);

impl ItemOrder {
    pub(super) fn real(index: usize) -> Self {
        Self(vec![index])
    }

    pub(super) fn generated_child(&self, index: usize) -> Self {
        let mut order = self.0.clone();
        order.push(index);
        Self(order)
    }
}
