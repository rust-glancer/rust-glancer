use std::path::PathBuf;

use crate::{EngineConfig, EngineError, ServiceNotification};

pub type EngineResult<T> = Result<T, EngineError>;

/// Requests and notifications accepted by one analysis engine.
///
/// The LSP server owns editor protocol concerns; an engine owns project indexing, document
/// freshness, queries, and cargo diagnostics. This service is the narrow request vocabulary between
/// those two domains.
#[tarpc::service]
pub trait EngineService {
    async fn initialize(root: PathBuf, config: EngineConfig) -> EngineResult<()>;

    async fn initialized() -> EngineResult<()>;

    async fn did_open(path: PathBuf, version: Option<i32>, text: String) -> EngineResult<()>;

    async fn did_change(
        path: PathBuf,
        version: Option<i32>,
        full_text: Option<String>,
        content_change_count: usize,
    ) -> EngineResult<()>;

    async fn did_save(path: PathBuf, text: Option<String>) -> EngineResult<()>;

    async fn did_close(path: PathBuf) -> EngineResult<()>;

    async fn goto_definition(
        path: PathBuf,
        position: ls_types::Position,
    ) -> EngineResult<Vec<ls_types::Location>>;

    async fn goto_type_definition(
        path: PathBuf,
        position: ls_types::Position,
    ) -> EngineResult<Vec<ls_types::Location>>;

    async fn goto_implementation(
        path: PathBuf,
        position: ls_types::Position,
    ) -> EngineResult<Vec<ls_types::Location>>;

    async fn references(
        path: PathBuf,
        position: ls_types::Position,
        include_declaration: bool,
    ) -> EngineResult<Vec<ls_types::Location>>;

    async fn document_highlight(
        path: PathBuf,
        position: ls_types::Position,
    ) -> EngineResult<Vec<ls_types::DocumentHighlight>>;

    async fn hover(
        path: PathBuf,
        position: ls_types::Position,
    ) -> EngineResult<Option<ls_types::Hover>>;

    async fn completion(
        path: PathBuf,
        position: ls_types::Position,
    ) -> EngineResult<Vec<ls_types::CompletionItem>>;

    async fn document_symbol(path: PathBuf) -> EngineResult<Vec<ls_types::DocumentSymbol>>;

    async fn inlay_hint(
        path: PathBuf,
        range: ls_types::Range,
    ) -> EngineResult<Vec<ls_types::InlayHint>>;

    async fn workspace_symbol(query: String) -> EngineResult<Vec<ls_types::WorkspaceSymbol>>;

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
