use std::{borrow::Cow, path::PathBuf};

use tower_lsp_server::{
    jsonrpc::{Error, ErrorCode},
    ls_types::*,
};

use rg_lsp_proto::ClientCapabilities;

use crate::{capabilities, engine_client::EngineClient};

pub(crate) mod text_document;
pub(crate) mod workspace;

#[derive(Clone, Debug)]
pub(crate) struct MethodContext {
    pub(crate) engine_client: EngineClient,
    pub(crate) client_capabilities: ClientCapabilities,
}

pub(crate) fn initialize() -> InitializeResult {
    InitializeResult {
        capabilities: capabilities::server_capabilities(),
        server_info: Some(ServerInfo {
            name: "rust-glancer".to_string(),
            version: Some(env!("CARGO_PKG_VERSION").to_string()),
        }),
        offset_encoding: None,
    }
}

#[tracing::instrument(level = "trace", skip_all)]
pub(crate) async fn shutdown(ctx: MethodContext) -> anyhow::Result<()> {
    ctx.engine_client
        .call("shutdown", |engine_client, request_context| async move {
            engine_client.shutdown(request_context).await
        })
        .await
}

pub(crate) fn internal_error(error: anyhow::Error) -> Error {
    Error {
        code: ErrorCode::InternalError,
        message: Cow::Owned(format!("{error:#}")),
        data: None,
    }
}

pub(crate) fn uri_to_path(uri: &Uri) -> Option<PathBuf> {
    if !uri.as_str().starts_with("file:") {
        return None;
    }

    uri.to_file_path().map(|path| path.into_owned())
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::{internal_error, uri_to_path};
    use tower_lsp_server::ls_types::Uri;

    #[test]
    fn internal_error_preserves_context_chain() {
        let error = anyhow::anyhow!("engine response channel closed")
            .context("while receiving engine response")
            .context("while handling hover");

        let message = internal_error(error).message;

        assert_eq!(
            message.as_ref(),
            "while handling hover: while receiving engine response: engine response channel closed",
        );
        assert!(
            !message.contains('\n'),
            "alternate anyhow display should keep context chains on one line",
        );
    }

    #[test]
    fn uri_to_path_accepts_only_file_uris() {
        let file_path = std::env::current_dir()
            .expect("test process should have a current directory")
            .join("src/lib.rs");
        let file_uri = Uri::from_file_path(&file_path).expect("test path should convert to URI");
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
