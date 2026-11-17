//! Stores the saved project owned by the engine's serialized command loop.

use std::path::PathBuf;

use anyhow::Context as _;
use rg_project::Project;

/// Owns the one saved project used by all analysis queries in this engine.
///
/// Source changes publish a new identity. Loading or completing bodies keeps that identity.
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

    pub(super) fn saved_project(&self) -> anyhow::Result<&Project> {
        self.saved
            .as_ref()
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
}
