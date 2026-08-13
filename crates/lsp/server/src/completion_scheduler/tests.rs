use std::{
    future::pending,
    path::{Path, PathBuf},
};

use rg_lsp_proto::{AnalysisInput, AnalysisOutcome, AnalysisReady};
use tokio::sync::{mpsc, oneshot};
use tower_lsp_server::ls_types::{Position, Range, TextDocumentContentChangeEvent};

use super::{
    CompletionAttemptOutcome, CompletionRequest, CompletionScheduler,
    attempt::{AttemptKey, AttemptWaiter},
    request::CompletionFuture,
};
use crate::ingress::{CapturedDocument, EditorStateHandle};

#[tokio::test(flavor = "current_thread")]
async fn editor_advance_is_an_attempt_transition_not_an_engine_abort() {
    let scheduler = CompletionScheduler::default();
    let (editor, path, captured) = open_document();
    let request = scheduler.capture_request(&captured, Position::new(0, 20));
    let (started, mut starts) = mpsc::unbounded_channel();
    let response = enqueue(
        &request,
        &captured,
        20,
        observed_job(1, started, Box::pin(pending())),
    );
    assert_eq!(starts.recv().await, Some(1));

    change(&editor, &path, 20, "Changed");

    assert_editor_advanced(response.wait().await);
    drop(request);
    assert_eq!(scheduler.session_count(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn late_admission_returns_editor_advance_without_starting_semantic_work() {
    let scheduler = CompletionScheduler::default();
    let (editor, path, captured) = open_document();
    let request = scheduler.capture_request(&captured, Position::new(0, 20));
    let (started, mut starts) = mpsc::unbounded_channel();
    change(&editor, &path, 20, "Changed");

    let response = enqueue(
        &request,
        &captured,
        20,
        observed_job(1, started, Box::pin(pending())),
    );

    assert_editor_advanced(response.wait().await);
    assert!(starts.try_recv().is_err());
    drop(request);
    assert_eq!(scheduler.session_count(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn newer_ingress_request_prevents_an_older_handler_from_reentering() {
    let scheduler = CompletionScheduler::default();
    let (editor, path, old_capture) = open_document();
    let old_request = scheduler.capture_request(&old_capture, Position::new(0, 20));
    change(&editor, &path, 20, "Changed");
    let current = editor
        .document(Some(path))
        .expect("changed document should be captured");
    let newer_request = scheduler.capture_request(&current, Position::new(0, 27));
    let (started, mut starts) = mpsc::unbounded_channel();

    let old_response = enqueue(
        &old_request,
        &current,
        27,
        observed_job(1, started.clone(), Box::pin(pending())),
    );
    let current_response = enqueue(
        &newer_request,
        &current,
        27,
        observed_job(2, started, Box::pin(async {})),
    );

    assert_replaced(old_response.wait().await);
    assert_eq!(starts.recv().await, Some(2));
    assert_completed(current_response.wait().await);
    assert!(starts.try_recv().is_err());
    drop(old_request);
    drop(newer_request);
    assert_eq!(scheduler.session_count(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn distinct_newer_request_replaces_active_and_pending_work() {
    let scheduler = CompletionScheduler::default();
    let (_editor, _path, captured) = open_document();
    let first_request = scheduler.capture_request(&captured, Position::new(0, 1));
    let (started, mut starts) = mpsc::unbounded_channel();
    let first = enqueue(
        &first_request,
        &captured,
        1,
        observed_job(1, started.clone(), Box::pin(pending())),
    );
    assert_eq!(starts.recv().await, Some(1));

    let second_request = scheduler.capture_request(&captured, Position::new(0, 2));
    let second = enqueue(
        &second_request,
        &captured,
        2,
        observed_job(2, started.clone(), Box::pin(pending())),
    );
    let third_request = scheduler.capture_request(&captured, Position::new(0, 3));
    let third = enqueue(
        &third_request,
        &captured,
        3,
        observed_job(3, started, Box::pin(async {})),
    );

    assert_replaced(first.wait().await);
    assert_replaced(second.wait().await);
    assert_eq!(starts.recv().await, Some(3));
    assert_completed(third.wait().await);
    assert!(starts.try_recv().is_err());
    drop(first_request);
    drop(second_request);
    drop(third_request);
    assert_eq!(scheduler.session_count(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn exact_duplicate_requests_share_the_same_active_attempt() {
    let scheduler = CompletionScheduler::default();
    let (_editor, _path, captured) = open_document();
    let first_request = scheduler.capture_request(&captured, Position::new(0, 5));
    let duplicate_request = scheduler.capture_request(&captured, Position::new(0, 5));
    assert_eq!(
        first_request.id(),
        duplicate_request.id(),
        "wire duplicates should share logical ownership"
    );
    let (started, mut starts) = mpsc::unbounded_channel();
    let (finish, finished) = oneshot::channel();
    let first = enqueue(
        &first_request,
        &captured,
        5,
        observed_job(
            1,
            started.clone(),
            Box::pin(async move {
                let _ = finished.await;
            }),
        ),
    );
    assert_eq!(starts.recv().await, Some(1));
    let duplicate = enqueue(
        &duplicate_request,
        &captured,
        5,
        observed_job(2, started, Box::pin(pending())),
    );

    finish.send(()).expect("shared attempt should finish");
    assert_completed(first.wait().await);
    assert_completed(duplicate.wait().await);
    assert!(starts.try_recv().is_err(), "duplicate work must not start");
    drop(first_request);
    drop(duplicate_request);
    assert_eq!(scheduler.session_count(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn a_new_attempt_for_the_same_request_advances_duplicate_waiters() {
    let scheduler = CompletionScheduler::default();
    let (editor, path, captured) = open_document();
    let request = scheduler.capture_request(&captured, Position::new(0, 20));
    let duplicate = request.clone();
    let (started, mut starts) = mpsc::unbounded_channel();
    let old = enqueue(
        &request,
        &captured,
        20,
        observed_job(1, started.clone(), Box::pin(pending())),
    );
    assert_eq!(starts.recv().await, Some(1));
    change(&editor, &path, 20, "Changed");
    let current = editor
        .document(Some(path))
        .expect("changed document should be captured");
    let new = enqueue(
        &duplicate,
        &current,
        27,
        observed_job(2, started, Box::pin(async {})),
    );

    assert_editor_advanced(old.wait().await);
    assert_eq!(starts.recv().await, Some(2));
    assert_completed(new.wait().await);
    drop(request);
    drop(duplicate);
    assert_eq!(scheduler.session_count(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn dropping_every_waiter_cancels_work_and_releases_the_session_queue() {
    let scheduler = CompletionScheduler::default();
    let (_editor, _path, captured) = open_document();
    let request = scheduler.capture_request(&captured, Position::new(0, 1));
    let (started, started_rx) = oneshot::channel();
    let (dropped, dropped_rx) = oneshot::channel();
    let response = request.enqueue_attempt(
        AttemptKey::for_capture(&captured, Position::new(0, 1)),
        captured.editor_revision_watch(),
        Box::pin(async move {
            let _notice = DropNotice(Some(dropped));
            started
                .send(())
                .expect("test should observe the semantic attempt start");
            pending().await
        }),
    );
    started_rx
        .await
        .expect("semantic attempt should start before its waiter closes");

    drop(response);
    drop(request);
    dropped_rx
        .await
        .expect("closed waiter should drop the semantic future");

    assert_eq!(scheduler.session_count(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn close_and_reopen_ends_the_old_request_and_releases_its_session_queue() {
    let scheduler = CompletionScheduler::default();
    let (editor, path, captured) = open_document();
    let request = scheduler.capture_request(&captured, Position::new(0, 20));
    let (started, mut starts) = mpsc::unbounded_channel();
    let response = enqueue(
        &request,
        &captured,
        20,
        observed_job(1, started, Box::pin(pending())),
    );
    assert_eq!(starts.recv().await, Some(1));

    editor
        .close(&path)
        .expect("the original document session should close");
    editor.open(path, Some(1), "pub struct Reopened;".to_string());

    assert_editor_advanced(response.wait().await);
    let error = captured
        .recapture_position(Position::new(0, 20))
        .expect_err("an old completion request must not cross into the reopened session");
    assert_eq!(error.reason(), "the captured document session has ended");
    drop(request);
    assert_eq!(scheduler.session_count(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn engine_abort_and_failure_remain_completed_attempt_outcomes() {
    let scheduler = CompletionScheduler::default();
    let (_editor, _path, captured) = open_document();

    let aborted_request = scheduler.capture_request(&captured, Position::new(0, 1));
    let aborted = aborted_request.enqueue_attempt(
        AttemptKey::for_capture(&captured, Position::new(0, 1)),
        captured.editor_revision_watch(),
        Box::pin(async {
            Ok(AnalysisOutcome::Aborted(
                rg_lsp_proto::AnalysisAbort::SourceChanged,
            ))
        }),
    );
    let CompletionAttemptOutcome::Completed(Ok(AnalysisOutcome::Aborted(abort))) =
        aborted.wait().await
    else {
        panic!("engine abort should remain a completed semantic attempt");
    };
    assert_eq!(abort, rg_lsp_proto::AnalysisAbort::SourceChanged);
    drop(aborted_request);

    let failed_request = scheduler.capture_request(&captured, Position::new(0, 2));
    let failed = failed_request.enqueue_attempt(
        AttemptKey::for_capture(&captured, Position::new(0, 2)),
        captured.editor_revision_watch(),
        Box::pin(async { Err(anyhow::anyhow!("semantic failure")) }),
    );
    let CompletionAttemptOutcome::Completed(Err(error)) = failed.wait().await else {
        panic!("engine failure should remain a completed semantic attempt");
    };
    assert!(error.to_string().contains("semantic failure"));
    drop(failed_request);
    assert_eq!(scheduler.session_count(), 0);
}

fn open_document() -> (EditorStateHandle, PathBuf, CapturedDocument) {
    let path = PathBuf::from("/workspace/src/lib.rs");
    let editor = EditorStateHandle::default();
    editor.open(path.clone(), Some(1), "pub struct Original;".to_string());
    let captured = editor
        .document(Some(path.clone()))
        .expect("opened document should be captured");
    (editor, path, captured)
}

fn enqueue(
    request: &CompletionRequest,
    captured: &CapturedDocument,
    character: u32,
    run: CompletionFuture,
) -> AttemptWaiter {
    request.enqueue_attempt(
        AttemptKey::for_capture(captured, Position::new(0, character)),
        captured.editor_revision_watch(),
        run,
    )
}

fn observed_job(
    label: u64,
    started: mpsc::UnboundedSender<u64>,
    wait: PinBoxFuture,
) -> CompletionFuture {
    Box::pin(async move {
        started
            .send(label)
            .expect("test should observe every started completion");
        wait.await;
        Ok(AnalysisOutcome::Ready(AnalysisReady::new(
            Vec::new(),
            AnalysisInput::for_saved_project(label),
        )))
    })
}

fn change(editor: &EditorStateHandle, path: &Path, character: u32, text: &str) {
    assert!(
        editor
            .change(
                path,
                Some(2),
                &[TextDocumentContentChangeEvent {
                    range: Some(Range::new(
                        Position::new(0, character),
                        Position::new(0, character),
                    )),
                    range_length: None,
                    text: text.to_string(),
                }],
            )
            .expect("incremental change should apply")
    );
}

fn assert_completed(outcome: CompletionAttemptOutcome) {
    assert!(matches!(
        outcome,
        CompletionAttemptOutcome::Completed(Ok(AnalysisOutcome::Ready(_)))
    ));
}

fn assert_editor_advanced(outcome: CompletionAttemptOutcome) {
    assert!(matches!(outcome, CompletionAttemptOutcome::EditorAdvanced));
}

fn assert_replaced(outcome: CompletionAttemptOutcome) {
    assert!(matches!(outcome, CompletionAttemptOutcome::Replaced));
}

type PinBoxFuture = std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>;

struct DropNotice(Option<oneshot::Sender<()>>);

impl Drop for DropNotice {
    fn drop(&mut self) {
        if let Some(notice) = self.0.take() {
            let _ = notice.send(());
        }
    }
}
