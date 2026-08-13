//! Saved project generation and its disposable source-override project.
//!
//! `ProjectState` answers one question for query code: which `Project` snapshot should this request
//! read? Workspace-only requests borrow the saved project. Document requests borrow a cached
//! project rebuilt from every applicable open buffer and that same saved generation. Saved source
//! publication, query-time materialization, and source override selection remain separate
//! transitions instead of being inferred by callers.

use std::{path::PathBuf, sync::Arc};

use anyhow::Context as _;
use rg_lsp_proto::EditorSnapshot;
use rg_project::{
    AnalysisSurface, DetachedSplitIndexing, Project, ProjectSnapshot, SourceOverrideScope,
};

use crate::memory::MemoryControl;

use super::source_overrides::SourceOverrideCache;

/// Owns the saved project and the disposable source-override project used by read-only queries.
///
/// The saved project's generation identity is seen by asynchronous work. It changes only when
/// `Project` successfully publishes saved source state, so background results produced from an
/// older clone can be discarded instead of merged into a newer workspace. Split-indexing
/// saved-project enrichment intentionally keeps the same generation because it does not change
/// source identity. Replacing saved package payloads clears source overrides because a derived
/// project cloned the old internals. Query-time materialization instead enriches the exact saved or
/// source-override project selected for that request.
#[derive(Debug)]
pub(super) struct ProjectState {
    saved: Option<Project>,
    source_overrides: SourceOverrideCache,
}

impl ProjectState {
    pub(super) fn new(memory_control: Arc<dyn MemoryControl>) -> Self {
        Self {
            saved: None,
            source_overrides: SourceOverrideCache::new(memory_control),
        }
    }

    pub(super) fn is_initialized(&self) -> bool {
        self.saved.is_some()
    }

    /// Publish a replacement saved project and invalidate source overrides based on the old one.
    pub(super) fn replace_saved(&mut self, project: Project) -> u64 {
        let generation = project.generation_id().get();
        self.saved = Some(project);
        self.source_overrides.clear();
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
    /// The `Project` operation is responsible for its transactional update. This layer clears the
    /// old source overrides because a successful source mutation gives future analysis a different
    /// base generation.
    pub(super) fn mutate_saved<T>(
        &mut self,
        mutation: impl FnOnce(&mut Project) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let saved = self
            .saved
            .as_mut()
            .context("saved project is not initialized")?;

        // Saved-source operations publish through `Project` only after their candidate succeeds.
        // An unchanged watcher replay returns successfully without advancing the generation, so it
        // must not discard a derived project that still has the same saved base.
        let previous_generation = saved.generation_id();
        let result = mutation(saved).context("mutate saved project");
        if result.is_ok() && saved.generation_id() != previous_generation {
            self.source_overrides.clear();
        }
        result
    }

    /// Enrich analysis data for the same saved source snapshot.
    ///
    /// Background merging can replace package payloads without changing source identity.
    /// Source-override projects still have to be discarded because they borrow the previous package
    /// payloads, even though the public generation number stays the same.
    pub(super) fn mutate_saved_preserving_generation<T>(
        &mut self,
        mutation: impl FnOnce(&mut Project) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let saved = self
            .saved
            .as_mut()
            .context("saved project is not initialized")?;

        self.source_overrides.clear();
        mutation(saved).context("mutate saved project without generation change")
    }

    pub(super) fn saved_snapshot(&self) -> anyhow::Result<ProjectSnapshot<'_>> {
        self.saved
            .as_ref()
            .map(Project::snapshot)
            .context("saved project is not initialized")
    }

    /// Materialize the same saved or source-override project that the query will read.
    pub(super) fn materialize_query_project(
        &mut self,
        editor: &EditorSnapshot,
        scope: SourceOverrideScope,
        surface: AnalysisSurface<'_>,
    ) -> anyhow::Result<()> {
        let saved = self
            .saved
            .as_mut()
            .context("saved project is not initialized")?;
        self.source_overrides
            .materialize_for_snapshot(saved, editor, scope, surface)
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
        self.source_overrides.release_query_memory();
    }

    #[cfg(test)]
    pub(super) fn source_override_rebuild_count(&self) -> usize {
        self.source_overrides.rebuild_count()
    }

    #[cfg(test)]
    pub(super) fn has_cached_override_project(&self) -> bool {
        self.source_overrides.has_cached_project()
    }

    /// Select the saved project or matching source-override project and lend out one snapshot.
    ///
    /// The closure keeps snapshots from escaping this state. In particular, a later command may
    /// clear or replace the derived project without any request retaining a reference to it.
    pub(super) fn with_query_snapshot<T>(
        &mut self,
        editor: Option<(&EditorSnapshot, SourceOverrideScope)>,
        query: impl FnOnce(ProjectSnapshot<'_>) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let project = match editor {
            Some((editor, scope)) => {
                let saved = self
                    .saved
                    .as_ref()
                    .context("saved project is not initialized")?;
                self.source_overrides
                    .project_for_snapshot(saved, editor, scope)
                    .context("build project from source overrides")?
            }
            None => {
                self.source_overrides.clear();
                self.saved
                    .as_ref()
                    .context("saved project is not initialized")?
            }
        };

        query(project.snapshot()).context("execute project query")
    }
}
