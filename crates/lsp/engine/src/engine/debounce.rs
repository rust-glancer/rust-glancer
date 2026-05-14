use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use tokio::task::JoinHandle;

/// Coalesces repeated triggers so only the latest delayed callback runs.
///
/// The counter is only used as a generation token, so relaxed ordering is enough: each callback
/// only needs to know whether another callback was scheduled after it.
#[derive(Debug, Clone)]
pub(crate) struct Debouncer {
    state: Arc<DebounceState>,
}

impl Debouncer {
    pub(crate) fn new(delay: Duration) -> Self {
        Self {
            state: Arc::new(DebounceState {
                delay,
                generation: AtomicU64::new(0),
                pending: Mutex::new(None),
            }),
        }
    }

    pub(crate) fn call(&self, callback: impl FnOnce() + Send + 'static) {
        let mut pending = self.pending();
        let generation = self.next_generation();
        let state = Arc::clone(&self.state);

        let handle = tokio::spawn(async move {
            tokio::time::sleep(state.delay).await;
            if state.generation.load(Ordering::Relaxed) == generation {
                callback();
            }
        });

        if let Some(previous) = pending.replace(handle) {
            previous.abort();
        }
    }

    pub(crate) fn call_now(&self, callback: impl FnOnce()) {
        let mut pending = self.pending();
        self.next_generation();
        if let Some(previous) = pending.take() {
            previous.abort();
        }
        drop(pending);

        callback();
    }

    fn next_generation(&self) -> u64 {
        self.state
            .generation
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1)
    }

    fn pending(&self) -> std::sync::MutexGuard<'_, Option<JoinHandle<()>>> {
        self.state
            .pending
            .lock()
            .expect("debounce pending-task mutex should not be poisoned")
    }
}

#[derive(Debug)]
struct DebounceState {
    delay: Duration,
    generation: AtomicU64,
    pending: Mutex<Option<JoinHandle<()>>>,
}
