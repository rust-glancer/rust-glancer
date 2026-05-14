use std::{sync::Arc, time::Instant};

use anyhow::Context as _;
use rg_project::{DirtyFileChange, Project};

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
}

impl DirtyOverlayCache {
    pub(crate) fn new(memory_control: Arc<dyn MemoryControl>) -> Self {
        Self {
            memory_control,
            cached: None,
        }
    }

    pub(crate) fn clear(&mut self) {
        self.cached = None;
    }

    pub(crate) fn project_for_dirty(
        &mut self,
        base: &Project,
        dirty: &DirtyDocumentSnapshot,
    ) -> anyhow::Result<&Project> {
        let identity = DirtyDocumentIdentity::from_snapshot(dirty);
        let should_rebuild = match &self.cached {
            Some(cached) => cached.identity != identity,
            None => true,
        };

        if should_rebuild {
            let started = Instant::now();
            let memory_control = self.memory_control.as_ref();
            let memory_before =
                MemoryReporter::log_checkpoint(memory_control, "dirty_overlay", "before_rebuild");
            let overlay = base
                .dirty_overlay([DirtyFileChange::new(dirty.path(), dirty.text().to_string())])
                .with_context(|| {
                    format!(
                        "while attempting to build dirty analysis overlay for {}",
                        dirty.path().display()
                    )
                })?;
            MemoryReporter::log_checkpoint_delta(
                memory_control,
                "dirty_overlay",
                "after_rebuild",
                memory_before,
            );
            // Dirty overlay rebuilds can temporarily materialize much more allocator memory than
            // the retained overlay needs. Purge at this rebuild boundary without making every
            // read-only query pay the same cost.
            MemoryReporter::purge_and_report(memory_control, "after dirty overlay");
            let changed_known_file = overlay.is_some();
            let project = overlay.unwrap_or_else(|| base.clone());
            tracing::debug!(
                path = %dirty.path().display(),
                version = ?dirty.version(),
                text_len = dirty.text().len(),
                dirty_overlay_cache_hit = false,
                dirty_overlay_changed_known_file = changed_known_file,
                dirty_overlay_build_ms = started.elapsed().as_millis(),
                "dirty analysis overlay rebuilt"
            );
            self.cached = Some(CachedDirtyOverlay { identity, project });
        } else {
            tracing::debug!(
                path = %dirty.path().display(),
                version = ?dirty.version(),
                text_len = dirty.text().len(),
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
    project: Project,
}
