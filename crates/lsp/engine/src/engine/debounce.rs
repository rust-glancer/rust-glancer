use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

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
            }),
        }
    }

    pub(crate) fn call(&self, callback: impl FnOnce() + Send + 'static) {
        let generation = self.next_generation();
        let state = Arc::clone(&self.state);

        tokio::spawn(async move {
            tokio::time::sleep(state.delay).await;
            if state.generation.load(Ordering::Relaxed) == generation {
                callback();
            }
        });
    }

    pub(crate) fn call_now(&self, callback: impl FnOnce()) {
        self.next_generation();
        callback();
    }

    fn next_generation(&self) -> u64 {
        self.state
            .generation
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1)
    }
}

#[derive(Debug)]
struct DebounceState {
    delay: Duration,
    generation: AtomicU64,
}
