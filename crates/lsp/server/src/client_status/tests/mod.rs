//! Protocol scenarios snapshot complete payloads in event order. `request` and `notify` come
//! from the server; `reply` is the test client's explicit acknowledgement. Generated request IDs
//! are numbered by first appearance, and tests name workspace progress tokens. Engine-owned tokens
//! stay literal so their original values remain part of the contract.
//!
//! Move the publisher into its future: dropping it closes the socket, allowing `drain` to capture
//! all remaining events without sleeps or fixed event counts. State-only tests use direct checks.

mod utils;

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use tower_lsp_server::{
    LspService,
    gen_lsp_types::{ClientCapabilities, WindowClientCapabilities},
};

use self::utils::{ProtocolClient, TestBackend};
use super::{
    ClientStatusCapabilities, ClientStatusPublisher, ClientStatusState, WorkspaceLifecycle,
    rust_analyzer, work_done_progress,
};

#[test]
fn reads_negotiated_status_capabilities() {
    let capabilities = ClientCapabilities {
        experimental: Some(serde_json::json!({
            "serverStatusNotification": true,
        })),
        window: Some(WindowClientCapabilities {
            work_done_progress: Some(true),
            ..WindowClientCapabilities::default()
        }),
        ..ClientCapabilities::default()
    };

    assert_eq!(
        ClientStatusCapabilities::from_lsp_client_capabilities(&capabilities),
        ClientStatusCapabilities {
            work_done_progress: true,
            rust_analyzer_server_status: true,
        }
    );
}

#[test]
fn rust_analyzer_status_separates_health_from_indexing() {
    let mut state = ClientStatusState::default();
    let ready = PathBuf::from("/workspace/ready");
    let failed = PathBuf::from("/workspace/failed");
    state
        .workspaces
        .insert(ready.clone(), WorkspaceLifecycle::Indexing);

    assert_eq!(
        state.rust_analyzer_status(),
        rust_analyzer::StatusSnapshot {
            health: rust_analyzer::Health::Ok,
            quiescent: false,
            message: None,
        }
    );

    state
        .workspaces
        .insert(ready.clone(), WorkspaceLifecycle::Ready);
    state.workspaces.insert(
        failed.clone(),
        WorkspaceLifecycle::Failed(Arc::from("engine exited")),
    );
    let partially_failed = state.rust_analyzer_status();
    assert_eq!(partially_failed.health, rust_analyzer::Health::Warning);
    assert!(partially_failed.quiescent);
    assert_eq!(
        partially_failed.message.as_deref(),
        Some("Workspace `/workspace/failed` failed: engine exited")
    );
}

#[test]
fn only_failed_workspace_makes_the_server_unhealthy() {
    let mut state = ClientStatusState::default();
    state.workspaces.insert(
        PathBuf::from("/workspace/failed"),
        WorkspaceLifecycle::Failed(Arc::from("metadata failed")),
    );

    assert_eq!(
        state.rust_analyzer_status().health,
        rust_analyzer::Health::Error
    );
}

#[tokio::test]
async fn indexing_lifecycle_publishes_negotiated_client_protocol() {
    let capabilities = ClientStatusCapabilities {
        work_done_progress: true,
        rust_analyzer_server_status: true,
    };
    let (client_status, mut client) = ProtocolClient::initialized(capabilities).await;
    let root = Path::new("/workspace/project_a");
    let publish = async move {
        client_status.workspace_indexing(root).await;
        client_status.workspace_ready(root).await;
    };
    let observe = async {
        let create = client.next_request().await;
        client.name_progress_token(&create, "foreground");
        client.acknowledge(create).await;
        client.drain().await;
    };

    tokio::join!(publish, observe);
    client.check(expect_test::expect![[r#"
        request #0 window/workDoneProgress/create {"token":"foreground"}
        reply #0 {"result":null}
        notify $/progress {"token":"foreground","value":{"cancellable":false,"kind":"begin","title":"Indexing project_a"}}
        notify experimental/serverStatus {"health":"ok","quiescent":false}
        notify $/progress {"token":"foreground","value":{"kind":"end","message":"Finished"}}
        notify experimental/serverStatus {"health":"ok","quiescent":true}
    "#]]);
}

#[tokio::test]
async fn terminal_engine_failure_ignores_late_project_status() {
    let (service, _socket) = LspService::new(|client| TestBackend {
        client_status: ClientStatusPublisher::new(client, ClientStatusCapabilities::default()),
    });
    let client_status = service.inner().client_status.clone();
    let root = Path::new("/workspace/project_a");

    client_status
        .workspace_unavailable(root, "engine exited")
        .await;
    client_status.workspace_indexing(root).await;
    client_status.workspace_ready(root).await;

    let state = client_status.state.lock().await;
    assert_eq!(
        state.workspaces.get(root),
        Some(&WorkspaceLifecycle::Unavailable(Arc::from("engine exited")))
    );
}

#[tokio::test]
async fn engine_progress_preserves_begin_and_end_payloads() {
    let (client_status, mut client) =
        ProtocolClient::initialized(ClientStatusCapabilities::default()).await;
    let publish = async move {
        let lsp_client = &client_status.lsp_client;
        let token =
            tower_lsp_server::gen_lsp_types::ProgressToken::String("cargo/check".to_string());
        work_done_progress::begin_engine_progress(
            lsp_client,
            token.clone(),
            "Cargo check".to_string(),
            Some("Checking workspace".to_string()),
        )
        .await;
        work_done_progress::end_engine_progress(lsp_client, token, Some("Finished".to_string()))
            .await;
    };
    let observe = async {
        let create = client.next_request().await;
        client.acknowledge(create).await;
        client.drain().await;
    };

    tokio::join!(publish, observe);
    client.check(expect_test::expect![[r#"
        request #0 window/workDoneProgress/create {"token":"cargo/check"}
        reply #0 {"result":null}
        notify $/progress {"token":"cargo/check","value":{"cancellable":false,"kind":"begin","message":"Checking workspace","title":"Cargo check"}}
        notify $/progress {"token":"cargo/check","value":{"kind":"end","message":"Finished"}}
    "#]]);
}
