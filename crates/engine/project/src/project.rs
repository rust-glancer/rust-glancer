//! Owns the accepted source generation and serializes access to its query runtime.

use std::{
    fmt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use rg_ir_model::FileId;
use rg_source::{CapturedSource, SourceError};
use rg_std::{CancellationToken, MemoryRecorder, MemorySize};
use rg_text::RustEdition;
use rg_workspace::{CargoMetadataConfig, WorkspaceLoweringConfig, WorkspaceMetadata};

use crate::{
    MacroExpansionLimitBuildSummary, MacroExpansionLimitReport, ProjectBuilder,
    ProjectGenerationId, ProjectMemoryHooks, ProjectMemoryPurgePoint, ProjectSnapshot,
    ProjectStats, project_queries::ProjectQueries, state::ProjectState,
};

#[derive(MemorySize)]
pub struct Project {
    #[memsize(with = "Self::record_queries_memory")]
    pub(crate) queries: Mutex<ProjectQueries>,
    pub(crate) state: ProjectState,
    #[memsize(skip)]
    pub(crate) workspace_lowering_config: WorkspaceLoweringConfig,
    pub(crate) cargo_metadata_config: CargoMetadataConfig,
    #[memsize(skip)]
    pub(crate) memory_hooks: Arc<dyn ProjectMemoryHooks>,
}

impl Project {
    fn record_queries_memory(queries: &Mutex<ProjectQueries>, recorder: &mut MemoryRecorder) {
        queries
            .lock()
            .expect("project query runtime")
            .record_memory_children(recorder);
    }

    /// Write pending query results and release unused memory at a host idle boundary.
    /// An already accepted save stays published if cache maintenance fails.
    pub fn collect(&self) {
        self.collect_at(ProjectMemoryPurgePoint::Idle);
    }

    pub(crate) fn collect_at(&self, point: ProjectMemoryPurgePoint) {
        let result = self
            .queries
            .lock()
            .expect("project query runtime")
            .collect();
        if let Err(error) = result {
            let persistence_kind = match error.kind() {
                ajika::ErrorKind::Persistence(error) => Some(error.kind()),
                _ => None,
            };
            tracing::warn!(%error, ?persistence_kind, boundary = point.label(), "query storage maintenance failed");
        }
        self.memory_hooks.purge(point);
    }

    pub fn builder(workspace: WorkspaceMetadata) -> ProjectBuilder {
        ProjectBuilder::new(workspace)
    }

    /// Lend the accepted saved source view and one working set to a request. The callback can return
    /// owned answers, while borrows of decoded declarations and bodies stay inside the operation.
    pub fn with_snapshot<R>(
        &self,
        cancellation: CancellationToken,
        work: impl FnOnce(ProjectSnapshot<'_>) -> anyhow::Result<R>,
    ) -> anyhow::Result<R> {
        self.with_editor_snapshot(&[], cancellation, work)
    }

    /// Analyze the saved project with a complete captured set of edited documents. Unsaved
    /// declarations affect other files through the ordinary queries, and their changed results
    /// remain reusable in the bounded session store instead of replacing saved results on disk.
    ///
    /// Supply every active edit on each call. For example, after publishing a save of `a.rs`,
    /// keep supplying the edit to `b.rs` until it is saved or discarded. Omitting a document
    /// restores its saved input. Captures equal to saved bytes use persistent inputs.
    ///
    /// This does not publish a new saved project generation. Returned source coordinates belong
    /// to this captured view, including edits in files other than the one being queried.
    pub fn with_editor_snapshot<R>(
        &self,
        documents: &[CapturedSource],
        cancellation: CancellationToken,
        work: impl FnOnce(ProjectSnapshot<'_>) -> anyhow::Result<R>,
    ) -> anyhow::Result<R> {
        self.queries
            .lock()
            .expect("project query runtime")
            .with_snapshot(&self.state, documents, cancellation, work)
    }

    /// Bound retained editor query results independently of disk retention. Limits cover their
    /// estimated encoded size, count and idle age. Editor text stays alive with its input view so
    /// an evicted result can be recomputed.
    pub fn set_session_retention(&self, retention: ajika::Retention) {
        self.queries
            .lock()
            .expect("project query runtime")
            .set_session_retention(retention);
    }

    /// Release retained editor inputs after their host capture is superseded. Query memos retain
    /// their normal bounded session policy; the next operation installs a complete capture again.
    pub fn release_editor_snapshot(&self) {
        self.queries
            .lock()
            .expect("project query runtime")
            .release_editor_snapshot();
    }

    /// Release retained query work without discarding editor inputs. The next request can use
    /// the same captures and recompute its answers; this does not save or discard documents.
    pub fn clear_session(&self) {
        self.queries
            .lock()
            .expect("project query runtime")
            .clear_session();
    }

    pub fn generation_id(&self) -> ProjectGenerationId {
        self.state.generation_id
    }

    pub fn workspace(&self) -> &WorkspaceMetadata {
        self.state.workspace()
    }

    /// Choose an edition for syntax-only requests without discovering semantic owners.
    /// Collected membership handles paths outside a package, such as `#[path = "../shared.rs"]`.
    /// For other files, use Cargo's generated-file attribution or the nearest package root.
    /// A file outside all of these has no project edition; the caller chooses its default.
    pub fn source_edition(&self, source_path: &Path) -> anyhow::Result<Option<RustEdition>> {
        let file = FileId::for_path(
            &self
                .queries
                .lock()
                .expect("project query runtime")
                .interner(),
            source_path,
        )?;
        let workspace = self.workspace();
        let preferred = workspace.source_package(source_path);
        let package = self
            .state
            .sources
            .first_source_crate(file, preferred)
            .map(|krate| krate.package)
            .or(preferred);
        Ok(package.map(|slot| workspace.packages()[slot.0].edition))
    }

    pub fn stats(&self) -> ProjectStats {
        self.state.stats()
    }

    pub fn macro_expansion_limit_reports(
        &self,
    ) -> impl Iterator<Item = &MacroExpansionLimitReport> {
        self.state.reports.iter()
    }

    pub fn macro_expansion_limit_summary(&self) -> &MacroExpansionLimitBuildSummary {
        &self.state.macro_expansion_limit_summary
    }

    pub fn query_recovery_pending(&self) -> bool {
        self.queries
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
        self.queries
            .lock()
            .expect("project query runtime")
            .stale_saved_paths()
    }
}

impl fmt::Debug for Project {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Project")
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}
