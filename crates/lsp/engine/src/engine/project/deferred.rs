//! Deferred-indexing reconciliation for the saved project generation.
//!
//! Early-start indexing leaves some expensive payloads for a detached project clone to finish.
//! The saved project remains queryable in parallel and may even materialize the same packages on
//! demand. This module keeps at most one clone in flight, sends its result back through the engine
//! queue, and merges only package-wise improvements that still belong to the saved generation.
//!
//! If source changes while a clone is running, the old clone is allowed to return before a new one
//! starts. That avoids holding two full detached projects at once.
//!
//! Example: a clone for generation 4 is running when a saved file publishes generation 5. The
//! generation-4 result is discarded when it returns, and only then is a generation-5 clone
//! started.

use std::{sync::mpsc::Sender, thread, time::Instant};

use rg_project::DetachedSplitIndexing;

use crate::engine::{
    QueuedEngineCommand,
    command::{DeferredIndexingResult, EngineCommand},
    project::ProjectState,
};

/// Tracks the one detached indexing finish allowed to run beside the engine lane.
///
/// The sender is owned here because background completion is this subsystem's only external
/// capability. It cannot mutate project state directly; it can only enqueue a result for the
/// coordinator to inspect.
#[derive(Debug)]
pub(super) struct DeferredIndexingFinish {
    sender: Sender<QueuedEngineCommand>,
    in_flight_generation: Option<u64>,
    restart_after_in_flight: bool,
}

impl DeferredIndexingFinish {
    pub(super) fn new(sender: Sender<QueuedEngineCommand>) -> Self {
        Self {
            sender,
            in_flight_generation: None,
            restart_after_in_flight: false,
        }
    }

    /// Start deferred indexing for the freshly saved project.
    ///
    /// This is called after the coordinator replaces the saved project during initialization. At
    /// that point there cannot be an older finish for the same engine, so the clone built by the
    /// initial index can be handed directly to the background thread. The return value tells the
    /// coordinator whether it should publish a deferred-started notification.
    pub(super) fn start_initial(
        &mut self,
        generation: u64,
        detached: DetachedSplitIndexing,
    ) -> bool {
        self.spawn_finish(generation, detached)
    }

    /// Ensure the current saved project will eventually finish deferred indexing.
    ///
    /// Saved-source mutations invalidate any detached clone that is already running. Starting
    /// another clone immediately would double peak memory, so this state records that one restart
    /// is needed and lets the old clone return first. Returning `true` means the active generation
    /// now has deferred work running or queued behind that older clone.
    pub(super) fn saved_project_changed(&mut self, project: &ProjectState) -> bool {
        if self.in_flight_generation.is_some() {
            self.restart_after_in_flight = true;
            return true;
        }
        self.start_current(project)
    }

    /// Reconcile one returned clone and decide whether the editor may see indexing as finished.
    ///
    /// An unknown result is ignored. A known result is merged only if its generation still matches
    /// saved source, then any restart requested by an intervening source change is launched. The
    /// return value is `true` only for a finish belonging to the active generation.
    pub(super) fn finish_returned(
        &mut self,
        project: &mut ProjectState,
        generation: u64,
        result: DeferredIndexingResult,
    ) -> bool {
        // Only the clone recorded as in flight is allowed to change this controller's state.
        if self.in_flight_generation != Some(generation) {
            tracing::info!(
                generation,
                current_in_flight_generation = ?self.in_flight_generation,
                "discarding unknown deferred indexing finish"
            );
            return false;
        }
        self.in_flight_generation = None;

        // The clone is no longer live even if its generation became stale while it ran.
        let is_current_generation = Self::apply_finish_if_current(project, generation, result);
        let should_restart = self.restart_after_in_flight || !is_current_generation;
        self.restart_after_in_flight = false;
        // Wait until after the old clone returns before allocating its replacement.
        if should_restart {
            self.start_current(project);
        }

        is_current_generation
    }

    /// Detach the latest saved state when no other background clone is live.
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

    /// Finish deferred indexing on a detached project clone.
    ///
    /// The saved project is already usable when this runs. The background result is sent back to
    /// the command loop instead of mutating saved state directly, so the command loop can keep all
    /// project generation checks in one place.
    fn spawn_finish(&mut self, generation: u64, detached: DetachedSplitIndexing) -> bool {
        let sender = self.sender.clone();

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

    /// Merge a background finish if it still matches the saved-project generation.
    ///
    /// Returning `true` means that the background finish belongs to the current saved project, even
    /// if there was nothing left to merge. The client-side status indicator cares about that
    /// lifecycle fact: deferred indexing is no longer pending once this command has been handled.
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

        let updated = match result {
            // The merge itself is monotonic: packages finished by query-time materialization win
            // over an equal or older package from the background clone.
            Ok(finished) => project
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
