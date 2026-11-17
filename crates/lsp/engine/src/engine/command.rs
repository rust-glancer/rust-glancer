//! Messages crossing from async RPC tasks onto the synchronous analysis lane.
//!
use std::path::PathBuf;

use gen_lsp_types::{
    CodeAction, CompletionItem, DocumentHighlight, DocumentSymbol, FoldingRange, Hover, InlayHint,
    Location, PrepareRenameResult, Range, SemanticTokens, TextEdit, WorkspaceEdit, WorkspaceSymbol,
};
use rg_lsp_proto::{
    CodeActionRequestContext, CompletionClientCapabilities, DocumentPositionSnapshot,
    DocumentRangeSnapshot, DocumentSnapshot, EditorDocumentSnapshot, EditorSnapshot,
    FoldingClientCapabilities, InlayHintsConfig, OpenDocumentsRevision, ProjectInitialization,
    QueryError, QueryValue,
};
use rg_project::SavedFileChange;
use tokio::sync::oneshot;

use super::ProjectConfiguration;

/// Response endpoint owned by one request until the engine dispatcher answers it.
pub(crate) type EngineResponder<T> = oneshot::Sender<anyhow::Result<T>>;
/// Response endpoint for a semantic request that may finish without a publishable feature value.
pub(crate) type QueryResponder<T> = oneshot::Sender<Result<QueryValue<T>, QueryError>>;

/// Work accepted by the one analysis thread.
///
/// Keeping project mutations and analysis queries in one enum makes their ordering explicit. RPC
/// handlers may run concurrently, but the dispatcher consumes these commands one at a time.
#[derive(Debug)]
pub(crate) enum EngineCommand {
    Initialize {
        root: PathBuf,
        configuration: ProjectConfiguration,
        inlay_hints: InlayHintsConfig,
        respond_to: EngineResponder<ProjectInitialization>,
    },
    InvalidateEditorSnapshot {
        revision: OpenDocumentsRevision,
        respond_to: EngineResponder<()>,
    },
    /// Background repair scheduled when a query proves that saved analysis is stale.
    RecoverStaleSource {
        path: PathBuf,
    },
    /// Exact captured sources, optionally paired with graph/discovery path changes.
    SavedProjectChanges {
        changes: Vec<SavedFileChange>,
        respond_to: EngineResponder<u64>,
    },
    GotoDefinition {
        input: DocumentPositionSnapshot,
        respond_to: QueryResponder<Vec<Location>>,
    },
    GotoTypeDefinition {
        input: DocumentPositionSnapshot,
        respond_to: QueryResponder<Vec<Location>>,
    },
    GotoImplementation {
        input: DocumentPositionSnapshot,
        respond_to: QueryResponder<Vec<Location>>,
    },
    References {
        input: DocumentPositionSnapshot,
        include_declaration: bool,
        respond_to: QueryResponder<Vec<Location>>,
    },
    PrepareRename {
        input: DocumentPositionSnapshot,
        respond_to: QueryResponder<Option<PrepareRenameResult>>,
    },
    Rename {
        input: DocumentPositionSnapshot,
        new_name: String,
        versioned_document_edits: bool,
        respond_to: QueryResponder<Option<WorkspaceEdit>>,
    },
    DocumentHighlight {
        input: DocumentPositionSnapshot,
        respond_to: QueryResponder<Vec<DocumentHighlight>>,
    },
    Hover {
        input: DocumentPositionSnapshot,
        respond_to: QueryResponder<Option<Hover>>,
    },
    CodeAction {
        input: DocumentRangeSnapshot,
        request_context: CodeActionRequestContext,
        respond_to: QueryResponder<Vec<CodeAction>>,
    },
    Completion {
        input: DocumentPositionSnapshot,
        client_capabilities: CompletionClientCapabilities,
        respond_to: QueryResponder<Vec<CompletionItem>>,
    },
    Formatting {
        snapshot: EditorDocumentSnapshot,
        respond_to: QueryResponder<Option<Vec<TextEdit>>>,
    },
    DocumentSymbol {
        snapshot: DocumentSnapshot,
        respond_to: QueryResponder<Vec<DocumentSymbol>>,
    },
    FoldingRange {
        snapshot: EditorDocumentSnapshot,
        client_capabilities: FoldingClientCapabilities,
        respond_to: QueryResponder<Vec<FoldingRange>>,
    },
    SemanticTokens {
        snapshot: DocumentSnapshot,
        range: Option<Range>,
        respond_to: QueryResponder<SemanticTokens>,
    },
    InlayHint {
        input: DocumentRangeSnapshot,
        respond_to: QueryResponder<Vec<InlayHint>>,
    },
    WorkspaceSymbol {
        snapshot: EditorSnapshot,
        query: String,
        respond_to: QueryResponder<Vec<WorkspaceSymbol>>,
    },
    ReindexWorkspace {
        respond_to: EngineResponder<()>,
    },
    Shutdown(EngineResponder<()>),
}
