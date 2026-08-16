//! RPC vocabulary between the editor-facing server and one analysis engine.
//!
//! Document methods carry complete immutable snapshots; there is no open/change/close document
//! protocol on this boundary. Saved-project mutations are separate and either carry exact source
//! text or explicitly ask the project layer to interpret a filesystem path. Responses preserve
//! operational failures instead of folding them into feature-specific empty values.

use std::path::PathBuf;

use crate::{
    CompletionClientCapabilities, DocumentPositionSnapshot, DocumentRangeSnapshot,
    EditorDocumentSnapshot, EngineConfig, EngineError, GlobalPositionSnapshot, QueryError,
    QueryValue, SaveProposal, SavedProjectChanges, ServiceNotification,
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
    ) -> Result<QueryValue<Vec<ls_types::Location>>, QueryError>;

    async fn goto_type_definition(
        input: GlobalPositionSnapshot,
    ) -> Result<QueryValue<Vec<ls_types::Location>>, QueryError>;

    async fn goto_implementation(
        input: GlobalPositionSnapshot,
    ) -> Result<QueryValue<Vec<ls_types::Location>>, QueryError>;

    async fn references(
        input: GlobalPositionSnapshot,
        include_declaration: bool,
    ) -> Result<QueryValue<Vec<ls_types::Location>>, QueryError>;

    async fn prepare_rename(
        input: GlobalPositionSnapshot,
    ) -> Result<QueryValue<Option<ls_types::PrepareRenameResponse>>, QueryError>;

    async fn rename(
        input: GlobalPositionSnapshot,
        new_name: String,
    ) -> Result<QueryValue<Option<ls_types::WorkspaceEdit>>, QueryError>;

    async fn document_highlight(
        input: DocumentPositionSnapshot,
    ) -> Result<QueryValue<Vec<ls_types::DocumentHighlight>>, QueryError>;

    async fn hover(
        input: DocumentPositionSnapshot,
    ) -> Result<QueryValue<Option<ls_types::Hover>>, QueryError>;

    async fn completion(
        input: DocumentPositionSnapshot,
        client_capabilities: CompletionClientCapabilities,
    ) -> Result<QueryValue<Vec<ls_types::CompletionItem>>, QueryError>;

    async fn formatting(
        snapshot: EditorDocumentSnapshot,
    ) -> Result<QueryValue<Option<Vec<ls_types::TextEdit>>>, QueryError>;

    async fn document_symbol(
        snapshot: EditorDocumentSnapshot,
    ) -> Result<QueryValue<Vec<ls_types::DocumentSymbol>>, QueryError>;

    async fn inlay_hint(
        input: DocumentRangeSnapshot,
    ) -> Result<QueryValue<Vec<ls_types::InlayHint>>, QueryError>;

    async fn workspace_symbol(
        query: String,
    ) -> Result<QueryValue<Vec<ls_types::WorkspaceSymbol>>, QueryError>;

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
