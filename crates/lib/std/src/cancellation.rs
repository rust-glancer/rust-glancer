//! Small cooperative-cancellation signal shared across synchronous engine layers.
//!
//! Request futures and semantic work live in different execution domains. Dropping the async
//! request therefore cannot unwind the synchronous analysis stack directly. A cloned token lets
//! the request owner mark that work obsolete while hot semantic loops decide where it is safe to
//! stop.

use std::{
    fmt,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

pub use rg_std_derive::cancelable;

/// Work that can check whether its owner still needs the result.
///
/// Query objects and builders delegate to the token or enclosing operation they already own.
/// Keeping the check here also allows an operation boundary to observe a closed response channel
/// before checking its token. Implementations must preserve the typed cancellation reason.
pub trait Cancelable {
    fn check_cancelled(&self, checkpoint: &'static str) -> Result<(), Cancelled>;
}

impl<T: Cancelable + ?Sized> Cancelable for &T {
    fn check_cancelled(&self, checkpoint: &'static str) -> Result<(), Cancelled> {
        T::check_cancelled(self, checkpoint)
    }
}

impl<T: Cancelable + ?Sized> Cancelable for &mut T {
    fn check_cancelled(&self, checkpoint: &'static str) -> Result<(), Cancelled> {
        T::check_cancelled(self, checkpoint)
    }
}

/// Check before the next unit of work, returning cancellation from the enclosing operation.
///
/// The source can be a token or any [`Cancelable`] object; it is borrowed and evaluated once.
/// An optional label describes the work being entered. Without one, the source location identifies
/// the checkpoint. The enclosing function or closure must return a `Result` whose error accepts
/// [`Cancelled`]. Cancellation stays typed when it enters a larger error, such as `OperationError`.
///
/// This macro only checks where it is written. Put it inside long-running loops and before result
/// publication, at points where stopping leaves shared state coherent.
///
/// ```
/// use rg_std::{CancellationToken, Cancelled};
///
/// fn collect_names(cancellation: &CancellationToken, names: &[&str]) -> Result<Vec<String>, Cancelled> {
///     let mut collected = Vec::new();
///     for name in names {
///         rg_std::check_cancel!(cancellation, "collect name");
///         collected.push((*name).to_owned());
///     }
///     Ok(collected)
/// }
/// ```
#[macro_export]
macro_rules! check_cancel {
    ($source:expr $(,)?) => {
        $crate::check_cancel!(
            $source,
            concat!(module_path!(), ":", line!(), ":", column!())
        )
    };
    ($source:expr, $checkpoint:expr $(,)?) => {
        if let ::core::result::Result::Err(cancelled) =
            $crate::Cancelable::check_cancelled(&$source, $checkpoint)
        {
            return ::core::result::Result::Err(::core::convert::From::from(cancelled));
        }
    };
}

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

impl Cancelable for CancellationToken {
    fn check_cancelled(&self, checkpoint: &'static str) -> Result<(), Cancelled> {
        if self.is_cancelled() {
            return Err(Cancelled { checkpoint });
        }
        Ok(())
    }
}

/// An operation stopped without producing a semantic result.
///
/// This is shared by builds and queries. Neither needs to know whether a dropped editor request,
/// a command-line owner, or another execution boundary cancelled its work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cancelled {
    checkpoint: &'static str,
}

impl Cancelled {
    pub fn checkpoint(&self) -> &'static str {
        self.checkpoint
    }
}

impl fmt::Display for Cancelled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "operation cancelled at {}", self.checkpoint)
    }
}

impl std::error::Error for Cancelled {}

/// An executing algorithm can stop even when its generic data source has no error to report.
/// Keeping the source error intact lets callers retain cache-recovery and diagnostic policy.
/// Source failures enter through `map_err(OperationError::Source)`; cancellation uses ordinary
/// `?` conversion. These conversions must stay distinct even when the source error is `Cancelled`.
#[derive(Debug)]
pub enum OperationError<E> {
    Source(E),
    Cancelled(Cancelled),
}

impl<E> From<Cancelled> for OperationError<E> {
    fn from(error: Cancelled) -> Self {
        Self::Cancelled(error)
    }
}

impl<E: fmt::Display> fmt::Display for OperationError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Source(error) => error.fmt(f),
            Self::Cancelled(error) => error.fmt(f),
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for OperationError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(match self {
            Self::Source(error) => error,
            Self::Cancelled(error) => error,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{Cancelable as _, CancellationToken};

    #[test]
    fn cancellation_is_shared_by_every_token_clone() {
        let token = CancellationToken::new();
        let worker = token.clone();

        token.cancel();

        assert!(worker.is_cancelled());
        assert_eq!(
            worker
                .check_cancelled("next file")
                .expect_err("owner cancelled")
                .checkpoint(),
            "next file",
        );
    }
}
