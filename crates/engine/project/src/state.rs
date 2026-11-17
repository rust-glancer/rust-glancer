//! Metadata published together with one accepted saved-source revision.

use std::sync::atomic::{AtomicU64, Ordering};

use rg_def_map::MacroExpansionLimitReport;
use rg_ir_model::CrateRef;
use rg_std::MemorySize;
use rg_workspace::WorkspaceMetadata;

use crate::{MacroExpansionLimitBuildSummary, ProjectStats, source_index::ProjectSourceIndex};

/// Identifies one saved version of a [`Project`](crate::Project).
///
/// Only publishing a source or workspace change gives the project a new id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, MemorySize)]
#[memsize(leaf)]
pub struct ProjectGenerationId(u64);

impl ProjectGenerationId {
    pub(crate) fn fresh() -> Self {
        static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);
        Self(NEXT_GENERATION.fetch_add(1, Ordering::Relaxed))
    }

    pub fn get(self) -> u64 {
        self.0
    }
}

/// Routing stays available between operations. Ajika owns all declaration and body results.
#[derive(Debug, MemorySize)]
pub(crate) struct ProjectState {
    pub(crate) generation_id: ProjectGenerationId,
    pub(crate) workspace: WorkspaceMetadata,
    pub(crate) sources: ProjectSourceIndex,
    pub(crate) targets: Vec<CrateRef>,
    pub(crate) reports: Vec<MacroExpansionLimitReport>,
    pub(crate) macro_expansion_limit_summary: MacroExpansionLimitBuildSummary,
}

impl ProjectState {
    pub(crate) fn workspace(&self) -> &WorkspaceMetadata {
        &self.workspace
    }

    pub(crate) fn stats(&self) -> ProjectStats {
        ProjectStats {
            package_count: self.workspace.packages().len(),
            workspace_package_count: self.workspace.workspace_packages().count(),
            target_count: self.targets.len(),
            file_count: self.sources.file_count(),
        }
    }
}
