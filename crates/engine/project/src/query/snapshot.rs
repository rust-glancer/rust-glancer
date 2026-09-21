//! Read-only access to one published project generation.

use rg_analysis::{Analysis, ReferenceSearchFile, ReferenceSearchLabel};
use rg_ir_model::{CrateRef, PackageSlot};
use rg_std::MemorySize;

use super::reference_search::ReferenceSearchPlanner;
use crate::{
    MacroExpansionLimitBuildSummary, ProjectStats, selection::subset, state::ProjectState,
};

/// Immutable project view used to answer LSP-shaped queries.
#[derive(Debug, Clone, Copy)]
pub struct ProjectSnapshot<'a> {
    pub(crate) state: &'a ProjectState,
}

impl<'a> ProjectSnapshot<'a> {
    /// Returns a full-project analysis view.
    pub fn full_analysis(
        &self,
        cancellation: rg_std::CancellationToken,
    ) -> anyhow::Result<Analysis<'a>> {
        let txn = self.state.read_txn(cancellation)?;
        Ok(self.state.analysis(&txn))
    }

    /// Returns an analysis view scoped to the package dependency closure of crate queries.
    pub fn analysis_for_crates(
        &self,
        crates: &[CrateRef],
        cancellation: rg_std::CancellationToken,
    ) -> anyhow::Result<Analysis<'a>> {
        let subset = subset::crates_with_visible_dependencies(self.state.workspace(), crates);
        let txn = self
            .state
            .read_txn_for_subset(&subset, cancellation.clone())?;
        Ok(self.state.analysis(&txn))
    }

    /// Returns crates whose source should be scanned for an explicit references query.
    ///
    /// Queries scan the selected declaration packages and their package reverse-dependency
    /// closure. Workspace-origin queries keep that closure focused on workspace members, falling
    /// back to the whole workspace only when the declaration package is graph-opaque.
    pub fn reference_search_crates(
        &self,
        origin_package: PackageSlot,
        declaration_crates: &[CrateRef],
        cancellation: &rg_std::CancellationToken,
    ) -> anyhow::Result<Vec<CrateRef>> {
        ReferenceSearchPlanner::new(self.state).crates(
            origin_package,
            declaration_crates,
            cancellation,
        )
    }

    /// Returns crate/file pairs whose source text contains one of the safe reference labels.
    ///
    /// This is a request-local text prefilter. It narrows expensive semantic scans without storing
    /// a persistent text index or changing the declaration matcher that proves each result.
    pub fn reference_search_files_matching_labels(
        &self,
        search_crates: &[CrateRef],
        labels: &[ReferenceSearchLabel],
        cancellation: &rg_std::CancellationToken,
    ) -> anyhow::Result<Option<Vec<ReferenceSearchFile>>> {
        ReferenceSearchPlanner::new(self.state).files_matching_labels(
            search_crates,
            labels,
            cancellation,
        )
    }

    pub fn stats(&self) -> ProjectStats {
        self.state.stats()
    }

    /// Returns bounded diagnostics from the def-map packages built for this project state.
    pub fn macro_expansion_limit_summary(&self) -> &MacroExpansionLimitBuildSummary {
        &self.state.macro_expansion_limit_summary
    }

    /// Returns an approximate retained-memory total for the current immutable analysis graph.
    ///
    /// This is intended for observability, not correctness. Computing it walks the graph, so LSP
    /// callers should keep it behind explicit memory logging.
    pub fn retained_memory_bytes(&self) -> usize {
        use MemorySize as _;

        self.state.memory_size()
    }
}
