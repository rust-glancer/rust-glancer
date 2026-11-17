//! RPC vocabulary between the editor-facing server and one analysis engine.
//!
//! Document methods carry complete immutable snapshots; there is no second document-text lifecycle
//! on this boundary.
//! Saved-project mutations are separate and either carry exact source text or explicitly ask the
//! project layer to interpret a filesystem path. Responses preserve operational failures instead
//! of folding them into feature-specific empty values.

use std::path::PathBuf;

use gen_lsp_types::{
    CodeAction, CompletionItem, DocumentHighlight, DocumentSymbol, FoldingRange, Hover, InlayHint,
    Location, PrepareRenameResult, Range, SemanticTokens, TextEdit, WorkspaceEdit, WorkspaceSymbol,
};
use serde::{Deserialize, Serialize};

use crate::{
    CodeActionRequestContext, CompletionClientCapabilities, DocumentPositionSnapshot,
    DocumentRangeSnapshot, DocumentSnapshot, EditorDocumentSnapshot, EditorSnapshot, EngineConfig,
    EngineError, FoldingClientCapabilities, OpenDocumentsRevision, QueryError, QueryValue,
    SaveProposal, SavedProjectChanges, ServiceNotification,
};

pub type EngineResult<T> = Result<T, EngineError>;

/// Identity of the saved project after eager indexing completes.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct ProjectInitialization {
    pub generation: u64,
}

/// Requests and notifications accepted by one analysis engine.
///
/// The LSP server owns editor protocol concerns; an engine owns project indexing, immutable-input
/// queries, and cargo diagnostics. This service is the narrow request vocabulary between those two
/// domains.
#[tarpc::service]
pub trait EngineService {
    async fn initialize(root: PathBuf, config: EngineConfig)
    -> EngineResult<ProjectInitialization>;

    async fn initialized() -> EngineResult<()>;

    /// Release a superseded capture without publishing saved project state.
    async fn invalidate_editor_snapshot(revision: OpenDocumentsRevision) -> EngineResult<()>;

    async fn did_save(proposal: SaveProposal) -> EngineResult<u64>;

    /// Apply one settled external saved-project batch.
    ///
    /// Existing Rust files carry exact captured text. Filesystem paths are reserved for graph,
    /// discovery, and deletion inputs that require project-domain interpretation.
    async fn external_project_changes(changes: SavedProjectChanges) -> EngineResult<()>;

    async fn goto_definition(
        input: DocumentPositionSnapshot,
    ) -> Result<QueryValue<Vec<Location>>, QueryError>;

    async fn goto_type_definition(
        input: DocumentPositionSnapshot,
    ) -> Result<QueryValue<Vec<Location>>, QueryError>;

    async fn goto_implementation(
        input: DocumentPositionSnapshot,
    ) -> Result<QueryValue<Vec<Location>>, QueryError>;

    async fn references(
        input: DocumentPositionSnapshot,
        include_declaration: bool,
    ) -> Result<QueryValue<Vec<Location>>, QueryError>;

    async fn prepare_rename(
        input: DocumentPositionSnapshot,
    ) -> Result<QueryValue<Option<PrepareRenameResult>>, QueryError>;

    async fn rename(
        input: DocumentPositionSnapshot,
        new_name: String,
        versioned_document_edits: bool,
    ) -> Result<QueryValue<Option<WorkspaceEdit>>, QueryError>;

    async fn document_highlight(
        input: DocumentPositionSnapshot,
    ) -> Result<QueryValue<Vec<DocumentHighlight>>, QueryError>;

    async fn hover(
        input: DocumentPositionSnapshot,
    ) -> Result<QueryValue<Option<Hover>>, QueryError>;

    async fn code_action(
        input: DocumentRangeSnapshot,
        request_context: CodeActionRequestContext,
    ) -> Result<QueryValue<Vec<CodeAction>>, QueryError>;

    async fn completion(
        input: DocumentPositionSnapshot,
        client_capabilities: CompletionClientCapabilities,
    ) -> Result<QueryValue<Vec<CompletionItem>>, QueryError>;

    async fn formatting(
        snapshot: EditorDocumentSnapshot,
    ) -> Result<QueryValue<Option<Vec<TextEdit>>>, QueryError>;

    async fn document_symbol(
        snapshot: DocumentSnapshot,
    ) -> Result<QueryValue<Vec<DocumentSymbol>>, QueryError>;

    async fn folding_range(
        snapshot: EditorDocumentSnapshot,
        client_capabilities: FoldingClientCapabilities,
    ) -> Result<QueryValue<Vec<FoldingRange>>, QueryError>;

    async fn semantic_tokens(
        snapshot: DocumentSnapshot,
        range: Option<Range>,
    ) -> Result<QueryValue<SemanticTokens>, QueryError>;

    async fn inlay_hint(
        input: DocumentRangeSnapshot,
    ) -> Result<QueryValue<Vec<InlayHint>>, QueryError>;

    async fn workspace_symbol(
        snapshot: EditorSnapshot,
        query: String,
    ) -> Result<QueryValue<Vec<WorkspaceSymbol>>, QueryError>;

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
