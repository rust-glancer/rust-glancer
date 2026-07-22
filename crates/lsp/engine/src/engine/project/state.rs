//! Saved project generation and its disposable dirty overlay.
//!
//! `ProjectState` answers one question for query code: which `Project` snapshot should this request
//! read? Clean requests borrow the saved project. A full-text dirty request borrows a cached
//! single-file overlay built from that same saved generation. Source publication, query-time
//! materialization, and dirty overlays remain distinct transitions here instead of being inferred
//! by callers.

use std::{path::PathBuf, sync::Arc};

use anyhow::Context as _;
use rg_project::{
    AnalysisSurface, DetachedSplitIndexing, DirtyOverlayScope, Project, ProjectSnapshot,
};

use crate::{
    dirty_state::DirtyOverlayCache, documents::DirtyDocumentSnapshot, memory::MemoryControl,
};

/// Owns the saved project and the disposable dirty overlay used by read-only queries.
///
/// The saved project's generation identity is seen by asynchronous work. It changes only when
/// `Project` successfully publishes saved source state, so background results produced from an
/// older clone can be discarded instead of merged into a newer workspace. Split-indexing
/// materialization intentionally keeps the same generation: it enriches analysis data for the same
/// saved source snapshot. A materialization that replaces package payloads clears dirty overlays
/// because they borrow the old project internals; preparing an already-ready surface leaves the
/// matching overlay cached.
#[derive(Debug)]
pub(super) struct ProjectState {
    saved: Option<Project>,
    dirty_overlay: DirtyOverlayCache,
}

impl ProjectState {
    pub(super) fn new(memory_control: Arc<dyn MemoryControl>) -> Self {
        Self {
            saved: None,
            dirty_overlay: DirtyOverlayCache::new(memory_control),
        }
    }

    pub(super) fn is_initialized(&self) -> bool {
        self.saved.is_some()
    }

    /// Publish a replacement saved project and invalidate overlays based on the old one.
    pub(super) fn replace_saved(&mut self, project: Project) -> u64 {
        let generation = project.generation_id().get();
        self.saved = Some(project);
        self.dirty_overlay.clear();
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
    /// old dirty overlay because a successful source mutation gives future overlays a different
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
        // must not discard an overlay that still has the same saved base.
        let previous_generation = saved.generation_id();
        let result = mutation(saved).context("mutate saved project");
        if result.is_ok() && saved.generation_id() != previous_generation {
            self.dirty_overlay.clear();
        }
        result
    }

    /// Enrich analysis data for the same saved source snapshot.
    ///
    /// Background merging can replace package payloads without changing source identity. Dirty
    /// overlays still have to be discarded because they borrow the previous package payloads, even
    /// though the public generation number stays the same.
    pub(super) fn mutate_saved_preserving_generation<T>(
        &mut self,
        mutation: impl FnOnce(&mut Project) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let saved = self
            .saved
            .as_mut()
            .context("saved project is not initialized")?;

        self.dirty_overlay.clear();
        mutation(saved).context("mutate saved project without generation change")
    }

    /// Materialize a query surface without discarding an overlay for an already-ready project.
    pub(super) fn materialize(&mut self, surface: AnalysisSurface<'_>) -> anyhow::Result<()> {
        let Self {
            saved,
            dirty_overlay,
        } = self;
        let saved = saved.as_mut().context("saved project is not initialized")?;

        // Materialization is a real mutation only for incomplete resident packages. Artifact-backed
        // packages and already-complete residents can serve the query without changing the base
        // shared by a cached dirty overlay.
        if !saved.split_indexing().needs_materialization(surface) {
            return Ok(());
        }

        dirty_overlay.clear();
        saved
            .split_indexing()
            .materialize(surface)
            .context("materialize saved project surface")
    }

    pub(super) fn saved_snapshot(&self) -> anyhow::Result<ProjectSnapshot<'_>> {
        self.saved
            .as_ref()
            .map(Project::snapshot)
            .context("saved project is not initialized")
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
        self.dirty_overlay.release_query_memory();
    }

    #[cfg(test)]
    pub(super) fn dirty_overlay_rebuild_count(&self) -> usize {
        self.dirty_overlay.rebuild_count()
    }

    /// Select the saved project or matching dirty overlay and lend out one snapshot.
    ///
    /// The closure keeps snapshots from escaping this state. In particular, a later command may
    /// clear or replace the overlay without any request retaining a reference to it.
    pub(super) fn with_query_snapshot<T>(
        &mut self,
        dirty: Option<&DirtyDocumentSnapshot>,
        dirty_scope: DirtyOverlayScope,
        query: impl FnOnce(ProjectSnapshot<'_>) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let project = match dirty {
            Some(dirty) => {
                let saved = self
                    .saved
                    .as_ref()
                    .context("saved project is not initialized")?;
                self.dirty_overlay
                    .project_for_dirty(saved, dirty, dirty_scope)
                    .context("build dirty project overlay")?
            }
            None => {
                self.dirty_overlay.clear();
                self.saved
                    .as_ref()
                    .context("saved project is not initialized")?
            }
        };

        query(project.snapshot()).context("execute project query")
    }
}
