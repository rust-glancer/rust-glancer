//! Periodic progress while workers are demanding saved declaration queries.

use std::{
    collections::HashMap,
    sync::{Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

use rg_ir_model::CrateRef;
use rg_workspace::WorkspaceMetadata;

pub(super) const LOG_TARGET: &str = "rg_project::indexing";
const REPORT_INTERVAL: Duration = Duration::from_secs(5);
const TARGET_SAMPLE_SIZE: usize = 3;

#[derive(Default)]
pub(super) struct IndexingProgress(Mutex<TargetProgress>);

#[derive(Default)]
struct TargetProgress {
    completed: usize,
    active: HashMap<CrateRef, (Instant, &'static str)>,
}

impl IndexingProgress {
    /// The reporter waits independently of the query workers so it can show the active targets
    /// even if every worker is blocked. Dropping the sender wakes it before the scope joins.
    pub(super) fn report_while<T>(
        &self,
        workspace: &WorkspaceMetadata,
        total: usize,
        work: impl FnOnce() -> T,
    ) -> T {
        thread::scope(|scope| {
            let (stop, receiver) = mpsc::channel::<()>();
            let dispatcher = tracing::dispatcher::get_default(Clone::clone);
            let span = tracing::Span::current();
            let started = Instant::now();
            let reporter = thread::Builder::new().name("rg-index-progress".to_owned());
            if let Err(error) = reporter.spawn_scoped(scope, move || {
                tracing::dispatcher::with_default(&dispatcher, || {
                    let _entered = span.enter();
                    while receiver.recv_timeout(REPORT_INTERVAL) == Err(mpsc::RecvTimeoutError::Timeout)
                    {
                        let (completed, mut active) = {
                            let state = self.0.lock().expect("indexing progress state");
                            let active = state.active.iter().map(|(&target, &work)| (target, work));
                            (state.completed, active.collect::<Vec<_>>())
                        };
                        active.sort_unstable_by_key(|(_, (started, _))| *started);
                        let oldest_targets = active
                            .iter()
                            .take(TARGET_SAMPLE_SIZE)
                            .map(|&(target, (started, phase))| {
                                let name = workspace.target(target).map_or("<unknown>", |target| target.name.as_str());
                                format!(
                                    "{name} (package_slot={}, crate_id={}, phase={phase}, elapsed_ms={})",
                                    target.package.0, target.crate_id.0, started.elapsed().as_millis()
                                )
                            })
                            .collect::<Vec<_>>();
                        tracing::debug!(
                            target: LOG_TARGET,
                            elapsed_ms = started.elapsed().as_millis(),
                            completed_targets = completed,
                            total_targets = total,
                            active_targets = active.len(),
                            ?oldest_targets,
                            "saved declaration indexing is still running"
                        );
                    }
                });
            }) {
                tracing::debug!(target: LOG_TARGET, %error, "could not start indexing progress reporter");
            }
            let result = work();
            drop(stop);
            result
        })
    }

    pub(super) fn start_target(&self, target: CrateRef) {
        self.0
            .lock()
            .expect("indexing progress state")
            .active
            .insert(target, (Instant::now(), "declarations"));
    }

    pub(super) fn prepare_lang_items(&self, target: CrateRef) {
        if let Some((_, phase)) = self
            .0
            .lock()
            .expect("indexing progress state")
            .active
            .get_mut(&target)
        {
            *phase = "language items";
        }
    }

    pub(super) fn finish_target(&self, target: CrateRef, completed: bool) {
        let mut state = self.0.lock().expect("indexing progress state");
        state.active.remove(&target);
        state.completed += usize::from(completed);
    }
}
