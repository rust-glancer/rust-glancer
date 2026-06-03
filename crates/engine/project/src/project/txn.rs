//! Project-level read transactions.

use rg_ir_view::IndexedViewDb;
use rg_package_store::PackageSubset;

use super::{loading::PackageReadLoaders, state::ProjectState, subset};

/// Read transaction for project-level query APIs.
///
/// The transaction is request-scoped: query callers create it once, build an `Analysis` view from
/// it, and reuse that view for the duration of the request.
#[derive(Debug, Clone)]
pub(crate) struct ProjectReadTxn<'a> {
    view_db: IndexedViewDb<'a>,
}

impl<'a> ProjectReadTxn<'a> {
    pub(crate) fn new(project: &'a ProjectState) -> anyhow::Result<Self> {
        let subset = subset::all(&project.workspace);
        Self::for_subset(project, &subset)
    }

    pub(crate) fn for_subset(
        project: &'a ProjectState,
        subset: &PackageSubset,
    ) -> anyhow::Result<Self> {
        let loaders = PackageReadLoaders::new(project);

        Ok(Self {
            view_db: IndexedViewDb::new(
                project
                    .def_map
                    .read_txn_for_subset(loaders.def_map.clone(), subset),
                project
                    .semantic_ir
                    .read_txn_for_subset(loaders.semantic_ir.clone(), subset),
                project.body_ir.read_txn_for_subset(loaders.body_ir, subset),
            ),
        })
    }

    pub(crate) fn view_db(&self) -> &IndexedViewDb<'a> {
        &self.view_db
    }
}
