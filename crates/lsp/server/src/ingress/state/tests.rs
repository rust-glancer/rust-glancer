use std::{path::PathBuf, sync::Arc};

use rg_lsp_proto::EngineServiceClient;
use tarpc::client::Config as TarpcClientConfig;
use tower_lsp_server::ls_types::{Position, Range, TextDocumentContentChangeEvent};

use crate::{engine_client::EngineClient, engine_registry::OpenDocumentRoute};

use super::{
    DiagnosticsPublication, DocumentRevision, EditorStateHandle, LifecycleEvent,
    PositionRecaptureError,
};

#[test]
fn internal_revisions_ignore_repeated_reset_and_missing_client_versions() {
    let path = PathBuf::from("/workspace/src/lib.rs");
    let state = EditorStateHandle::default();

    let opened = state.open(path.clone(), Some(8), "fn v1() {}".to_string());
    let LifecycleEvent::Open { document, .. } = opened.event() else {
        panic!("expected open event");
    };
    let mut revisions = vec![document.revision()];
    for (version, text) in [
        (Some(8), "fn v2() {}"),
        (Some(1), "fn v3() {}"),
        (None, "fn v4() {}"),
    ] {
        assert!(
            state
                .change(&path, version, &[full(text)])
                .expect("full change should apply")
        );
        revisions.push(
            state
                .document(Some(path.clone()))
                .expect("changed document should remain queryable")
                .document()
                .revision(),
        );
    }
    assert_eq!(
        revisions,
        [
            DocumentRevision::new(1),
            DocumentRevision::new(2),
            DocumentRevision::new(3),
            DocumentRevision::new(4),
        ]
    );
}

#[test]
fn close_and_reopen_allocates_a_new_session_even_when_client_version_resets() {
    let path = PathBuf::from("/workspace/src/lib.rs");
    let state = EditorStateHandle::default();

    let opened = state.open(path.clone(), Some(41), "fn first() {}".to_string());
    let first_session = match opened.event() {
        LifecycleEvent::Open { document, .. } => document.session(),
        event => panic!("expected open event, got {event:?}"),
    };
    let closed = state.close(&path).expect("open document should close");
    let reopened = state.open(path.clone(), Some(1), "fn second() {}".to_string());
    let second_session = match reopened.event() {
        LifecycleEvent::Open { document, .. } => document.session(),
        event => panic!("expected reopened event, got {event:?}"),
    };

    assert_ne!(first_session, second_session);
    assert!(matches!(closed.event(), LifecycleEvent::Close { .. }));
    assert_eq!(
        state
            .document(Some(path))
            .expect("reopened document should be queryable")
            .document()
            .text(),
        "fn second() {}"
    );
}

#[test]
fn invalid_incremental_range_makes_requests_unavailable_until_next_full_text() {
    let path = PathBuf::from("/workspace/src/lib.rs");
    let state = EditorStateHandle::default();
    state.open(path.clone(), Some(1), "fn first() {}".to_string());

    let error = state
        .change(&path, Some(2), &[incremental((0, 40), (0, 40), "invalid")])
        .expect_err("an out-of-bounds incremental range should be rejected");
    assert!(error.to_string().contains("UTF-16 boundary"));
    assert!(state.document(Some(path.clone())).is_err());

    assert!(
        state
            .change(&path, Some(3), &[full("fn recovered() {}")])
            .expect("full replacement should repair synchronized text")
    );
    assert_eq!(
        state
            .document(Some(path))
            .expect("full text should make the document queryable")
            .document()
            .text(),
        "fn recovered() {}"
    );
}

#[test]
fn unresolved_or_failed_route_does_not_discard_captured_editor_text() {
    let path = PathBuf::from("/workspace/src/lib.rs");
    let state = EditorStateHandle::default();
    let opened = state.open(path.clone(), Some(1), "fn retained() {}".to_string());
    let LifecycleEvent::Open { route, .. } = opened.event() else {
        panic!("expected open event");
    };
    let captured = state
        .document(Some(path.clone()))
        .expect("opened editor text should be captured");
    let invalidation = captured.editor_revision_watch();

    assert!(
        captured
            .engine_client()
            .expect_err("unresolved route should be explicitly unavailable")
            .contains("still being resolved")
    );

    route.publish(Ok(None));
    assert!(captured.engine_client().is_err());
    assert!(invalidation.is_superseded());
    assert!(
        !captured.is_current(captured.document().target(), captured.editor_revision()),
        "route publication changes the applicable editor snapshot revision"
    );
    assert_eq!(
        state
            .document(Some(path))
            .expect("failed routing must retain editor state")
            .document()
            .text(),
        "fn retained() {}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn analysis_snapshot_accepts_matching_aliases_and_rejects_conflicts() {
    // Snapshot composition needs only a stable engine identity, so the test keeps the other end of
    // this transport alive without starting an engine service.
    let (client_transport, _server_transport) = tarpc::transport::channel::unbounded();
    let engine_service_client =
        EngineServiceClient::new(TarpcClientConfig::default(), client_transport).spawn();
    let engine_client = EngineClient::new(engine_service_client);
    let state = EditorStateHandle::default();
    let primary_path = PathBuf::from("/workspace/src/lib.rs");
    let alias_path = PathBuf::from("/workspace-link/src/lib.rs");
    let source_path = PathBuf::from("/canonical/workspace/src/lib.rs");

    for path in [&primary_path, &alias_path] {
        let opened = state.open(path.clone(), Some(1), "pub struct Shared;".to_string());
        let LifecycleEvent::Open { route, .. } = opened.event() else {
            panic!("expected open lifecycle event");
        };
        route.publish(Ok(Some(OpenDocumentRoute::new(
            engine_client.clone(),
            source_path.clone(),
        ))));
    }

    let captured = state
        .document(Some(primary_path.clone()))
        .expect("primary alias should be captured");
    let snapshot = captured
        .analysis_snapshot(&engine_client)
        .expect("matching aliases should form one coherent editor snapshot");
    assert_eq!(snapshot.editor().documents().len(), 2);

    assert!(
        state
            .change(&alias_path, Some(2), &[full("pub struct Conflicting;")],)
            .expect("full alias change should apply")
    );
    let captured = state
        .document(Some(primary_path))
        .expect("primary alias should remain captured");
    let error = captured
        .analysis_snapshot(&engine_client)
        .expect_err("conflicting aliases must not use last-writer-wins composition");
    assert!(error.reason().contains("conflicting text"));
}

#[test]
fn save_proposal_keeps_the_revision_seen_at_ingress() {
    let path = PathBuf::from("/workspace/src/lib.rs");
    let state = EditorStateHandle::default();
    state.open(path.clone(), Some(1), "fn saved() {}".to_string());
    let saved = state
        .save(&path, Some("fn saved() {}".to_string()))
        .expect("open document should create a save proposal");
    assert!(
        state
            .change(&path, Some(2), &[full("fn later() {}")])
            .expect("full change should apply")
    );

    let LifecycleEvent::Save { proposal, .. } = saved.event() else {
        panic!("expected save proposal event");
    };
    assert_eq!(proposal.target().revision(), DocumentRevision::new(1));
    assert_eq!(proposal.text(), "fn saved() {}");
    assert_eq!(
        state
            .document(Some(path))
            .expect("later document should remain available")
            .document()
            .text(),
        "fn later() {}"
    );
}

#[test]
fn save_without_text_proposes_the_exact_current_editor_value() {
    let path = PathBuf::from("/workspace/src/lib.rs");
    let state = EditorStateHandle::default();
    state.open(path.clone(), Some(7), "fn current() {}".to_string());

    let saved = state
        .save(&path, None)
        .expect("open document should save without notification text");
    let LifecycleEvent::Save { proposal, .. } = saved.event() else {
        panic!("expected save proposal event");
    };

    assert_eq!(proposal.text(), "fn current() {}");
    assert_eq!(proposal.client_version(), Some(7));
    assert_eq!(proposal.target().revision(), DocumentRevision::new(1));
}

#[test]
fn captured_target_is_invalidated_only_by_the_editor_owner() {
    let path = PathBuf::from("/workspace/src/lib.rs");
    let state = EditorStateHandle::default();
    state.open(path.clone(), Some(1), "fn first() {}".to_string());
    let captured = state
        .document(Some(path.clone()))
        .expect("opened document should be captured");
    let invalidation = captured.editor_revision_watch();

    assert!(captured.is_current(captured.document().target(), captured.editor_revision()));
    assert!(!invalidation.is_superseded());
    assert!(
        state
            .change(&path, Some(2), &[full("fn second() {}")])
            .expect("full change should apply")
    );
    assert!(invalidation.is_superseded());
    assert!(!captured.is_current(captured.document().target(), captured.editor_revision()));
}

#[test]
fn sibling_edit_invalidates_a_captured_complete_editor_revision() {
    let target = PathBuf::from("/workspace/src/lib.rs");
    let sibling = PathBuf::from("/workspace/src/sibling.rs");
    let state = EditorStateHandle::default();
    state.open(target.clone(), Some(1), "mod sibling;".to_string());
    state.open(sibling.clone(), Some(1), "pub struct First;".to_string());
    let captured = state
        .document(Some(target))
        .expect("target document should be captured");
    let invalidation = captured.editor_revision_watch();

    assert!(
        state
            .change(&sibling, Some(2), &[full("pub struct Second;")])
            .expect("full sibling change should apply")
    );
    assert!(invalidation.is_superseded());
    assert!(
        !captured.is_current(captured.document().target(), captured.editor_revision()),
        "a sibling change must supersede results from the older complete snapshot"
    );
}

#[test]
fn close_and_reopen_invalidates_the_captured_session_signal() {
    let path = PathBuf::from("/workspace/src/lib.rs");
    let state = EditorStateHandle::default();
    state.open(path.clone(), Some(7), "fn old_session() {}".to_string());
    let captured = state
        .document(Some(path.clone()))
        .expect("opened document should be captured");
    let invalidation = captured.editor_revision_watch();

    state.close(&path).expect("open document should close");
    state.open(path, Some(1), "fn new_session() {}".to_string());

    assert!(invalidation.is_superseded());
    assert!(!captured.is_current(captured.document().target(), captured.editor_revision()));
}

#[test]
fn saved_diagnostics_require_exact_open_editor_text() {
    let path = PathBuf::from("/workspace/src/lib.rs");
    let state = EditorStateHandle::default();
    state.open(path.clone(), Some(7), "fn editor() {}".to_string());

    assert_eq!(
        state.diagnostics_publication(&path, Some("fn saved() {}")),
        DiagnosticsPublication::KeepVisible
    );
    assert_eq!(
        state.diagnostics_publication(&path, Some("fn editor() {}")),
        DiagnosticsPublication::Publish { version: Some(7) }
    );

    state.close(&path).expect("open document should close");
    assert_eq!(
        state.diagnostics_publication(&path, None),
        DiagnosticsPublication::Publish { version: None }
    );
}

#[test]
fn incremental_changes_keep_complete_text_and_rebase_with_right_affinity() {
    let path = PathBuf::from("/workspace/src/lib.rs");
    let state = EditorStateHandle::default();
    state.open(path.clone(), Some(1), "impl RwLo".to_string());
    let captured = state
        .document(Some(path.clone()))
        .expect("opened document should be captured");

    assert!(
        state
            .change(&path, Some(2), &[incremental((0, 9), (0, 9), "ck")],)
            .expect("incremental typing should apply")
    );
    let (recaptured, position) = captured
        .recapture_position(Position::new(0, 9))
        .expect("typed position should rebase");

    assert_eq!(recaptured.document().text(), "impl RwLock");
    assert_eq!(recaptured.document().client_version(), Some(2));
    assert_eq!(position, Position::new(0, 11));
}

#[test]
fn a_late_request_can_traverse_several_already_accepted_changes() {
    let path = PathBuf::from("/workspace/src/lib.rs");
    let state = EditorStateHandle::default();
    state.open(path.clone(), Some(1), "Hash".to_string());
    let captured = state
        .document(Some(path.clone()))
        .expect("opened document should be captured");

    for (version, character) in [(2, "M"), (3, "a"), (4, "p")] {
        let current_character = u32::try_from(version + 2).expect("fixture position should fit");
        assert!(
            state
                .change(
                    &path,
                    Some(version),
                    &[incremental(
                        (0, current_character),
                        (0, current_character),
                        character,
                    )],
                )
                .expect("incremental typing should apply")
        );
    }

    let (recaptured, position) = captured
        .recapture_position(Position::new(0, 4))
        .expect("the retained forward sequence should rebase late admission");
    assert_eq!(recaptured.document().text(), "HashMap");
    assert_eq!(position, Position::new(0, 7));
}

#[test]
fn sibling_change_refreshes_snapshot_without_moving_target_position() {
    let target = PathBuf::from("/workspace/src/lib.rs");
    let sibling = PathBuf::from("/workspace/src/sibling.rs");
    let state = EditorStateHandle::default();
    state.open(target.clone(), Some(1), "impl RwLo".to_string());
    state.open(sibling.clone(), Some(1), "pub struct First;".to_string());
    let captured = state
        .document(Some(target))
        .expect("target document should be captured");

    assert!(
        state
            .change(
                &sibling,
                Some(2),
                &[incremental((0, 11), (0, 16), "Second")],
            )
            .expect("sibling change should apply")
    );
    let (recaptured, position) = captured
        .recapture_position(Position::new(0, 9))
        .expect("sibling edit should permit a fresh capture");

    assert_eq!(recaptured.document().text(), "impl RwLo");
    assert_eq!(position, Position::new(0, 9));
    assert_ne!(recaptured.editor_revision(), captured.editor_revision());
}

#[test]
fn full_replacement_never_reuses_an_unproven_numeric_position() {
    let path = PathBuf::from("/workspace/src/lib.rs");
    let state = EditorStateHandle::default();
    state.open(path.clone(), Some(1), "impl OldName".to_string());
    let captured = state
        .document(Some(path.clone()))
        .expect("opened document should be captured");

    assert!(
        state
            .change(&path, Some(2), &[full("impl NewName")])
            .expect("full replacement should materialize exact text")
    );
    let error = captured
        .recapture_position(Position::new(0, 12))
        .expect_err("full replacement should not guess a position mapping");

    assert!(matches!(error, PositionRecaptureError::Unavailable(_)));
    assert!(error.reason().contains("cannot be mapped"));
}

#[test]
fn recapture_never_crosses_a_close_and_reopen_boundary() {
    let path = PathBuf::from("/workspace/src/lib.rs");
    let state = EditorStateHandle::default();
    state.open(path.clone(), Some(1), "impl Old".to_string());
    let captured = state
        .document(Some(path.clone()))
        .expect("opened document should be captured");

    state.close(&path).expect("open document should close");
    state.open(path, Some(1), "impl New".to_string());

    assert!(matches!(
        captured.recapture_position(Position::new(0, 8)),
        Err(PositionRecaptureError::SessionEnded)
    ));
}

#[test]
fn forward_transformations_drop_with_the_oldest_capture() {
    let path = PathBuf::from("/workspace/src/lib.rs");
    let state = EditorStateHandle::default();
    state.open(path.clone(), Some(1), "RwLo".to_string());
    let captured = state
        .document(Some(path.clone()))
        .expect("opened document should be captured");
    let old_revision = Arc::downgrade(&captured.editor_revision);

    assert!(
        state
            .change(&path, Some(2), &[incremental((0, 4), (0, 4), "ck")],)
            .expect("incremental typing should apply")
    );
    assert!(old_revision.upgrade().is_some());

    drop(captured);
    assert!(
        old_revision.upgrade().is_none(),
        "the current editor node must not retain its historical predecessor"
    );
}

fn incremental(start: (u32, u32), end: (u32, u32), text: &str) -> TextDocumentContentChangeEvent {
    TextDocumentContentChangeEvent {
        range: Some(Range::new(
            Position::new(start.0, start.1),
            Position::new(end.0, end.1),
        )),
        range_length: None,
        text: text.to_string(),
    }
}

fn full(text: &str) -> TextDocumentContentChangeEvent {
    TextDocumentContentChangeEvent {
        range: None,
        range_length: None,
        text: text.to_string(),
    }
}
