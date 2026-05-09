//! LSP-side project shape reporting.
//!
//! These counters describe the retained analysis graph, not allocator behavior. Keeping them out
//! of memory reporting makes save/index logs easier to read and keeps each subsystem honest.

use rg_project::ProjectSnapshot;

use crate::memory::format_bytes;

/// Coarse counters for the analysis snapshot currently served by the LSP.
#[derive(Debug, Clone, Copy, derive_more::Display)]
#[display("{:?}", self)]
pub(crate) struct ProjectStats {
    package_count: usize,
    workspace_package_count: usize,
    def_map_targets: usize,
    def_map_modules: usize,
    unresolved_imports: usize,
    semantic_targets: usize,
    semantic_type_defs: usize,
    semantic_traits: usize,
    semantic_impls: usize,
    semantic_functions: usize,
    body_targets: usize,
    body_built_targets: usize,
    body_skipped_targets: usize,
    body_count: usize,
    expression_count: usize,
}

impl ProjectStats {
    pub(crate) fn capture(snapshot: ProjectSnapshot<'_>) -> Self {
        let stats = snapshot.stats();

        Self {
            package_count: stats.package_count,
            workspace_package_count: stats.workspace_package_count,
            def_map_targets: stats.def_map.target_count,
            def_map_modules: stats.def_map.module_count,
            unresolved_imports: stats.def_map.unresolved_import_count,
            semantic_targets: stats.semantic_ir.target_count,
            semantic_type_defs: stats.semantic_ir.struct_count
                + stats.semantic_ir.enum_count
                + stats.semantic_ir.union_count,
            semantic_traits: stats.semantic_ir.trait_count,
            semantic_impls: stats.semantic_ir.impl_count,
            semantic_functions: stats.semantic_ir.function_count,
            body_targets: stats.body_ir.target_count,
            body_built_targets: stats.body_ir.built_target_count,
            body_skipped_targets: stats.body_ir.skipped_target_count,
            body_count: stats.body_ir.body_count,
            expression_count: stats.body_ir.expression_count,
        }
    }

    pub(crate) fn log_info(self, label: &'static str) {
        tracing::info!(
            label,
            stats = %self,
            "project stats"
        );
    }
}

pub(crate) fn log_retained_memory(snapshot: ProjectSnapshot<'_>, label: &'static str) {
    if !tracing::enabled!(target: "rg_lsp_engine::memory", tracing::Level::DEBUG) {
        return;
    }

    // Retained-memory accounting walks the full analysis graph. Keep it opt-in so normal editor
    // logs get cheap counters without slowing every save.
    let retained_bytes = snapshot.retained_memory_bytes();
    tracing::debug!(
        target: "rg_lsp_engine::memory",
        label,
        retained_bytes,
        retained = %format_bytes(retained_bytes),
        "analysis retained memory"
    );
}
