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
    time::Instant,
};

use rg_def_map::PackageSlot;
use rg_project::DetachedSplitIndexing;

use crate::engine::{
    QueuedEngineCommand,
    command::{DeferredIndexingResult, EngineCommand},
    project::ProjectState,
};

/// Tracks the one detached indexing finish allowed to run beside the engine lane.
///
/// The background worker cannot mutate saved state. Its only external capabilities are receiving
/// best-effort path priorities and enqueueing package copies for generation-checked publication.
#[derive(Debug)]
pub(super) struct DeferredIndexingFinish {
    sender: Sender<QueuedEngineCommand>,
    in_flight_generation: Option<u64>,
    worker_priority: Option<Arc<Mutex<Vec<PackageSlot>>>>,
    restart_after_in_flight: bool,
    priority_paths: BTreeSet<PathBuf>,
}

impl DeferredIndexingFinish {
    pub(super) fn new(sender: Sender<QueuedEngineCommand>) -> Self {
        Self {
            sender,
            in_flight_generation: None,
            worker_priority: None,
            restart_after_in_flight: false,
            priority_paths: BTreeSet::new(),
        }
    }

    /// Start deferred indexing for the freshly saved project.
    pub(super) fn start_initial(
        &mut self,
        generation: u64,
        detached: DetachedSplitIndexing,
    ) -> bool {
        self.spawn_finish(generation, detached)
    }

    /// Ensure the current saved project will eventually finish deferred indexing.
    ///
    /// Starting another clone immediately would double peak memory. Record one restart and let the
    /// old build return first; its generation-tagged publications are harmless in the meantime.
    pub(super) fn saved_project_changed(&mut self, project: &ProjectState) -> bool {
        if self.in_flight_generation.is_some() {
            self.restart_after_in_flight = true;
            return true;
        }
        self.start_current(project)
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

        Self::apply_finished_if_current(project, generation, finished, "priority package");
    }

    /// Reconcile the final background result and decide whether indexing is finished for the client.
    pub(super) fn finish_returned(
        &mut self,
        project: &mut ProjectState,
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
        self.worker_priority = None;

        let is_current_generation = Self::apply_finish_if_current(project, generation, result);
        let should_restart = self.restart_after_in_flight || !is_current_generation;
        self.restart_after_in_flight = false;
        if should_restart {
            self.start_current(project);
        }

        is_current_generation
    }

    fn start_current(&mut self, project: &ProjectState) -> bool {
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
    ) -> bool {
        if project.generation() != generation {
            tracing::info!(
                generation,
                current_generation = project.generation(),
                "discarding stale deferred indexing finish"
            );
            return false;
        }

        match result {
            Ok(finished) => {
                Self::apply_finished_if_current(project, generation, *finished, "finish");
            }
            Err(error) => {
                tracing::warn!(
                    generation,
                    error = %format!("{error:#}"),
                    "deferred indexing finish did not update project"
                );
            }
        }
        true
    }

    fn apply_finished_if_current(
        project: &mut ProjectState,
        generation: u64,
        finished: rg_project::FinishedSplitIndexing,
        label: &'static str,
    ) -> bool {
        if project.generation() != generation {
            tracing::info!(
                generation,
                current_generation = project.generation(),
                label,
                "discarding stale deferred indexing publication"
            );
            return false;
        }

        let updated = project
            .mutate_saved_preserving_generation(|saved| {
                saved.split_indexing().merge_finished(finished)
            })
            .unwrap_or_else(|error| {
                tracing::warn!(
                    generation,
                    label,
                    error = %format!("{error:#}"),
                    "deferred indexing publication could not merge into saved project"
                );
                false
            });
        if !updated {
            tracing::trace!(
                generation,
                label,
                "deferred indexing publication completed without saved project changes"
            );
        }
        true
    }
}
