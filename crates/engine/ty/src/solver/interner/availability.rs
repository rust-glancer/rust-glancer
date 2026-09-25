//! Track missing information when asking the compiler trait solver a question.
//!
//! To solve a goal such as `Widget: Render`, the compiler solver calls back into Rust Glancer for
//! impl headers, bounds, generic parameters, and associated types. These are the callbacks we
//! track here. Some must return a value even when we cannot provide the requested information:
//! for example, a missing declaration's generic metadata is returned as an empty parameter list.
//!
//! Such fallback values let the compiler code keep running, but its answer may depend on them.
//! We therefore record missing information separately from the solver's answer. The caller checks
//! that record before accepting a result, including a result saying that no impl applies.

use std::cell::Cell;

use super::SolverInterner;

/// Shared reporting state for the declaration callbacks reached through one interner.
/// Each query remembers its own starting count in a `CallbackScope`, so sharing this storage
/// between candidate tables does not make a failed candidate affect the next one.
#[derive(Default)]
pub(crate) struct CallbackAvailability {
    failures: Cell<u64>,
    reason: Cell<Option<&'static str>>,
    // Callback and cache code has the interner, but not the caller's scope guard. This tells
    // `has_unavailable` which query's starting count to compare against.
    active_start: Cell<Option<u64>>,
}

/// Check whether one query had the information it needed, from its first declaration read
/// through deciding whether to keep the result.
///
/// For example, matching an impl starts a scope before loading its header. If that read reports
/// missing data, `failure()` lets the caller reject the match or mark it unavailable. This works
/// even if matching returns before it has any trait goals to evaluate.
///
/// Scopes can nest: reading a header can itself require reading another declaration. Each scope
/// remembers the failure count at entry, and leaving a scope never resets that count:
///
/// ```text
/// candidate A starts at 0
///   a declaration read also starts at 0; missing data advances the count to 1
///   the read returns; candidate A still sees that 1 differs from its starting 0
/// candidate A returns unavailable and its scope ends
/// candidate B starts at 1; if its reads succeed, the count stays at 1
/// ```
///
/// The guard handles callback tracking and clears cached solver answers after a failure. It does
/// not undo assignments such as `?T = u8`. A caller that changes inference variables also needs
/// an inference snapshot or a separate candidate table, which it can discard on failure.
#[must_use]
pub struct CallbackScope<'s> {
    cx: SolverInterner<'s>,
    start: u64,
    parent: Option<u64>,
}

impl<'s> SolverInterner<'s> {
    /// Include input preparation in the scope: lowering a type or loading an impl can fail before
    /// goal evaluation starts. Keep guards nested, dropping an inner scope before its caller's.
    pub fn track_callbacks(self) -> CallbackScope<'s> {
        let callbacks = &self.0.callbacks;
        let start = callbacks.failures.get();
        CallbackScope {
            cx: self,
            start,
            parent: callbacks.active_start.replace(Some(start)),
        }
    }

    /// Report missing information before returning fallback data to the compiler. This makes
    /// every enclosing scope observe the failure, including callers of a nested declaration read.
    pub(crate) fn unavailable(self, reason: &'static str) {
        let callbacks = &self.0.callbacks;
        callbacks.failures.set(callbacks.failures.get() + 1);
        callbacks.reason.set(Some(reason));
        self.profile(|p| *p.unavailable.entry(reason).or_default() += 1);
    }

    /// Check the innermost active scope when only the interner is available. Declaration loading
    /// uses this to decide whether its result is complete enough to cache.
    pub(crate) fn has_unavailable(self) -> bool {
        let callbacks = &self.0.callbacks;
        callbacks
            .active_start
            .get()
            .is_some_and(|start| callbacks.failures.get() != start)
    }
}

impl CallbackScope<'_> {
    /// Return the last reason reported for missing data in this scope, including nested scopes.
    /// `None` means no callback reported missing data; the solver may still reject the goal.
    pub fn failure(&self) -> Option<&'static str> {
        let callbacks = &self.cx.0.callbacks;
        (callbacks.failures.get() != self.start).then(|| {
            callbacks
                .reason
                .get()
                .expect("a failed callback records its reason")
        })
    }
}

impl Drop for CallbackScope<'_> {
    fn drop(&mut self) {
        // Resume checking the caller's scope. Leave the failure count alone: that caller may
        // need to reject its own result because of a declaration read which failed inside us.
        self.cx.0.callbacks.active_start.set(self.parent);
        if self.failure().is_some() {
            // The compiler may cache an answer before our caller checks callback availability.
            // Reusing it would skip the failed callback and make the next attempt look complete.
            // We cannot identify which answers depended on that read, so clear the solver answer
            // cache. Declaration reads check completeness before caching their source data.
            *self.cx.0.cache.borrow_mut() = Default::default();
        }
    }
}
