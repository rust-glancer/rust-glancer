use std::{future::Future, path::PathBuf, pin::Pin, sync::Arc};

use rg_project::PackageResidencyPolicy;
use rg_workspace::CargoMetadataConfig;

use crate::CheckConfig;

pub type EngineServiceHandle = Arc<dyn EngineService>;

pub type EngineResultFuture<'a, T> = Pin<Box<dyn Future<Output = anyhow::Result<T>> + Send + 'a>>;
pub type EngineNotifyFuture<'a> = Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

/// Request/notification surface used by the LSP server to talk to an analysis engine.
///
/// The current implementation is still in-process, but the boxed-future shape makes the server
/// independent from that detail. A future tarpc-backed engine can replace this interim contract
/// with generated RPC traits while keeping the same request vocabulary.
pub trait EngineService: std::fmt::Debug + Send + Sync {
    fn initialize(
        &self,
        root: PathBuf,
        package_residency_policy: PackageResidencyPolicy,
        cargo_metadata_config: CargoMetadataConfig,
        check_config: CheckConfig,
    ) -> EngineResultFuture<'_, ()>;

    fn initialized(&self) -> EngineNotifyFuture<'_>;

    fn did_open(&self, path: PathBuf, version: Option<i32>, text: String)
    -> EngineNotifyFuture<'_>;

    fn did_change(
        &self,
        path: PathBuf,
        version: Option<i32>,
        full_text: Option<String>,
        content_change_count: usize,
    ) -> EngineNotifyFuture<'_>;

    fn did_save(&self, path: PathBuf, text: Option<String>) -> EngineNotifyFuture<'_>;

    fn did_close(&self, path: PathBuf) -> EngineNotifyFuture<'_>;

    fn goto_definition(
        &self,
        path: PathBuf,
        position: ls_types::Position,
    ) -> EngineResultFuture<'_, Vec<ls_types::Location>>;

    fn goto_type_definition(
        &self,
        path: PathBuf,
        position: ls_types::Position,
    ) -> EngineResultFuture<'_, Vec<ls_types::Location>>;

    fn hover(
        &self,
        path: PathBuf,
        position: ls_types::Position,
    ) -> EngineResultFuture<'_, Option<ls_types::Hover>>;

    fn completion(
        &self,
        path: PathBuf,
        position: ls_types::Position,
    ) -> EngineResultFuture<'_, Vec<ls_types::CompletionItem>>;

    fn document_symbol(
        &self,
        path: PathBuf,
    ) -> EngineResultFuture<'_, Vec<ls_types::DocumentSymbol>>;

    fn inlay_hint(
        &self,
        path: PathBuf,
        range: ls_types::Range,
    ) -> EngineResultFuture<'_, Vec<ls_types::InlayHint>>;

    fn workspace_symbol(
        &self,
        query: String,
    ) -> EngineResultFuture<'_, Vec<ls_types::WorkspaceSymbol>>;

    fn reindex_workspace(&self) -> EngineResultFuture<'_, ()>;

    fn shutdown(&self) -> EngineResultFuture<'_, ()>;
}
