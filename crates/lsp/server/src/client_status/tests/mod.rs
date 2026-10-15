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

use rg_lsp_proto::{
    DeferredIndexingOutcome, IndexingProgress, IndexingStage, ProjectInitialization,
};
use tower_lsp_server::{
    LspService,
    gen_lsp_types::{ClientCapabilities, WindowClientCapabilities},
};

use self::utils::{ProtocolClient, TestBackend};
use super::{
    ActiveWorkspaceState, ActiveWorkspaceStatus, ClientStatusCapabilities, ClientStatusPublisher,
    ClientStatusState, DeferredIndexingState, WorkspaceLifecycle, rust_analyzer,
    work_done_progress,
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
fn rust_analyzer_status_separates_health_from_background_work() {
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

    state.deferred_indexing.insert(
        ready.clone(),
        DeferredIndexingState::Running {
            generation: 1,
            progress: None,
        },
    );
    assert!(!state.rust_analyzer_status().quiescent);

    state.workspaces.remove(&failed);
    assert_eq!(
        state.rust_analyzer_status().health,
        rust_analyzer::Health::Ok
    );

    state.deferred_indexing.insert(
        ready,
        DeferredIndexingState::Failed {
            generation: 1,
            message: Arc::from("body indexing failed"),
        },
    );
    assert_eq!(
        state.rust_analyzer_status(),
        rust_analyzer::StatusSnapshot {
            health: rust_analyzer::Health::Warning,
            quiescent: true,
            message: Some(
                "Workspace `/workspace/ready` background indexing failed: body indexing failed"
                    .to_string()
            ),
        }
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
        client_status
            .workspace_initialized(
                root,
                ProjectInitialization {
                    generation: 1,
                    has_deferred_indexing: false,
                },
            )
            .await;
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
async fn deferred_progress_replaces_foreground_progress_after_queryable_boundary() {
    let capabilities = ClientStatusCapabilities {
        work_done_progress: true,
        rust_analyzer_server_status: false,
    };
    let (client_status, mut client) = ProtocolClient::initialized(capabilities).await;
    let root = Path::new("/workspace/project_a");
    let first_progress = IndexingProgress {
        stage: IndexingStage::LoweringBodies,
        completed_packages: 4,
        total_packages: 12,
    };
    let second_progress = IndexingProgress {
        stage: IndexingStage::ResolvingBodies,
        completed_packages: 7,
        total_packages: 12,
    };
    let publish = async move {
        client_status.workspace_indexing(root).await;
        client_status.deferred_indexing_started(root, 7).await;
        client_status
            .deferred_indexing_progress(root, 7, first_progress)
            .await;
        client_status.workspace_ready(root).await;
        client_status
            .deferred_indexing_progress(root, 7, second_progress)
            .await;
        client_status
            .deferred_indexing_finished(root, 7, DeferredIndexingOutcome::Succeeded)
            .await;
    };
    let observe = async {
        let create = client.next_request().await;
        client.name_progress_token(&create, "foreground");
        client.acknowledge(create).await;
        let create = client.next_request().await;
        client.name_progress_token(&create, "background");
        client.acknowledge(create).await;
        client.drain().await;
    };

    tokio::join!(publish, observe);
    client.check(expect_test::expect![[r#"
        request #0 window/workDoneProgress/create {"token":"foreground"}
        reply #0 {"result":null}
        notify $/progress {"token":"foreground","value":{"cancellable":false,"kind":"begin","title":"Indexing project_a"}}
        notify rust-glancer/deferredIndexingStarted {"root":"/workspace/project_a"}
        notify $/progress {"token":"foreground","value":{"kind":"end","message":"Finished"}}
        request #1 window/workDoneProgress/create {"token":"background"}
        reply #1 {"result":null}
        notify $/progress {"token":"background","value":{"cancellable":false,"kind":"begin","title":"project_a ready · background"}}
        notify $/progress {"token":"background","value":{"kind":"report","message":"Lowering · 4/12"}}
        notify $/progress {"token":"background","value":{"kind":"report","message":"Resolving · 7/12"}}
        notify $/progress {"token":"background","value":{"kind":"end","message":"Finished"}}
        notify rust-glancer/deferredIndexingFinished {"outcome":"succeeded","root":"/workspace/project_a"}
    "#]]);
}

#[tokio::test]
async fn deferred_failure_ends_progress_and_publishes_degraded_ready_status() {
    let capabilities = ClientStatusCapabilities {
        work_done_progress: true,
        rust_analyzer_server_status: true,
    };
    let (client_status, mut client) = ProtocolClient::initialized(capabilities).await;
    let root = Path::new("/workspace/project_a");
    let publish = async move {
        client_status.workspace_ready(root).await;
        client_status.deferred_indexing_started(root, 7).await;
        client_status
            .deferred_indexing_finished(
                root,
                7,
                DeferredIndexingOutcome::Failed {
                    message: "body indexing failed".to_string(),
                },
            )
            .await;
    };
    let observe = async {
        let create = client.next_request().await;
        client.name_progress_token(&create, "background");
        client.acknowledge(create).await;
        client.drain().await;
    };

    tokio::join!(publish, observe);
    client.check(expect_test::expect![[r#"
        notify experimental/serverStatus {"health":"ok","quiescent":true}
        request #0 window/workDoneProgress/create {"token":"background"}
        reply #0 {"result":null}
        notify $/progress {"token":"background","value":{"cancellable":false,"kind":"begin","title":"project_a ready · background"}}
        notify experimental/serverStatus {"health":"ok","quiescent":false}
        notify rust-glancer/deferredIndexingStarted {"root":"/workspace/project_a"}
        notify $/progress {"token":"background","value":{"kind":"end","message":"Failed"}}
        notify experimental/serverStatus {"health":"warning","message":"Workspace `/workspace/project_a` background indexing failed: body indexing failed","quiescent":true}
        notify rust-glancer/deferredIndexingFinished {"message":"body indexing failed","outcome":"failed","root":"/workspace/project_a"}
    "#]]);
}

#[tokio::test]
async fn stale_deferred_generation_cannot_replace_or_finish_current_progress() {
    let (service, _socket) = LspService::new(|client| TestBackend {
        client_status: ClientStatusPublisher::new(client, ClientStatusCapabilities::default()),
    });
    let client_status = service.inner().client_status.clone();
    let root = Path::new("/workspace/project_a");
    let progress = IndexingProgress {
        stage: IndexingStage::ResolvingBodies,
        completed_packages: 2,
        total_packages: 8,
    };

    client_status.deferred_indexing_started(root, 9).await;
    client_status
        .deferred_indexing_progress(root, 8, progress)
        .await;
    client_status
        .deferred_indexing_finished(root, 8, DeferredIndexingOutcome::Succeeded)
        .await;

    let state = client_status.state.lock().await;
    assert_eq!(
        state.deferred_indexing.get(root),
        Some(&DeferredIndexingState::Running {
            generation: 9,
            progress: None,
        })
    );
}

#[tokio::test]
async fn rust_glancer_notifications_remain_additive_without_negotiation() {
    let (client_status, mut client) =
        ProtocolClient::initialized(ClientStatusCapabilities::default()).await;
    let root = Path::new("/workspace/project_a");
    let publish = async move {
        client_status
            .active_workspace_changed(Some(ActiveWorkspaceStatus {
                root: root.to_path_buf(),
                state: ActiveWorkspaceState::Ready,
                message: None,
            }))
            .await;
        client_status.deferred_indexing_started(root, 1).await;
        client_status
            .deferred_indexing_finished(
                root,
                1,
                DeferredIndexingOutcome::Failed {
                    message: "body indexing failed".to_string(),
                },
            )
            .await;
    };

    tokio::join!(publish, client.drain());
    client.check(expect_test::expect![[r#"
        notify rust-glancer/activeWorkspaceChanged {"root":"/workspace/project_a","state":"ready"}
        notify rust-glancer/deferredIndexingStarted {"root":"/workspace/project_a"}
        notify rust-glancer/deferredIndexingFinished {"message":"body indexing failed","outcome":"failed","root":"/workspace/project_a"}
    "#]]);
}

#[tokio::test]
async fn startup_quiescence_accounts_for_pending_and_completed_work() {
    // The response and worker notifications travel through separate channels. Neither order
    // should make clients proceed before completion or wait forever after it.
    for (worker_finishes_first, expected_quiescence, message) in [
        (false, false, "pending work should keep the server busy"),
        (
            true,
            true,
            "initialization should not restart completed work",
        ),
    ] {
        let (service, _socket) = LspService::new(|client| TestBackend {
            client_status: ClientStatusPublisher::new(client, ClientStatusCapabilities::default()),
        });
        let client_status = &service.inner().client_status;
        let root = Path::new("/workspace/project");
        client_status.workspace_indexing(root).await;
        if worker_finishes_first {
            client_status.deferred_indexing_started(root, 1).await;
            client_status
                .deferred_indexing_finished(root, 1, DeferredIndexingOutcome::Succeeded)
                .await;
        }
        client_status
            .workspace_initialized(
                root,
                ProjectInitialization {
                    generation: 1,
                    has_deferred_indexing: true,
                },
            )
            .await;
        assert_eq!(
            client_status
                .state
                .lock()
                .await
                .rust_analyzer_status()
                .quiescent,
            expected_quiescence,
            "{message}",
        );

        if !worker_finishes_first {
            client_status.deferred_indexing_started(root, 1).await;
            assert!(
                !client_status
                    .state
                    .lock()
                    .await
                    .rust_analyzer_status()
                    .quiescent
            );
            client_status
                .deferred_indexing_finished(root, 1, DeferredIndexingOutcome::Succeeded)
                .await;
            assert!(
                client_status
                    .state
                    .lock()
                    .await
                    .rust_analyzer_status()
                    .quiescent
            );
        }
    }
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
    client_status.deferred_indexing_started(root, 1).await;

    let state = client_status.state.lock().await;
    assert_eq!(
        state.workspaces.get(root),
        Some(&WorkspaceLifecycle::Unavailable(Arc::from("engine exited")))
    );
    assert!(state.deferred_indexing.is_empty());
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
