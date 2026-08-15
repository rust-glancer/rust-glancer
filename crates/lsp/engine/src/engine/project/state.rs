//! Stores the saved project owned by the engine's serialized command loop.

use std::path::PathBuf;

use anyhow::Context as _;
use rg_project::{AnalysisSurface, DetachedSplitIndexing, Project, ProjectSnapshot};

/// Owns the one saved project used by all analysis queries in this engine.
///
/// Background indexing remembers the project's generation id. When it finishes, the command loop
/// uses that id to reject work from a project that has since been replaced. Finishing deferred
/// indexing or loading an offloaded package keeps the same id because the saved source did not
/// change.
#[derive(Debug)]
pub(super) struct ProjectState {
    saved: Option<Project>,
}

impl ProjectState {
    pub(super) fn new() -> Self {
        Self { saved: None }
    }

    pub(super) fn is_initialized(&self) -> bool {
        self.saved.is_some()
    }

    /// Publish a replacement saved project.
    pub(super) fn replace_saved(&mut self, project: Project) -> u64 {
        let generation = project.generation_id().get();
        self.saved = Some(project);
        generation
    }

    pub(super) fn generation(&self) -> u64 {
        self.saved
            .as_ref()
            .map(|project| project.generation_id().get())
            .unwrap_or(0)
    }

    /// Clone saved analysis into a generation-paired deferred-finish handle.
    ///
    /// Raw `Project` ownership stays on the coordinator lane. Background split-indexing only needs
    /// a detached finish capability, and the paired generation lets the coordinator reject a
    /// result produced from an older saved-source snapshot.
    pub(super) fn detach_saved_split_indexing(
        &self,
    ) -> anyhow::Result<(u64, DetachedSplitIndexing)> {
        let saved = self
            .saved
            .as_ref()
            .context("saved project is not initialized")?;
        Ok((saved.generation_id().get(), saved.detach_split_indexing()))
    }

    /// Run a mutation that may publish a new saved-source generation.
    ///
    /// The `Project` operation is responsible for its transactional update.
    pub(super) fn mutate_saved<T>(
        &mut self,
        mutation: impl FnOnce(&mut Project) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let saved = self
            .saved
            .as_mut()
            .context("saved project is not initialized")?;

        mutation(saved).context("mutate saved project")
    }

    /// Enrich analysis data for the same saved source snapshot.
    ///
    /// Background merging can replace package payloads without changing source identity.
    pub(super) fn mutate_saved_preserving_generation<T>(
        &mut self,
        mutation: impl FnOnce(&mut Project) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let saved = self
            .saved
            .as_mut()
            .context("saved project is not initialized")?;

        mutation(saved).context("mutate saved project without generation change")
    }

    pub(super) fn saved_snapshot(&self) -> anyhow::Result<ProjectSnapshot<'_>> {
        self.saved
            .as_ref()
            .map(Project::snapshot)
            .context("saved project is not initialized")
    }

    /// Load deferred analysis data needed by a query without changing the source generation.
    pub(super) fn materialize_saved_project(
        &mut self,
        surface: AnalysisSurface<'_>,
    ) -> anyhow::Result<()> {
        let saved = self
            .saved
            .as_mut()
            .context("saved project is not initialized")?;
        if saved.split_indexing().needs_materialization(surface) {
            saved
                .split_indexing()
                .materialize(surface)
                .context("materialize saved project analysis surface")?;
        }
        Ok(())
    }

    /// Reconcile a failed candidate with every known source changed since the published snapshot.
    pub(super) fn stale_saved_source_paths(&self) -> anyhow::Result<Vec<PathBuf>> {
        self.saved
            .as_ref()
            .context("saved project is not initialized")?
            .stale_saved_source_paths()
            .context("scan published project for stale saved sources")
    }

    /// Drop loads retained only to serve the request that just finished.
    pub(super) fn release_query_memory(&mut self) {
        if let Some(saved) = &mut self.saved {
            saved.release_query_memory();
        }
    }
}
