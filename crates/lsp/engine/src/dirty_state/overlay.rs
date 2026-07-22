use std::{sync::Arc, time::Instant};

use anyhow::Context as _;
use rg_project::{DirtyFileChange, DirtyOverlayScope, Project};

use super::DirtyDocumentIdentity;
use crate::{
    documents::DirtyDocumentSnapshot,
    memory::{MemoryControl, MemoryReporter},
};

/// Caches the most recent single-file dirty overlay built on top of the saved project.
#[derive(Debug)]
pub(crate) struct DirtyOverlayCache {
    memory_control: Arc<dyn MemoryControl>,
    cached: Option<CachedDirtyOverlay>,
    #[cfg(test)]
    // Keep this cumulative across `clear` so tests can observe an invalidation followed by rebuild.
    rebuild_count: usize,
}

impl DirtyOverlayCache {
    pub(crate) fn new(memory_control: Arc<dyn MemoryControl>) -> Self {
        Self {
            memory_control,
            cached: None,
            #[cfg(test)]
            rebuild_count: 0,
        }
    }

    pub(crate) fn clear(&mut self) {
        self.cached = None;
    }

    /// Keep the rebuilt overlay cached while releasing state owned by the query that just used it.
    pub(crate) fn release_query_memory(&mut self) {
        if let Some(cached) = &mut self.cached {
            cached.project.release_query_memory();
        }
    }

    #[cfg(test)]
    pub(crate) fn rebuild_count(&self) -> usize {
        self.rebuild_count
    }

    pub(crate) fn project_for_dirty(
        &mut self,
        base: &Project,
        dirty: &DirtyDocumentSnapshot,
        scope: DirtyOverlayScope,
    ) -> anyhow::Result<&Project> {
        let identity = DirtyDocumentIdentity::from_snapshot(dirty);
        let base_generation = base.generation_id();
        let should_rebuild = match &self.cached {
            Some(cached) => {
                cached.identity != identity
                    || cached.base_generation != base_generation
                    || !cached.scope.covers(scope)
            }
            None => true,
        };

        if should_rebuild {
            // A cache miss means the stored overlay has a different dirty identity or base
            // generation, or it does not cover the requested package scope. Drop it before
            // constructing the replacement so edit bursts do not keep two overlays live across
            // the expensive package rebuild.
            self.cached = None;

            let started = Instant::now();
            let memory_control = self.memory_control.as_ref();
            let memory_before = MemoryReporter::snapshot(memory_control);
            let overlay = base
                .dirty_overlay(
                    scope,
                    [DirtyFileChange::new(dirty.path(), dirty.text().to_string())],
                )
                .with_context(|| {
                    format!(
                        "while attempting to build dirty analysis overlay for {}",
                        dirty.path().display()
                    )
                })?;
            MemoryReporter::log_delta_debug(
                memory_control,
                "dirty_overlay",
                "after_rebuild",
                memory_before,
            );
            let changed_known_file = overlay.is_some();
            let project = overlay.unwrap_or_else(|| base.clone());
            tracing::debug!(
                path = %dirty.path().display(),
                version = ?dirty.version(),
                text_len = dirty.text().len(),
                dirty_overlay_scope = ?scope,
                dirty_overlay_cache_hit = false,
                dirty_overlay_changed_known_file = changed_known_file,
                dirty_overlay_build_ms = started.elapsed().as_millis(),
                "dirty analysis overlay rebuilt"
            );
            self.cached = Some(CachedDirtyOverlay {
                identity,
                base_generation,
                scope,
                project,
            });
            #[cfg(test)]
            {
                self.rebuild_count += 1;
            }
        } else {
            tracing::debug!(
                path = %dirty.path().display(),
                version = ?dirty.version(),
                text_len = dirty.text().len(),
                dirty_overlay_scope = ?scope,
                dirty_overlay_cache_hit = true,
                dirty_overlay_build_ms = 0_u128,
                "dirty analysis overlay cache hit"
            );
        }

        Ok(&self
            .cached
            .as_ref()
            .expect("dirty overlay should be cached after successful build")
            .project)
    }
}

#[derive(Debug)]
struct CachedDirtyOverlay {
    identity: DirtyDocumentIdentity,
    base_generation: rg_project::ProjectGenerationId,
    scope: DirtyOverlayScope,
    project: Project,
}
