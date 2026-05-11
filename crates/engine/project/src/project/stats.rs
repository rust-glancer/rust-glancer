//! Coarse project counters for status output and observability.
//!
//! These stats intentionally summarize the currently retained project state without exposing the
//! raw phase databases. Some phase counters are residency-sensitive because offloaded payloads are
//! intentionally absent from memory.

use rg_body_ir::BodyIrStats;
use rg_def_map::DefMapStats;
use rg_semantic_ir::SemanticIrStats;

use super::state::ProjectState;

/// Coarse counters for one built project snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectStats {
    pub package_count: usize,
    pub workspace_package_count: usize,
    pub def_map: DefMapStats,
    pub semantic_ir: SemanticIrStats,
    pub body_ir: BodyIrStats,
}

impl ProjectStats {
    pub(crate) fn capture(project: &ProjectState) -> Self {
        Self {
            package_count: project.parse.package_count(),
            workspace_package_count: project.parse.workspace_packages().count(),
            def_map: project.def_map.stats(),
            semantic_ir: project.semantic_ir.stats(),
            body_ir: project.body_ir.stats(),
        }
    }
}
