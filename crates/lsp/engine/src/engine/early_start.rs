//! Early-start orchestration for the LSP engine.
//!
//! The command worker decides which LSP query is running. This module owns the split-indexing
//! lifecycle around that command flow: finish deferred indexing in the background, reconcile it
//! with the current saved-project generation, and materialize the saved analysis surface a query
//! needs before semantic analysis can produce false negatives.

use std::{path::Path, sync::mpsc::Sender, thread, time::Instant};

use anyhow::Context as _;
use rg_analysis::{ReferenceQuery, ReferenceSearchFile};
use rg_ir_model::TargetRef;
use rg_project::{AnalysisSurface, DetachedSplitIndexing, FileContext};
use rg_std::UniqueVec;

use super::{
    QueuedEngineCommand,
    command::{DeferredIndexingResult, EngineCommand},
    project_proxy::ProjectProxy,
};

#[derive(Debug, Clone)]
pub(super) struct ReferenceSearchPlan {
    targets: Vec<TargetRef>,
    files: Option<Vec<ReferenceSearchFile>>,
}

impl ReferenceSearchPlan {
    pub(super) fn new(targets: Vec<TargetRef>, files: Option<Vec<ReferenceSearchFile>>) -> Self {
        Self { targets, files }
    }

    pub(super) fn query(&self, include_declaration: bool) -> ReferenceQuery<'_> {
        match self.files.as_deref() {
            Some(files) => ReferenceQuery::find_references_in_files(files, include_declaration),
            None => ReferenceQuery::find_references(&self.targets, include_declaration),
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct DeferredIndexingFinish {
    in_flight_generation: Option<u64>,
    restart_after_in_flight: bool,
}

impl DeferredIndexingFinish {
    pub(super) fn new() -> Self {
        Self::default()
    }

    /// Start deferred indexing for the freshly saved project.
    ///
    /// This is called after the worker replaces the saved project during initialization. At that
    /// point there cannot be an older finish for the same engine, so the clone built by the initial
    /// index can be handed directly to the background thread.
    pub(super) fn start_initial(
        &mut self,
        sender: &Sender<QueuedEngineCommand>,
        generation: u64,
        detached: DetachedSplitIndexing,
    ) {
        self.spawn_finish(sender, generation, detached);
    }

    /// Ensure the current saved project will eventually finish deferred indexing.
    ///
    /// Saved-source mutations invalidate any detached clone that is already running. Starting
    /// another clone immediately would double peak memory, so the worker records that one restart is
    /// needed and lets the old clone return first.
    pub(super) fn saved_project_changed(
        &mut self,
        sender: &Sender<QueuedEngineCommand>,
        project_proxy: &ProjectProxy,
    ) {
        if self.in_flight_generation.is_some() {
            self.restart_after_in_flight = true;
            return;
        }
        self.start_current(sender, project_proxy);
    }

    /// Handle a background result and return whether the editor should see this root as finished.
    pub(super) fn finish_returned(
        &mut self,
        sender: &Sender<QueuedEngineCommand>,
        project_proxy: &mut ProjectProxy,
        generation: u64,
        result: DeferredIndexingResult,
    ) -> bool {
        if self.in_flight_generation != Some(generation) {
            tracing::info!(
                generation,
                current_in_flight_generation = ?self.in_flight_generation,
                "discarding unknown deferred indexing finish"
            );
            return false;
        }
        self.in_flight_generation = None;

        let is_current_generation =
            Self::apply_finish_if_current(project_proxy, generation, result);
        let should_restart = self.restart_after_in_flight || !is_current_generation;
        self.restart_after_in_flight = false;
        if should_restart {
            self.start_current(sender, project_proxy);
        }

        is_current_generation
    }

    fn start_current(
        &mut self,
        sender: &Sender<QueuedEngineCommand>,
        project_proxy: &ProjectProxy,
    ) {
        let (generation, detached) = match project_proxy.detach_saved_split_indexing() {
            Ok(detached) => detached,
            Err(error) => {
                tracing::warn!(
                    error = %format!("{error:#}"),
                    "failed to detach saved project for deferred indexing finish"
                );
                return;
            }
        };

        self.spawn_finish(sender, generation, detached);
    }

    /// Finish deferred indexing on a detached project clone.
    ///
    /// The saved project is already usable when this runs. The background result is sent back to
    /// the command loop instead of mutating saved state directly, so the command loop can keep all
    /// project generation checks in one place.
    fn spawn_finish(
        &mut self,
        sender: &Sender<QueuedEngineCommand>,
        generation: u64,
        detached: DetachedSplitIndexing,
    ) {
        let sender = sender.clone();

        let spawn_result = thread::Builder::new()
            .name("rg-deferred-indexing".to_string())
            .spawn(move || {
                let started = Instant::now();
                tracing::info!(generation, "deferred indexing background finish started");

                // Finish against the clone. The result still owns that clone, which lets the saved
                // project later merge only package-wise improvements.
                let result = detached.finish().map(Box::new);
                let elapsed_ms = started.elapsed().as_millis();
                match &result {
                    Ok(_) => {
                        tracing::info!(
                            generation,
                            elapsed_ms,
                            "deferred indexing background finish completed"
                        );
                    }
                    Err(error) => {
                        tracing::warn!(
                            generation,
                            elapsed_ms,
                            error = %format!("{error:#}"),
                            "deferred indexing background finish failed"
                        );
                    }
                }

                let _ = sender.send(QueuedEngineCommand::new(
                    EngineCommand::DeferredIndexingFinished { generation, result },
                ));
            });

        match spawn_result {
            Ok(_) => {
                self.in_flight_generation = Some(generation);
            }
            Err(error) => {
                tracing::warn!(
                    generation,
                    error = %error,
                    "failed to spawn deferred indexing background finish"
                );
            }
        }
    }

    /// Merge the initial background finish if it still matches the saved-project generation.
    ///
    /// Returning `true` means that the background finish belongs to the current saved project, even
    /// if there was nothing left to merge. The client-side status indicator cares about that
    /// lifecycle fact: deferred indexing is no longer pending once this command has been handled.
    fn apply_finish_if_current(
        project_proxy: &mut ProjectProxy,
        generation: u64,
        result: DeferredIndexingResult,
    ) -> bool {
        if project_proxy.generation() != generation {
            tracing::info!(
                generation,
                current_generation = project_proxy.generation(),
                "discarding stale deferred indexing finish"
            );
            return false;
        }

        let updated = match result {
            // The merge itself is monotonic: packages finished by query-time materialization win
            // over an equal or older package from the background clone.
            Ok(finished) => project_proxy
                .mutate_saved_preserving_generation(|saved| {
                    saved.split_indexing().merge_finished(*finished)
                })
                .unwrap_or_else(|error| {
                    tracing::warn!(
                        generation,
                        error = %format!("{error:#}"),
                        "deferred indexing finish could not merge into saved project"
                    );
                    false
                }),
            Err(error) => {
                tracing::warn!(
                    generation,
                    error = %format!("{error:#}"),
                    "deferred indexing finish did not update project"
                );
                false
            }
        };

        if !updated {
            tracing::trace!(
                generation,
                "deferred indexing finish completed without saved project changes"
            );
        }
        true
    }
}

pub(super) struct EarlyStart;

impl EarlyStart {
    /// Materialize the saved analysis surface needed by one path-local query.
    pub(super) fn ensure_path(
        project_proxy: &mut ProjectProxy,
        query: &'static str,
        path: &Path,
    ) -> anyhow::Result<()> {
        let started = Instant::now();

        // Resolve the path before mutating the saved project. A single file can have several target
        // contexts, but file-shaped materialization only needs the package-local file ids.
        let files = {
            let snapshot = project_proxy.saved_snapshot()?;
            Self::file_contexts(snapshot, path)?
                .into_iter()
                .map(|context| (context.package, context.file))
                .collect::<Vec<_>>()
        };
        project_proxy
            .mutate_saved_preserving_generation(|project| {
                project
                    .split_indexing()
                    .materialize(AnalysisSurface::Files(&files))
            })
            .with_context(|| {
                format!("while attempting to prepare analysis for {query} query path")
            })?;
        tracing::trace!(
            query,
            file_count = files.len(),
            elapsed_ms = started.elapsed().as_millis(),
            "saved analysis surface prepared for query path"
        );
        Ok(())
    }

    /// Materialize the saved analysis surface needed by reference-like scans.
    ///
    /// Text prefiltering can narrow some work to exact files, while other plans still need whole
    /// target/package coverage. Preserve both shapes so split indexing only does the work the query
    /// planner proved necessary.
    pub(super) fn ensure_reference_plans(
        project_proxy: &mut ProjectProxy,
        query: &'static str,
        plans: &[ReferenceSearchPlan],
    ) -> anyhow::Result<()> {
        let started = Instant::now();
        let mut files = UniqueVec::<(rg_def_map::PackageSlot, rg_parse::FileId)>::new();
        let mut targets = UniqueVec::<TargetRef>::new();

        // `ReferenceSearchPlan::files == None` means "scan the targets normally". `Some(files)`
        // means the text prefilter already selected the exact source files worth materializing.
        for plan in plans {
            match &plan.files {
                Some(plan_files) => {
                    files.extend(
                        plan_files
                            .iter()
                            .map(|file| (file.target.package, file.file_id)),
                    );
                }
                None => targets.extend(plan.targets.iter().copied()),
            }
        }

        let files = files.into_vec();
        let targets = targets.into_vec();
        project_proxy
            .mutate_saved_preserving_generation(|project| {
                project
                    .split_indexing()
                    .materialize(AnalysisSurface::FilesAndTargets {
                        files: &files,
                        targets: &targets,
                    })
            })
            .with_context(|| {
                format!("while attempting to prepare analysis for {query} reference scan")
            })?;
        tracing::trace!(
            query,
            file_count = files.len(),
            target_count = targets.len(),
            elapsed_ms = started.elapsed().as_millis(),
            "saved analysis surface prepared for reference scan"
        );
        Ok(())
    }

    /// Return saved file contexts for an existing query path.
    fn file_contexts(
        snapshot: rg_project::ProjectSnapshot<'_>,
        path: &Path,
    ) -> anyhow::Result<Vec<FileContext>> {
        if !path.exists() {
            tracing::debug!(path = %path.display(), "query path does not exist");
            return Ok(Vec::new());
        }

        snapshot.file_contexts_for_path(path)
    }
}
