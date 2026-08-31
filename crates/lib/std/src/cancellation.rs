//! Small cooperative-cancellation signal shared across synchronous engine layers.
//!
//! Request futures and semantic work live in different execution domains. Dropping the async
//! request therefore cannot unwind the synchronous analysis stack directly. A cloned token lets
//! the request owner mark that work obsolete while hot semantic loops decide where it is safe to
//! stop.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// Cloneable signal for work whose result is no longer needed.
#[derive(Clone, Debug, Default)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl CancellationToken {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mark this token and every clone as cancelled.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    /// Return whether the owner has made this work obsolete.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::CancellationToken;

    #[test]
    fn cancellation_is_shared_by_every_token_clone() {
        let token = CancellationToken::new();
        let worker = token.clone();

        token.cancel();

        assert!(worker.is_cancelled());
    }
}
