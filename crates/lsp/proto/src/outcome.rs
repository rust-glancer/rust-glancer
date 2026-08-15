//! Separates a real analysis result from a request that had to stop.
//!
//! An empty completion, hover, or location list can be a valid answer. It must not look the same as
//! a query that could not use its captured input. `AnalysisOutcome` keeps those cases separate.
//! Successful results also return the project and editor ids used by the engine, so the server can
//! reject a result if the editor changed before publication.

use serde::{Deserialize, Serialize};

use crate::{OpenDocumentsRevision, TargetDocumentRevision};

/// Result of one interactive analysis request.
///
/// An empty value inside `Ready` is a real answer. `Aborted` means analysis could not safely
/// answer for the request's captured input, so protocol adapters must not turn it into the
/// feature's empty value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnalysisOutcome<T> {
    /// Analysis produced a value for the attached immutable input.
    Ready(AnalysisReady<T>),
    /// Analysis stopped for an expected operational reason and produced no feature value.
    Aborted(AnalysisAbort),
}

/// Successful value paired with the project and editor ids used to compute it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalysisReady<T> {
    value: T,
    input: AnalysisInput,
}

impl<T> AnalysisReady<T> {
    pub fn new(value: T, input: AnalysisInput) -> Self {
        Self { value, input }
    }

    pub fn value(&self) -> &T {
        &self.value
    }

    pub fn input(&self) -> &AnalysisInput {
        &self.input
    }

    pub fn into_value(self) -> T {
        self.value
    }
}

/// Project and editor ids that the server must validate before publishing a result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalysisInput {
    saved_project_generation: u64,
    open_documents_revision: Option<OpenDocumentsRevision>,
    target_document: Option<TargetDocumentRevision>,
}

impl AnalysisInput {
    /// Tag a result that depends on every captured open document staying unchanged.
    pub fn for_global_operation(
        saved_project_generation: u64,
        open_documents_revision: OpenDocumentsRevision,
        target_document: TargetDocumentRevision,
    ) -> Self {
        Self {
            saved_project_generation,
            open_documents_revision: Some(open_documents_revision),
            target_document: Some(target_document),
        }
    }

    /// Tag a result that depends on its target document but not on open sibling documents.
    pub fn for_target_document(
        saved_project_generation: u64,
        target_document: TargetDocumentRevision,
    ) -> Self {
        Self {
            saved_project_generation,
            open_documents_revision: None,
            target_document: Some(target_document),
        }
    }

    pub const fn for_saved_project(saved_project_generation: u64) -> Self {
        Self {
            saved_project_generation,
            open_documents_revision: None,
            target_document: None,
        }
    }

    pub const fn saved_project_generation(&self) -> u64 {
        self.saved_project_generation
    }

    pub const fn open_documents_revision(&self) -> Option<OpenDocumentsRevision> {
        self.open_documents_revision
    }

    pub fn target_document(&self) -> Option<&TargetDocumentRevision> {
        self.target_document.as_ref()
    }
}

/// Operational reason why an interactive request produced no semantic answer.
///
/// Failures are not represented here. They remain `EngineError`s so callers cannot accidentally
/// treat a broken analysis operation as an ordinary retryable lifecycle event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnalysisAbort {
    /// Disk no longer matches the saved generation used by the request.
    SourceChanged,
    /// Request input or a recoverable analysis resource cannot be used yet.
    TemporarilyUnavailable,
}

impl AnalysisAbort {
    pub const fn description(self) -> &'static str {
        match self {
            Self::SourceChanged => "saved source changed while analysis was running",
            Self::TemporarilyUnavailable => "analysis input is temporarily unavailable",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AnalysisAbort, AnalysisInput, AnalysisOutcome, AnalysisReady};

    #[test]
    fn valid_empty_and_operational_abort_have_distinct_wire_shapes() {
        let ready = serde_json::to_value(AnalysisOutcome::Ready(AnalysisReady::new(
            Vec::<usize>::new(),
            AnalysisInput::for_saved_project(7),
        )))
        .expect("ready analysis outcome should serialize");
        let aborted = serde_json::to_value(AnalysisOutcome::<Vec<usize>>::Aborted(
            AnalysisAbort::SourceChanged,
        ))
        .expect("aborted analysis outcome should serialize");

        assert_ne!(ready, aborted);
        assert_eq!(ready["Ready"]["value"], serde_json::json!([]));
        assert_eq!(ready["Ready"]["input"]["saved_project_generation"], 7);
        assert_eq!(aborted, serde_json::json!({ "Aborted": "SourceChanged" }));
    }
}
