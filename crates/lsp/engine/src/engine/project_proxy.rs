use std::sync::Arc;

use anyhow::Context as _;
use rg_project::{Project, ProjectSnapshot};

use crate::{
    dirty_state::DirtyOverlayCache, documents::DirtyDocumentSnapshot, memory::MemoryControl,
};

/// Owns the saved project and the disposable dirty overlay used by read-only queries.
///
/// `generation` is the saved-project identity seen by asynchronous work. It changes when saved
/// source state is replaced or rebuilt, so background results produced from an older `Project`
/// clone can be discarded instead of merged into a newer workspace. Split-indexing materialization
/// intentionally keeps the same generation: it enriches analysis data for the same saved source
/// snapshot, but it still clears dirty overlays because those overlays borrow from the old project
/// internals.
#[derive(Debug)]
pub(super) struct ProjectProxy {
    saved: Option<Project>,
    generation: u64,
    dirty_overlay: DirtyOverlayCache,
}

impl ProjectProxy {
    pub(super) fn new(memory_control: Arc<dyn MemoryControl>) -> Self {
        Self {
            saved: None,
            generation: 0,
            dirty_overlay: DirtyOverlayCache::new(memory_control),
        }
    }

    pub(super) fn is_initialized(&self) -> bool {
        self.saved.is_some()
    }

    pub(super) fn replace_saved(&mut self, project: Project) -> u64 {
        self.generation = self.generation.saturating_add(1);
        self.saved = Some(project);
        self.dirty_overlay.clear();
        self.generation
    }

    pub(super) fn generation(&self) -> u64 {
        self.generation
    }

    pub(super) fn mutate_saved<T>(
        &mut self,
        mutation: impl FnOnce(&mut Project) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let saved = self
            .saved
            .as_mut()
            .context("LSP engine is not initialized")?;

        // Any saved-project mutation attempt may leave the project in a different state even if it
        // returns an error, so discard overlays derived from the previous saved state up front.
        self.generation = self.generation.saturating_add(1);
        self.dirty_overlay.clear();
        mutation(saved)
    }

    pub(super) fn mutate_saved_preserving_generation<T>(
        &mut self,
        mutation: impl FnOnce(&mut Project) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let saved = self
            .saved
            .as_mut()
            .context("LSP engine is not initialized")?;

        // Body-only materialization keeps the same saved-source snapshot, but dirty overlays still
        // need to be rebuilt because they borrow analysis state from the previous saved project.
        self.dirty_overlay.clear();
        mutation(saved)
    }

    pub(super) fn saved_snapshot(&self) -> anyhow::Result<ProjectSnapshot<'_>> {
        self.saved
            .as_ref()
            .map(Project::snapshot)
            .context("LSP engine is not initialized")
    }

    pub(super) fn release_query_memory(&mut self) {
        if let Some(saved) = &mut self.saved {
            saved.release_query_memory();
        }
        self.dirty_overlay.release_query_memory();
    }

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
                    .context("LSP engine is not initialized")?;
                self.dirty_overlay.project_for_dirty(saved, dirty)?
            }
            None => {
                self.dirty_overlay.clear();
                self.saved
                    .as_ref()
                    .context("LSP engine is not initialized")?
            }
        };

        query(project.snapshot())
    }
}
