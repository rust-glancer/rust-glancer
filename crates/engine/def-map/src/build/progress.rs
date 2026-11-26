//! Rate-limited checkpoints for crate scope construction.

use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

pub(super) const LOG_TARGET: &str = "rg_def_map::progress";
const REPORT_INTERVAL: Duration = Duration::from_secs(5);

// Multiple crates can build concurrently in one workspace engine. One shared budget bounds
// their combined output even when every worker is slow.
static LAST_REPORT: Mutex<Option<Instant>> = Mutex::new(None);

pub(crate) struct ResolutionProgress {
    started: Instant,
    next_report: Instant,
}

impl ResolutionProgress {
    pub(super) fn start() -> Option<Self> {
        tracing::enabled!(target: LOG_TARGET, tracing::Level::DEBUG).then(|| {
            let started = Instant::now();
            Self {
                started,
                next_report: started + REPORT_INTERVAL,
            }
        })
    }

    pub(super) fn elapsed_at_checkpoint(&mut self) -> Option<Duration> {
        let now = Instant::now();
        if now < self.next_report {
            return None;
        }
        self.next_report = now + REPORT_INTERVAL;
        let mut last = LAST_REPORT
            .lock()
            .expect("scope construction progress budget");
        if last.is_some_and(|last| now.saturating_duration_since(last) < REPORT_INTERVAL) {
            return None;
        }
        *last = Some(now);
        Some(now.duration_since(self.started))
    }
}
