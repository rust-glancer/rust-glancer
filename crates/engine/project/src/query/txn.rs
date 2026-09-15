//! Project-level read transactions.

use rg_analysis::{Analysis, SavedSourceView};
use rg_def_map::DefMapReadTxn;
use rg_ir_view::IndexedViewDb;
use rg_package_store::PackageSubset;

use crate::{selection::subset, state::ProjectState};

/// Read transaction for project-level query APIs.
///
/// The transaction is request-scoped: query callers create it once, build an `Analysis` view from
/// it, and reuse that view for the duration of the request.
#[derive(Debug, Clone)]
pub(crate) struct ProjectReadTxn<'a> {
    view_db: IndexedViewDb<'a>,
}

impl<'a> ProjectReadTxn<'a> {
    pub(crate) fn new(
        project: &'a ProjectState,
        cancellation: rg_std::CancellationToken,
    ) -> anyhow::Result<Self> {
        let subset = subset::all(&project.workspace);
        Self::for_subset(project, &subset, cancellation)
    }

    #[rg_std::cancelable("open project read view", token = cancellation)]
    pub(crate) fn for_subset(
        project: &'a ProjectState,
        subset: &PackageSubset,
        cancellation: rg_std::CancellationToken,
    ) -> anyhow::Result<Self> {
        let loaders = project.query_read_loaders();

        Ok(Self {
            view_db: IndexedViewDb::new(
                project
                    .def_map
                    .read_txn_for_subset(loaders.def_map.clone(), subset),
                project
                    .semantic_ir
                    .read_txn_for_subset(loaders.semantic_ir.clone(), subset),
                project.body_ir.read_txn_for_subset(loaders.body_ir, subset),
                cancellation,
            ),
        })
    }

    pub(crate) fn view_db(&self) -> &IndexedViewDb<'a> {
        &self.view_db
    }
}

impl ProjectState {
    /// Starts a read transaction over resident and lazy-loadable offloaded packages.
    pub(crate) fn read_txn(
        &self,
        cancellation: rg_std::CancellationToken,
    ) -> anyhow::Result<ProjectReadTxn<'_>> {
        ProjectReadTxn::new(self, cancellation)
    }

    pub(crate) fn read_txn_for_subset(
        &self,
        subset: &PackageSubset,
        cancellation: rg_std::CancellationToken,
    ) -> anyhow::Result<ProjectReadTxn<'_>> {
        ProjectReadTxn::for_subset(self, subset, cancellation)
    }

    /// Starts a def-map-only read transaction over selected package slots.
    pub(crate) fn def_map_read_txn_for_subset(&self, subset: &PackageSubset) -> DefMapReadTxn<'_> {
        let loaders = self.query_read_loaders();
        self.def_map.read_txn_for_subset(loaders.def_map, subset)
    }

    /// Returns the high-level query API for this frozen project analysis.
    pub(crate) fn analysis<'a>(&'a self, txn: &ProjectReadTxn<'a>) -> Analysis<'a> {
        Analysis::new(txn.view_db().clone(), SavedSourceView::new(self.parse_db()))
    }
}
