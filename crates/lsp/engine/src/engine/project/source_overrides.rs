//! Bounded selection of saved analysis or a project derived from source overrides.
//!
//! The immutable `EditorSnapshot` contains every applicable open source and its frozen project
//! identity. This module compares those values with one saved generation. If every value matches,
//! queries use the saved project directly. Otherwise this module keeps at most one disposable
//! derived project covering the requested package scope.
//!
//! This cache is not document authority. Its key is the complete editor revision plus saved base
//! generation, and its contents may be dropped after any request. Rebuilding from the immutable
//! snapshot is always the ordinary miss path.

use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Instant};

use anyhow::Context as _;
use rg_lsp_proto::EditorSnapshot;
use rg_project::{AnalysisSurface, Project, SourceOverrideScope};
use rg_source::CapturedSource;

use crate::memory::{MemoryControl, MemoryReporter};

/// Owns the one source-override project that may accompany the published saved project.
///
/// The cache key covers the complete editor snapshot, not one privileged target document. A
/// broader scope may satisfy a later local query, while any editor or saved generation change
/// replaces the derived project wholesale.
#[derive(Debug)]
pub(super) struct SourceOverrideCache {
    memory_control: Arc<dyn MemoryControl>,
    cached: Option<CachedProjectSelection>,
    #[cfg(test)]
    // Keep this cumulative across `clear` so tests can observe invalidation followed by rebuild.
    rebuild_count: usize,
}

impl SourceOverrideCache {
    pub(super) fn new(memory_control: Arc<dyn MemoryControl>) -> Self {
        Self {
            memory_control,
            cached: None,
            #[cfg(test)]
            rebuild_count: 0,
        }
    }

    pub(super) fn clear(&mut self) {
        self.cached = None;
    }

    /// Release request-owned data without preserving a cache entry whose premise was evicted.
    pub(super) fn release_query_memory(&mut self) {
        // A clean selection is cached as `project: None` after matching editor text may have been
        // restored into the saved project's evictable source cells. Request cleanup can evict those
        // bytes. Drop the selection too, so the same editor revision repeats the comparison and
        // restores its bytes instead of assuming they are still resident.
        if self
            .cached
            .as_ref()
            .is_some_and(|cached| cached.project.is_none())
        {
            self.cached = None;
            return;
        }
        if let Some(cached) = &mut self.cached
            && let Some(project) = &mut cached.project
        {
            project.release_query_memory();
        }
    }

    #[cfg(test)]
    pub(super) fn rebuild_count(&self) -> usize {
        self.rebuild_count
    }

    #[cfg(test)]
    pub(super) fn has_cached_project(&self) -> bool {
        self.cached
            .as_ref()
            .is_some_and(|cached| cached.project.is_some())
    }

    /// Select the saved project or a derived project covering this editor snapshot and scope.
    pub(super) fn project_for_snapshot<'a>(
        &'a mut self,
        base: &'a Project,
        editor: &EditorSnapshot,
        scope: SourceOverrideScope,
    ) -> anyhow::Result<&'a Project> {
        self.ensure_project(base, editor, scope)?;
        let cached = self
            .cached
            .as_ref()
            .expect("source override selection should be cached after successful build");
        Ok(cached.project.as_ref().unwrap_or(base))
    }

    /// Materialize the exact saved or source-override project that the query will read.
    pub(super) fn materialize_for_snapshot(
        &mut self,
        base: &mut Project,
        editor: &EditorSnapshot,
        scope: SourceOverrideScope,
        surface: AnalysisSurface<'_>,
    ) -> anyhow::Result<()> {
        self.ensure_project(base, editor, scope)?;
        let cached = self
            .cached
            .as_mut()
            .expect("source override selection should be cached after successful build");
        let selected = cached.project.as_mut().unwrap_or(base);
        if !selected.split_indexing().needs_materialization(surface) {
            return Ok(());
        }
        selected
            .split_indexing()
            .materialize(surface)
            .context("materialize query-selected project surface")
    }

    /// Rebuild the cached selection when its editor revision, saved generation, or scope changes.
    fn ensure_project(
        &mut self,
        base: &Project,
        editor: &EditorSnapshot,
        scope: SourceOverrideScope,
    ) -> anyhow::Result<()> {
        let base_generation = base.generation_id();
        let should_rebuild = match &self.cached {
            Some(cached) => {
                cached.editor_revision != editor.revision()
                    || cached.base_generation != base_generation
                    || (cached.project.is_some() && !cached.scope.covers(scope))
            }
            None => true,
        };

        if should_rebuild {
            // The project graph is the large value here. Drop the previous generation before the
            // replacement starts rebuilding so bursts remain bounded to one derived project.
            self.cached = None;

            let started = Instant::now();
            let memory_control = self.memory_control.as_ref();
            let memory_before = MemoryReporter::snapshot(memory_control);
            let sources = Self::capture_sources(base, editor)?;
            let derived = base
                .derive_with_source_overrides(scope, sources)
                .with_context(|| {
                    format!(
                        "while attempting to build source overrides for editor revision {}",
                        editor.revision().get()
                    )
                })?;
            MemoryReporter::log_delta_debug(
                memory_control,
                "source_overrides",
                "after_rebuild",
                memory_before,
            );
            let retained_derived_project = derived.is_some();
            tracing::debug!(
                editor_revision = editor.revision().get(),
                document_count = editor.documents().len(),
                source_override_scope = ?scope,
                source_override_cache_hit = false,
                source_override_retained_project = retained_derived_project,
                source_override_build_ms = started.elapsed().as_millis(),
                "source override selection rebuilt"
            );
            self.cached = Some(CachedProjectSelection {
                editor_revision: editor.revision(),
                base_generation,
                scope,
                project: derived,
            });
            #[cfg(test)]
            {
                self.rebuild_count += 1;
            }
        } else {
            tracing::debug!(
                editor_revision = editor.revision().get(),
                document_count = editor.documents().len(),
                source_override_scope = ?scope,
                source_override_cache_hit = true,
                source_override_build_ms = 0_u128,
                "source override cache hit"
            );
        }

        Ok(())
    }

    /// Bind raw editor documents to source identities already present in the saved generation.
    fn capture_sources(
        base: &Project,
        editor: &EditorSnapshot,
    ) -> anyhow::Result<Vec<CapturedSource>> {
        let mut sources = BTreeMap::<PathBuf, CapturedSource>::new();

        for document in editor.documents() {
            let Some(source) = base
                .capture_known_source(document.source_path(), Arc::<str>::from(document.text()))
            else {
                tracing::trace!(
                    editor_path = %document.path().display(),
                    source_path = %document.source_path().display(),
                    "ignored open editor document outside the selected source generation"
                );
                continue;
            };

            if let Some(previous) = sources.get(source.path()) {
                if previous.text() != source.text() {
                    anyhow::bail!(
                        "editor paths mapped to source `{}` with conflicting text",
                        source.path().display()
                    );
                }
                continue;
            }
            sources.insert(source.path().to_path_buf(), source);
        }

        Ok(sources.into_values().collect())
    }
}

#[derive(Debug)]
struct CachedProjectSelection {
    editor_revision: rg_lsp_proto::EditorSnapshotRevision,
    base_generation: rg_project::ProjectGenerationId,
    scope: SourceOverrideScope,
    /// `None` means every applicable editor value already matched the saved project.
    project: Option<Project>,
}
