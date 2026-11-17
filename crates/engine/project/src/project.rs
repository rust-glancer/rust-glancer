//! Owns the accepted source generation and serializes access to its query runtime.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use rg_semantic_ir::TargetDeclarationReader;
use rg_source::{QueryDatabase, QueryServices, ReadError, SourceError, SourceInput};
use rg_std::{CancellationToken, MemorySize};
use rg_workspace::{CargoMetadataConfig, WorkspaceLoweringConfig, WorkspaceMetadata};

use crate::{
    MacroExpansionLimitBuildSummary, ProjectBuilder, ProjectGenerationId, ProjectMemoryHooks,
    ProjectSnapshot, ProjectStats, state::ProjectState,
};

#[derive(MemorySize)]
pub struct Project {
    #[memsize(skip)]
    pub(crate) database: Mutex<QueryDatabase>,
    pub(crate) state: ProjectState,
    #[memsize(skip)]
    pub(crate) workspace_lowering_config: WorkspaceLoweringConfig,
    pub(crate) cargo_metadata_config: CargoMetadataConfig,
    #[memsize(skip)]
    pub(crate) memory_hooks: Arc<dyn ProjectMemoryHooks>,
}

impl Project {
    pub fn builder(workspace: WorkspaceMetadata) -> ProjectBuilder {
        ProjectBuilder::new(workspace)
    }

    /// Lend the accepted source view and one working set to a request. The callback can return
    /// owned answers, while borrows of decoded declarations and bodies stay inside the operation.
    pub fn with_snapshot<R>(
        &self,
        cancellation: CancellationToken,
        work: impl FnOnce(ProjectSnapshot<'_>) -> anyhow::Result<R>,
    ) -> anyhow::Result<R> {
        let mut database = self.database.lock().expect("project query runtime");
        database.set_services(QueryServices { cancellation });
        let result = database.run(|db| {
            let declarations =
                TargetDeclarationReader::for_project(db, self.state.targets.iter().copied());
            let snapshot = ProjectSnapshot::new(&self.state, db, &declarations);
            work(snapshot).map_err(|error| {
                ReadError::query_cause(error.as_ref()).unwrap_or_else(|| {
                    ajika::Error::application(
                        error.reallocate_into_boxed_dyn_error_without_backtrace(),
                    )
                })
            })
        });
        database.set_services(QueryServices::default());
        Ok(result?)
    }

    pub fn generation_id(&self) -> ProjectGenerationId {
        self.state.generation_id
    }

    pub fn workspace(&self) -> &WorkspaceMetadata {
        self.state.workspace()
    }

    pub fn stats(&self) -> ProjectStats {
        self.state.stats()
    }

    pub fn macro_expansion_limit_reports(
        &self,
    ) -> impl Iterator<Item = &rg_def_map::MacroExpansionLimitReport> {
        self.state.reports.iter()
    }

    pub fn macro_expansion_limit_summary(&self) -> &MacroExpansionLimitBuildSummary {
        &self.state.macro_expansion_limit_summary
    }

    pub fn query_recovery_pending(&self) -> bool {
        self.database
            .lock()
            .expect("project query runtime")
            .recovery_pending()
    }

    pub fn stale_source_path(error: &anyhow::Error) -> Option<&Path> {
        error.chain().find_map(|cause| {
            cause
                .downcast_ref::<SourceError>()
                .and_then(SourceError::stale_path)
        })
    }

    /// After a source race, find other captured files in the same disk write burst.
    pub fn stale_saved_source_paths(&self) -> ajika::Result<Vec<PathBuf>> {
        let database = self.database.lock().expect("project query runtime");
        SourceInput::stale_saved_paths(database.inputs(), &database.interner())
    }
}

impl std::fmt::Debug for Project {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Project")
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}
