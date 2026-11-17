//! Read-only access to one published project generation.

use rg_analysis::{Analysis, ReferenceSearchFile, ReferenceSearchLabel, SavedSourceView};
use rg_ir_model::{CrateRef, PackageSlot};
use rg_ir_view::IndexedViewDb;
use rg_semantic_ir::TargetDeclarationReader;
use rg_source::QueryContext;

use super::reference_search::ReferenceSearchPlanner;
use crate::state::ProjectState;

/// Immutable project view used to answer LSP-shaped queries.
#[derive(Debug, Clone, Copy)]
pub struct ProjectSnapshot<'a> {
    pub(crate) state: &'a ProjectState,
    db: &'a QueryContext<'a>,
    declarations: &'a TargetDeclarationReader<'a>,
}

impl<'a> ProjectSnapshot<'a> {
    pub(crate) fn new(
        state: &'a ProjectState,
        db: &'a QueryContext<'a>,
        declarations: &'a TargetDeclarationReader<'a>,
    ) -> Self {
        Self {
            state,
            db,
            declarations,
        }
    }

    /// Build a facade over the active operation. Individual feature reads acquire their
    /// declarations and bodies as they need them.
    pub fn analysis(&self) -> Analysis<'a> {
        Analysis::new(self.view_db(), SavedSourceView::new(self.state.parse_db()))
    }

    pub(crate) fn view_db(&self) -> IndexedViewDb<'a> {
        IndexedViewDb::new(
            self.db,
            rg_def_map::CrateDefMapReader(self.declarations),
            rg_semantic_ir::ItemReader(self.declarations),
            rg_body_ir::BodyReader::new(self.db, self.declarations),
            self.state.parse_db(),
            self.db.services().cancellation.clone(),
        )
    }

    #[cfg(test)]
    pub(crate) fn query_context(&self) -> &'a QueryContext<'a> {
        self.db
    }

    #[cfg(test)]
    pub(crate) fn declarations(&self) -> &'a TargetDeclarationReader<'a> {
        self.declarations
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
}
