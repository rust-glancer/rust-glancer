//! LSP feature handlers and the small adapters shared between them.
//!
//! `Backend` owns the verbose `LanguageServer` trait implementation and delegates real feature work
//! to the free-function modules below. Keeping those handlers separate leaves `Backend` as an index
//! of protocol methods rather than mixing feature logic into the trait implementation.
//!
//! Two supporting modules define the shared boundary around those handlers:
//!
//! - `context` turns the editor snapshot captured by ingress into either a target-document request
//!   or a cross-file request with all relevant open documents.
//! - `query_response` checks engine response tags, verifies that document results still match live
//!   editor state, and maps engine failures to JSON-RPC errors.

mod context;
mod query_response;

use std::path::PathBuf;

use tower_lsp_server::ls_types::*;

use crate::{capabilities, engine_client::EngineClient};

pub(crate) use self::{
    context::{CompletionMethodContext, DocumentMethodContext},
    query_response::{internal_error, into_lsp_error, temporarily_unavailable},
};

pub(crate) mod text_document;
pub(crate) mod workspace;

// Cargo package versions intentionally stay at 0.0.0. Release Please updates this artifact version
// so the server reports the same version as the published extension.
pub const SERVER_VERSION: &str = "0.1.1"; // x-release-please-version

pub(crate) fn initialize() -> InitializeResult {
    InitializeResult {
        capabilities: capabilities::server_capabilities(),
        server_info: Some(ServerInfo {
            name: "rust-glancer".to_string(),
            version: Some(SERVER_VERSION.to_string()),
        }),
        offset_encoding: None,
    }
}

#[tracing::instrument(level = "trace", skip_all)]
pub(crate) async fn shutdown(engine_client: EngineClient) -> anyhow::Result<()> {
    engine_client
        .call_unconditional("shutdown", |engine_client, request_context| async move {
            engine_client.shutdown(request_context).await
        })
        .await
}

pub(crate) fn uri_to_path(uri: &Uri) -> Option<PathBuf> {
    rg_lsp_proto::file_uri_to_path(uri).ok()
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::uri_to_path;
    use tower_lsp_server::ls_types::Uri;

    #[test]
    fn uri_to_path_accepts_only_file_uris() {
        let file_path = std::env::current_dir()
            .expect("test process should have a current directory")
            .join("src/lib.rs");
        let file_uri =
            rg_lsp_proto::path_to_file_uri(&file_path).expect("test path should convert to URI");
        let cases = [
            (file_uri, Some(file_path)),
            (
                Uri::from_str("untitled:Scratch").expect("untitled URI should be valid"),
                None,
            ),
            (
                Uri::from_str("rust-analyzer://synthetic/lib.rs")
                    .expect("custom URI should be valid"),
                None,
            ),
        ];

        for (uri, expected) in cases {
            assert_eq!(uri_to_path(&uri), expected);
        }
    }
}
