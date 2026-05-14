use std::sync::Arc;

use anyhow::Context as _;
use rg_project::{Project, ProjectSnapshot};

use crate::{
    dirty_state::DirtyOverlayCache, documents::DirtyDocumentSnapshot, memory::MemoryControl,
};

/// Owns the saved project and the disposable dirty overlay used by read-only queries.
#[derive(Debug)]
pub(super) struct ProjectProxy {
    saved: Option<Project>,
    dirty_overlay: DirtyOverlayCache,
}

impl ProjectProxy {
    pub(super) fn new(memory_control: Arc<dyn MemoryControl>) -> Self {
        Self {
            saved: None,
            dirty_overlay: DirtyOverlayCache::new(memory_control),
        }
    }

    pub(super) fn is_initialized(&self) -> bool {
        self.saved.is_some()
    }

    pub(super) fn replace_saved(&mut self, project: Project) {
        self.saved = Some(project);
        self.dirty_overlay.clear();
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
        self.dirty_overlay.clear();
        mutation(saved)
    }

    pub(super) fn saved_snapshot(&self) -> anyhow::Result<ProjectSnapshot<'_>> {
        self.saved
            .as_ref()
            .map(Project::snapshot)
            .context("LSP engine is not initialized")
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
