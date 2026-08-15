//! RPC vocabulary between the editor-facing server and one analysis engine.
//!
//! Document methods carry complete immutable snapshots; there is no open/change/close document
//! protocol on this boundary. Saved-project mutations are separate and either carry exact source
//! text or explicitly ask the project layer to interpret a filesystem path. Responses preserve
//! operational aborts instead of folding them into feature-specific empty values.

use std::path::PathBuf;

use crate::{
    AnalysisOutcome, CompletionClientCapabilities, CompletionResult, DocumentPositionSnapshot,
    DocumentQueryResult, DocumentRangeSnapshot, EditorDocumentSnapshot, EngineConfig, EngineError,
    GlobalOperationResult, GlobalPositionSnapshot, SaveProposal, SavedProjectChanges,
    ServiceNotification,
};

pub type EngineResult<T> = Result<T, EngineError>;

/// Requests and notifications accepted by one analysis engine.
///
/// The LSP server owns editor protocol concerns; an engine owns project indexing, immutable-input
/// queries, and cargo diagnostics. This service is the narrow request vocabulary between those two
/// domains.
#[tarpc::service]
pub trait EngineService {
    async fn initialize(root: PathBuf, config: EngineConfig) -> EngineResult<()>;

    async fn initialized() -> EngineResult<()>;

    async fn did_save(proposal: SaveProposal) -> EngineResult<u64>;

    /// Apply one settled external saved-project batch.
    ///
    /// Existing Rust files carry exact captured text. Filesystem paths are reserved for graph,
    /// discovery, and deletion inputs that require project-domain interpretation.
    async fn external_project_changes(changes: SavedProjectChanges) -> EngineResult<()>;

    async fn goto_definition(
        input: GlobalPositionSnapshot,
    ) -> EngineResult<AnalysisOutcome<DocumentQueryResult<Vec<ls_types::Location>>>>;

    async fn goto_type_definition(
        input: GlobalPositionSnapshot,
    ) -> EngineResult<AnalysisOutcome<DocumentQueryResult<Vec<ls_types::Location>>>>;

    async fn goto_implementation(
        input: GlobalPositionSnapshot,
    ) -> EngineResult<AnalysisOutcome<GlobalOperationResult<Vec<ls_types::Location>>>>;

    async fn references(
        input: GlobalPositionSnapshot,
        include_declaration: bool,
    ) -> EngineResult<AnalysisOutcome<GlobalOperationResult<Vec<ls_types::Location>>>>;

    async fn prepare_rename(
        input: GlobalPositionSnapshot,
    ) -> EngineResult<AnalysisOutcome<GlobalOperationResult<Option<ls_types::PrepareRenameResponse>>>>;

    async fn rename(
        input: GlobalPositionSnapshot,
        new_name: String,
    ) -> EngineResult<AnalysisOutcome<GlobalOperationResult<Option<ls_types::WorkspaceEdit>>>>;

    async fn document_highlight(
        input: DocumentPositionSnapshot,
    ) -> EngineResult<AnalysisOutcome<DocumentQueryResult<Vec<ls_types::DocumentHighlight>>>>;

    async fn hover(
        input: DocumentPositionSnapshot,
    ) -> EngineResult<AnalysisOutcome<DocumentQueryResult<Option<ls_types::Hover>>>>;

    async fn completion(
        input: DocumentPositionSnapshot,
        client_capabilities: CompletionClientCapabilities,
    ) -> EngineResult<AnalysisOutcome<CompletionResult>>;

    async fn formatting(
        snapshot: EditorDocumentSnapshot,
    ) -> EngineResult<AnalysisOutcome<DocumentQueryResult<Option<Vec<ls_types::TextEdit>>>>>;

    async fn document_symbol(
        snapshot: EditorDocumentSnapshot,
    ) -> EngineResult<AnalysisOutcome<DocumentQueryResult<Vec<ls_types::DocumentSymbol>>>>;

    async fn inlay_hint(
        input: DocumentRangeSnapshot,
    ) -> EngineResult<AnalysisOutcome<DocumentQueryResult<Vec<ls_types::InlayHint>>>>;

    async fn workspace_symbol(
        query: String,
    ) -> EngineResult<AnalysisOutcome<Vec<ls_types::WorkspaceSymbol>>>;

    async fn reindex_workspace() -> EngineResult<()>;

    async fn shutdown() -> EngineResult<()>;
}

/// Fire-and-forget side effects that an engine asks the LSP server to publish.
///
/// This is a service instead of an event stream so subprocess engines can report progress,
/// diagnostics, and logs without knowing anything about tower-lsp.
#[tarpc::service]
pub trait NotificationsService {
    async fn publish(notification: ServiceNotification) -> EngineResult<()>;
}
