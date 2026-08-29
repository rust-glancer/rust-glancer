//! Deferred-indexing reconciliation for the saved project generation.
//!
//! Early-start indexing leaves expensive Body IR for one detached project clone to finish. The
//! saved project remains queryable in parallel and may materialize the same package on demand.
//! Open-document packages are scheduled first inside the detached build and copied back as soon as
//! their resolution finishes; the same build continues through every ordinary package.
//!
//! All publication still re-enters the serialized engine queue. A source generation change makes
//! later copies stale, and the old clone must return before a replacement is detached. There is
//! therefore never more than one full background clone or a concurrent saved-project writer.

use std::{
    collections::BTreeSet,
    path::PathBuf,
    sync::{Arc, Mutex, mpsc::Sender},
    thread,
    time::{Duration, Instant},
};

use anyhow::Context as _;
use rg_def_map::PackageSlot;
use rg_lsp_proto::DeferredIndexingOutcome;
use rg_project::{DetachedSplitIndexing, SplitIndexingProgress};

use crate::engine::{
    QueuedEngineCommand,
    command::{DeferredIndexingResult, EngineCommand},
    project::ProjectState,
};

// Human-visible progress should feel live without turning every parallel package completion into
// an engine command and an RPC notification.
const PROGRESS_PUBLICATION_INTERVAL: Duration = Duration::from_millis(200);

/// Tracks the one detached indexing finish allowed to run beside the engine lane.
///
/// The background worker cannot mutate saved state. Its only external capabilities are receiving
/// best-effort path priorities and enqueueing package copies for generation-checked publication.
/// A newer saved generation can be announced while an older worker drains, so the client-visible
/// lifecycle generation is tracked separately from the generation that owns the worker.
#[derive(Debug)]
pub(super) struct DeferredIndexingFinish {
    sender: Sender<QueuedEngineCommand>,
    in_flight_generation: Option<u64>,
    worker_priority: Option<Arc<Mutex<Vec<PackageSlot>>>>,
    restart_after_in_flight: bool,
    /// Latest generation whose deferred lifecycle was announced but has not been terminated.
    active_lifecycle_generation: Option<u64>,
    priority_paths: BTreeSet<PathBuf>,
}

/// Terminal lifecycle event produced while reconciling one worker result.
///
/// The generation belongs to the latest announced lifecycle. It can differ from both the worker
/// that returned and the saved project that made that worker stale.
pub(super) struct DeferredIndexingTerminal {
    pub(super) generation: u64,
    pub(super) outcome: DeferredIndexingOutcome,
}

impl DeferredIndexingFinish {
    pub(super) fn new(sender: Sender<QueuedEngineCommand>) -> Self {
        Self {
            sender,
            in_flight_generation: None,
            worker_priority: None,
            restart_after_in_flight: false,
            active_lifecycle_generation: None,
            priority_paths: BTreeSet::new(),
        }
    }

    /// Ensure the current saved project will eventually finish deferred indexing.
    ///
    /// Starting another clone immediately would double peak memory. Record one restart and let the
    /// old build return first; its generation-tagged publications are harmless in the meantime.
    pub(super) fn saved_project_changed(&mut self, project: &ProjectState) -> bool {
        let should_announce_start = if self.in_flight_generation.is_some() {
            self.restart_after_in_flight = project.has_unfinished_split_indexing();
            self.restart_after_in_flight
        } else {
            self.start_current(project)
        };
        if should_announce_start {
            self.active_lifecycle_generation = Some(project.generation());
        }
        should_announce_start
    }

    /// Record whether an editor path should be scheduled ahead of ordinary background packages.
    ///
    /// The set survives source generations so an open document stays prioritized after a save.
    /// Workers consume complete package-priority snapshots between resolution jobs. Work already
    /// executing is never preempted, but a late didOpen still moves pending work to the front.
    pub(super) fn set_priority(
        &mut self,
        project: &ProjectState,
        path: PathBuf,
        prioritized: bool,
    ) {
        let changed = if prioritized {
            self.priority_paths.insert(path.clone())
        } else {
            self.priority_paths.remove(&path)
        };
        if !changed {
            return;
        }

        if self.in_flight_generation != Some(project.generation()) {
            return;
        }
        if let Some(worker_priority) = &self.worker_priority {
            let priorities = Self::package_priorities_for_paths(&self.priority_paths, |path| {
                project.package_slots_for_path(path)
            });
            tracing::debug!(
                generation = project.generation(),
                priority_package_count = priorities.len(),
                "deferred indexing package priority updated"
            );
            *worker_priority
                .lock()
                .expect("deferred indexing package priorities should not be poisoned") = priorities;
        }
    }

    /// Merge one priority package without ending the deferred-indexing lifecycle.
    pub(super) fn priority_package_returned(
        &mut self,
        project: &mut ProjectState,
        generation: u64,
        finished: rg_project::FinishedSplitIndexing,
    ) {
        if self.in_flight_generation != Some(generation) {
            tracing::info!(
                generation,
                current_in_flight_generation = ?self.in_flight_generation,
                "discarding unknown deferred indexing priority package"
            );
            return;
        }

        if let Err(error) =
            Self::apply_finished_if_current(project, generation, finished, "priority package")
        {
            tracing::warn!(
                generation,
                error = %format!("{error:#}"),
                "deferred indexing priority package could not merge into saved project"
            );
        }
    }

    /// Reconcile the final background result for the client-visible saved generation.
    ///
    /// `None` means this result cannot terminate the client's active operation because it is
    /// unknown or a replacement worker started. A current result remains terminal when its work
    /// failed. A stale result also becomes terminal when the latest saved generation is already
    /// complete and therefore needs no replacement worker.
    pub(super) fn finish_returned(
        &mut self,
        project: &mut ProjectState,
        generation: u64,
        result: DeferredIndexingResult,
    ) -> Option<DeferredIndexingTerminal> {
        if self.in_flight_generation != Some(generation) {
            tracing::info!(
                generation,
                current_in_flight_generation = ?self.in_flight_generation,
                "discarding unknown deferred indexing finish"
            );
            return None;
        }
        self.in_flight_generation = None;
        self.worker_priority = None;

        let worker_outcome = Self::apply_finish_if_current(project, generation, result);
        let should_restart = self.restart_after_in_flight || worker_outcome.is_none();
        self.restart_after_in_flight = false;
        if should_restart {
            if self.start_current(project) {
                return None;
            }

            // The latest generation may have completed while the older worker was running. Close
            // the latest announced lifecycle successfully when no replacement work remains.
            let outcome = if project.has_unfinished_split_indexing() {
                DeferredIndexingOutcome::Failed {
                    message: "deferred indexing could not start for the latest project generation"
                        .to_string(),
                }
            } else {
                DeferredIndexingOutcome::Succeeded
            };
            return self.finish_active_lifecycle(outcome);
        }

        worker_outcome.and_then(|outcome| self.finish_active_lifecycle(outcome))
    }

    fn finish_active_lifecycle(
        &mut self,
        outcome: DeferredIndexingOutcome,
    ) -> Option<DeferredIndexingTerminal> {
        let Some(generation) = self.active_lifecycle_generation.take() else {
            tracing::warn!("deferred indexing finished without an active client lifecycle");
            return None;
        };
        Some(DeferredIndexingTerminal {
            generation,
            outcome,
        })
    }

    fn start_current(&mut self, project: &ProjectState) -> bool {
        // Check the saved project before detaching it. Lower-memory package batches finish Body IR
        // before publication, and an empty worker would otherwise pay for a complete project clone.
        if !project.has_unfinished_split_indexing() {
            tracing::debug!(
                generation = project.generation(),
                "deferred indexing already complete"
            );
            return false;
        }

        let (generation, detached) = match project.detach_saved_split_indexing() {
            Ok(detached) => detached,
            Err(error) => {
                tracing::warn!(
                    error = %format!("{error:#}"),
                    "failed to detach saved project for deferred indexing finish"
                );
                return false;
            }
        };

        self.spawn_finish(generation, detached)
    }

    /// Finish deferred indexing on one detached project clone.
    fn spawn_finish(&mut self, generation: u64, detached: DetachedSplitIndexing) -> bool {
        let sender = self.sender.clone();
        let priority_packages = Self::package_priorities_for_paths(&self.priority_paths, |path| {
            detached.package_slots_for_path(path)
        });
        let worker_priority = Arc::new(Mutex::new(priority_packages));
        let background_priority = Arc::clone(&worker_priority);

        let spawn_result = thread::Builder::new()
            .name("rg-deferred-indexing".to_string())
            .spawn(move || {
                let started = Instant::now();
                tracing::info!(generation, "deferred indexing background finish started");

                let result = Self::finish_with_priorities(
                    &sender,
                    generation,
                    detached,
                    background_priority,
                    started,
                );
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
                self.worker_priority = Some(worker_priority);
                true
            }
            Err(error) => {
                tracing::warn!(
                    generation,
                    error = %error,
                    "failed to spawn deferred indexing background finish"
                );
                false
            }
        }
    }

    /// Run one Body IR build whose package queue keeps accepting editor priorities.
    fn finish_with_priorities(
        sender: &Sender<QueuedEngineCommand>,
        generation: u64,
        detached: DetachedSplitIndexing,
        priority_packages: Arc<Mutex<Vec<PackageSlot>>>,
        started: Instant,
    ) -> DeferredIndexingResult {
        let unfinished = detached.unfinished_packages();
        let priority_package_count = priority_packages
            .lock()
            .expect("deferred indexing package priorities should not be poisoned")
            .len();
        tracing::debug!(
            generation,
            pending_package_count = unfinished.len(),
            priority_package_count,
            "deferred indexing package schedule prepared"
        );

        let progress = DeferredIndexingProgressReporter::new(sender.clone(), generation);
        detached
            .finish_with_package_priority(
                || {
                    priority_packages
                        .lock()
                        .expect("deferred indexing package priorities should not be poisoned")
                        .clone()
                },
                |finished| {
                    tracing::debug!(
                        generation,
                        elapsed_ms = started.elapsed().as_millis(),
                        "deferred indexing priority package resolved"
                    );
                    let _ = sender.send(QueuedEngineCommand::new(
                        EngineCommand::DeferredIndexingPriorityPackageFinished {
                            generation,
                            finished: Box::new(finished),
                        },
                    ));
                },
                |snapshot| progress.report(snapshot),
            )
            .map(Box::new)
    }

    fn package_priorities_for_paths(
        paths: &BTreeSet<PathBuf>,
        mut packages_for_path: impl FnMut(&std::path::Path) -> anyhow::Result<Vec<PackageSlot>>,
    ) -> Vec<PackageSlot> {
        let mut packages = BTreeSet::new();
        for path in paths {
            match packages_for_path(path) {
                Ok(path_packages) => packages.extend(path_packages),
                Err(error) => {
                    tracing::trace!(
                        path = %path.display(),
                        error = %format!("{error:#}"),
                        "deferred indexing priority path is not in the saved source inventory"
                    );
                }
            }
        }
        packages.into_iter().collect()
    }

    /// Merge the final result if it still matches the saved-project generation.
    fn apply_finish_if_current(
        project: &mut ProjectState,
        generation: u64,
        result: DeferredIndexingResult,
    ) -> Option<DeferredIndexingOutcome> {
        if project.generation() != generation {
            tracing::info!(
                generation,
                current_generation = project.generation(),
                "discarding stale deferred indexing finish"
            );
            return None;
        }

        let outcome = match result {
            Ok(finished) => {
                match Self::apply_finished_if_current(project, generation, *finished, "finish") {
                    Ok(true) => DeferredIndexingOutcome::Succeeded,
                    // The generation was checked immediately above on the serialized engine lane, but
                    // retain the stale result in the type in case this helper's use changes later.
                    Ok(false) => return None,
                    Err(error) => {
                        let message = format!("{error:#}");
                        tracing::warn!(
                            generation,
                            error = %message,
                            "deferred indexing finish could not merge into saved project"
                        );
                        DeferredIndexingOutcome::Failed { message }
                    }
                }
            }
            Err(error) => {
                let message = format!("{error:#}");
                tracing::warn!(
                    generation,
                    error = %message,
                    "deferred indexing finish did not update project"
                );
                DeferredIndexingOutcome::Failed { message }
            }
        };
        Some(outcome)
    }

    fn apply_finished_if_current(
        project: &mut ProjectState,
        generation: u64,
        finished: rg_project::FinishedSplitIndexing,
        label: &'static str,
    ) -> anyhow::Result<bool> {
        if project.generation() != generation {
            tracing::info!(
                generation,
                current_generation = project.generation(),
                label,
                "discarding stale deferred indexing publication"
            );
            return Ok(false);
        }

        let updated = project
            .mutate_saved_preserving_generation(|saved| {
                saved.split_indexing().merge_finished(finished)
            })
            .with_context(|| format!("merge deferred indexing {label}"))?;
        if !updated {
            tracing::trace!(
                generation,
                label,
                "deferred indexing publication completed without saved project changes"
            );
        }
        Ok(true)
    }
}

/// Reduces parallel package completions to a small ordered stream on the engine lane.
///
/// Reporting stays synchronous and non-blocking from the Body IR workers' point of view: an
/// accepted snapshot only enters the existing engine command queue. Stage transitions and exact
/// terminal counts bypass the cadence so the client never misses a meaningful boundary.
#[derive(Debug)]
struct DeferredIndexingProgressReporter {
    sender: Sender<QueuedEngineCommand>,
    generation: u64,
    publication: Mutex<ProgressPublication>,
}

#[derive(Debug, Default)]
struct ProgressPublication {
    last_progress: Option<SplitIndexingProgress>,
    last_published_at: Option<Instant>,
}

impl DeferredIndexingProgressReporter {
    fn new(sender: Sender<QueuedEngineCommand>, generation: u64) -> Self {
        Self {
            sender,
            generation,
            publication: Mutex::new(ProgressPublication::default()),
        }
    }

    fn report(&self, progress: SplitIndexingProgress) {
        debug_assert!(progress.completed_packages() <= progress.total_packages());

        let now = Instant::now();
        let mut publication = self
            .publication
            .lock()
            .expect("deferred indexing progress publication should not be poisoned");
        if let Some(previous) = publication.last_progress
            && previous.stage() == progress.stage()
            && previous.completed_packages() >= progress.completed_packages()
        {
            // The atomic counter gives every worker a newer count, but a worker can be
            // descheduled before it invokes this callback. Do not let that delayed callback move
            // an editor from, for example, 18/40 back to 17/40.
            return;
        }

        let stage_changed = publication
            .last_progress
            .is_none_or(|previous| previous.stage() != progress.stage());
        let stage_finished = progress.completed_packages() == progress.total_packages();
        let cadence_elapsed = publication
            .last_published_at
            .is_none_or(|last_published_at| {
                now.duration_since(last_published_at) >= PROGRESS_PUBLICATION_INTERVAL
            });
        if !stage_changed && !stage_finished && !cadence_elapsed {
            return;
        }

        publication.last_progress = Some(progress);
        publication.last_published_at = Some(now);
        drop(publication);

        let command = EngineCommand::DeferredIndexingProgress {
            generation: self.generation,
            progress,
        };
        if self.sender.send(QueuedEngineCommand::new(command)).is_err() {
            tracing::debug!(
                generation = self.generation,
                "failed to enqueue deferred indexing progress"
            );
        }
    }
}
