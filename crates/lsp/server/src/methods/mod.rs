use std::{borrow::Cow, path::PathBuf};

use tower_lsp_server::{
    jsonrpc::{Error, ErrorCode},
    ls_types::*,
};

use crate::{capabilities, engine_client::EngineClient};

pub(crate) mod text_document;
pub(crate) mod workspace;

#[derive(Clone, Debug)]
pub(crate) struct MethodContext {
    pub(crate) engine_client: EngineClient,
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
        message: Cow::Owned(error.to_string()),
        data: None,
    }
}

pub(crate) fn uri_to_path(uri: &Uri) -> Option<PathBuf> {
    if !uri.as_str().starts_with("file:") {
        return None;
    }

    uri.to_file_path().map(|path| path.into_owned())
}
