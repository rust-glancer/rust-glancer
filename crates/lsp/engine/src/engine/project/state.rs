//! Saved project generation and its disposable dirty overlay.
//!
//! `ProjectState` answers one question for query code: which `Project` snapshot should this request
//! read? Clean requests borrow the saved project. A full-text dirty request borrows a cached
//! single-file overlay built from that same saved generation. Source publication, query-time
//! materialization, and dirty overlays remain distinct transitions here instead of being inferred
//! by callers.

use std::sync::Arc;

use anyhow::Context as _;
use rg_project::{DetachedSplitIndexing, Project, ProjectSnapshot};

use crate::{
    dirty_state::DirtyOverlayCache, documents::DirtyDocumentSnapshot, memory::MemoryControl,
};

/// Owns the saved project and the disposable dirty overlay used by read-only queries.
///
/// The saved project's generation identity is seen by asynchronous work. It changes only when
/// `Project` successfully publishes saved source state, so background results produced from an
/// older clone can be discarded instead of merged into a newer workspace. Split-indexing
/// materialization intentionally keeps the same generation: it enriches analysis data for the same
/// saved source snapshot, but it still clears dirty overlays because those overlays borrow from the
/// old project internals.
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
        // Clear overlays up front because a successful mutation changes their base generation.
        self.dirty_overlay.clear();
        mutation(saved).context("mutate saved project")
    }

    /// Enrich analysis data for the same saved source snapshot.
    ///
    /// Query materialization and background merging can replace package payloads without changing
    /// source identity. Dirty overlays still have to be discarded because they borrow the previous
    /// package payloads, even though the public generation number stays the same.
    pub(super) fn mutate_saved_preserving_generation<T>(
        &mut self,
        mutation: impl FnOnce(&mut Project) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let saved = self
            .saved
            .as_mut()
            .context("saved project is not initialized")?;

        // Body-only materialization keeps the same saved-source snapshot, but dirty overlays still
        // need to be rebuilt because they borrow analysis state from the previous saved project.
        self.dirty_overlay.clear();
        mutation(saved).context("mutate saved project without generation change")
    }

    pub(super) fn saved_snapshot(&self) -> anyhow::Result<ProjectSnapshot<'_>> {
        self.saved
            .as_ref()
            .map(Project::snapshot)
            .context("saved project is not initialized")
    }

    /// Drop loads retained only to serve the request that just finished.
    pub(super) fn release_query_memory(&mut self) {
        if let Some(saved) = &mut self.saved {
            saved.release_query_memory();
        }
        self.dirty_overlay.release_query_memory();
    }

    /// Select the saved project or matching dirty overlay and lend out one snapshot.
    ///
    /// The closure keeps snapshots from escaping this state. In particular, a later command may
    /// clear or replace the overlay without any request retaining a reference to it.
    pub(super) fn with_query_snapshot<T>(
        &mut self,
        dirty: Option<&DirtyDocumentSnapshot>,
        query: impl FnOnce(ProjectSnapshot<'_>) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let project = match dirty {
            Some(dirty) => {
                let saved = self
                    .saved
                    .as_ref()
                    .context("saved project is not initialized")?;
                self.dirty_overlay
                    .project_for_dirty(saved, dirty)
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
