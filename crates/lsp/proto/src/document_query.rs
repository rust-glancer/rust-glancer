//! Tells the server when a document query had to fall back to saved information.
//!
//! Both coverage values still carry a normal feature result. `Partial` is useful for logging and
//! diagnostics: it means the query could read the current syntax, but could not rebuild every body
//! the feature normally uses.

use serde::{Deserialize, Serialize};

/// Whether a document result used all of the current editor information it normally needs.
///
/// The engine keeps details such as which body failed to rebuild. The server only needs this small
/// summary when it publishes the result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum DocumentQueryCoverage {
    /// Every required current body or syntax value came from the captured editor text.
    Exact,
    /// The query returned a best-effort result using current syntax and saved project data.
    Partial,
}

impl DocumentQueryCoverage {
    pub const fn is_partial(self) -> bool {
        matches!(self, Self::Partial)
    }
}

/// A document query result together with its current-editor coverage.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct DocumentQueryResult<T> {
    value: T,
    coverage: DocumentQueryCoverage,
}

impl<T> DocumentQueryResult<T> {
    pub fn new(value: T, coverage: DocumentQueryCoverage) -> Self {
        Self { value, coverage }
    }

    pub fn value(&self) -> &T {
        &self.value
    }

    pub const fn coverage(&self) -> DocumentQueryCoverage {
        self.coverage
    }

    pub fn into_value(self) -> T {
        self.value
    }
}
