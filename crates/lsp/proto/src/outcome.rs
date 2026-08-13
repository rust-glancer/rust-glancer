//! Engine result envelope and the input identity that makes a result publishable.
//!
//! Feature values alone cannot distinguish “the query found nothing” from “the engine could not
//! answer for this captured generation.” `AnalysisOutcome` keeps those cases separate. Successful
//! values also return the saved/editor identity used by analysis, allowing the server—the owner of
//! live editor state—to reject a result overtaken after the engine finished.

use serde::{Deserialize, Serialize};

use crate::{EditorSnapshotRevision, TargetDocumentRevision};

/// Semantic result of one interactive analysis request.
///
/// An empty value inside `Ready` is a real answer. `Aborted` means analysis could not safely
/// answer for the request's captured input, so protocol adapters must not turn it into the
/// feature's empty value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnalysisOutcome<T> {
    /// Analysis produced a semantic value for the attached immutable input.
    Ready(AnalysisReady<T>),
    /// Analysis stopped for an expected operational reason and produced no feature value.
    Aborted(AnalysisAbort),
}

/// Successful value paired with the immutable project/editor input used to compute it.
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

/// Breadth of project/editor input selected for one operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnalysisScope {
    /// Use the captured target text directly without building a source-override project.
    TargetDocument,
    /// Rebuild packages containing changed editor sources; suitable for file-local queries.
    ChangedPackages,
    /// Also rebuild reverse dependents for cross-package references and edits.
    ReverseDependencyClosure,
    /// Use the saved workspace without source overrides.
    Workspace,
}

/// Exact saved/editor generation combination used by one successful analysis operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalysisInput {
    saved_project_generation: u64,
    editor_snapshot_revision: Option<EditorSnapshotRevision>,
    scope: AnalysisScope,
    target_document: Option<TargetDocumentRevision>,
}

impl AnalysisInput {
    pub fn for_target_revision(
        saved_project_generation: u64,
        editor_snapshot_revision: EditorSnapshotRevision,
        target_document: TargetDocumentRevision,
        scope: AnalysisScope,
    ) -> Self {
        Self {
            saved_project_generation,
            editor_snapshot_revision: Some(editor_snapshot_revision),
            scope,
            target_document: Some(target_document),
        }
    }

    pub const fn for_saved_project(saved_project_generation: u64) -> Self {
        Self {
            saved_project_generation,
            editor_snapshot_revision: None,
            scope: AnalysisScope::Workspace,
            target_document: None,
        }
    }

    pub const fn saved_project_generation(&self) -> u64 {
        self.saved_project_generation
    }

    pub const fn editor_snapshot_revision(&self) -> Option<EditorSnapshotRevision> {
        self.editor_snapshot_revision
    }

    pub const fn scope(&self) -> AnalysisScope {
        self.scope
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
