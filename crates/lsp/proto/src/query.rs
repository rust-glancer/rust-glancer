//! Result of one query across the server/engine boundary.
//!
//! A successful document query carries the editor ids used to compute its value. The server checks
//! those ids against live editor state before it publishes the value. Expected operational stops
//! and real engine failures remain distinct errors, so neither can be mistaken for a valid empty
//! feature result.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{EngineError, OpenDocumentsRevision, TargetDocumentRevision};

/// One query value together with the editor state that must still match at publication time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryValue<T> {
    value: T,
    scope: QueryScope,
}

impl<T> QueryValue<T> {
    pub fn new(value: T, scope: QueryScope) -> Self {
        Self { value, scope }
    }

    pub fn value(&self) -> &T {
        &self.value
    }

    pub fn scope(&self) -> &QueryScope {
        &self.scope
    }

    pub fn into_value(self) -> T {
        self.value
    }

    pub fn into_parts(self) -> (T, QueryScope) {
        (self.value, self.scope)
    }
}

/// Editor state that must remain unchanged before a query value can be published.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum QueryScope {
    /// The query depends only on the serialized saved project.
    SavedProject,
    /// The query depends on one captured editor document.
    TargetDocument(TargetDocumentRevision),
    /// The query may return saved ranges from any captured open document.
    GlobalOperation {
        target: TargetDocumentRevision,
        open_documents_revision: OpenDocumentsRevision,
    },
}

/// Why a query has no publishable feature value.
///
/// The engine reports saved-source races, temporary resource failures, save requirements, and
/// internal failures. `EditorChanged` is added by the server after a successful engine response no
/// longer matches live ingress state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum QueryError {
    /// Disk no longer matches the saved project used by the request.
    #[error("saved source changed while analysis was running")]
    SavedSourceChanged,
    /// A recoverable analysis resource cannot be used yet.
    #[error("analysis input is temporarily unavailable")]
    TemporarilyUnavailable,
    #[error("analysis request was superseded by newer editor state")]
    EditorChanged,
    #[error("save `{path}` before running this operation")]
    SaveRequired { path: PathBuf },
    #[error(transparent)]
    Internal(EngineError),
}

#[cfg(test)]
mod tests {
    use super::{QueryError, QueryScope, QueryValue};

    #[test]
    fn valid_empty_and_retryable_error_have_distinct_wire_shapes() {
        let success: Result<QueryValue<Vec<usize>>, QueryError> =
            Ok(QueryValue::new(Vec::new(), QueryScope::SavedProject));
        let failed: Result<QueryValue<Vec<usize>>, QueryError> =
            Err(QueryError::SavedSourceChanged);

        let success =
            serde_json::to_value(success).expect("successful query result should serialize");
        let failed = serde_json::to_value(failed).expect("failed query result should serialize");

        assert_ne!(success, failed);
        assert_eq!(success["Ok"]["value"], serde_json::json!([]));
        assert_eq!(success["Ok"]["scope"], "SavedProject");
        assert_eq!(failed, serde_json::json!({ "Err": "SavedSourceChanged" }));
    }
}
