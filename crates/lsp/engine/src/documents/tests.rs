use std::path::PathBuf;

use super::{DirtyDocumentSnapshotState, DocumentStore};

#[test]
fn tracks_clean_to_dirty_to_clean_document_lifecycle() {
    let path = PathBuf::from("/workspace/src/lib.rs");
    let mut store = DocumentStore::default();

    store.did_open(path.clone(), Some(1), "fn main() {}\n");
    assert!(!store.is_dirty(&path));

    let change = store.did_change(path.clone(), Some(2), Some("fn main() {\n}\n"));
    assert!(change.became_dirty);
    assert!(store.is_dirty(&path));

    let change = store.did_change(path.clone(), Some(3), Some("fn main() {\n}\n"));
    assert!(!change.became_dirty);
    assert!(store.is_dirty(&path));

    store.did_save(path.clone(), Some("fn main() {\n}\n"));
    assert!(!store.is_dirty(&path));

    store.mark_dirty_after_failed_save(path.clone());
    assert!(store.is_dirty(&path));

    store.did_close(&path);
    assert!(!store.is_dirty(&path));
}

#[test]
fn ignores_delayed_change_events_that_match_saved_text() {
    let path = PathBuf::from("/workspace/src/lib.rs");
    let mut store = DocumentStore::default();

    store.did_open(path.clone(), Some(1), "fn main() {}\n");
    store.did_save(path.clone(), Some("fn main() {\n}\n"));
    assert!(!store.is_dirty(&path));

    let change = store.did_change(path.clone(), Some(2), Some("fn main() {\n}\n"));
    assert!(!change.became_dirty);
    assert!(!store.is_dirty(&path));
}

#[test]
fn ignores_out_of_order_older_change_versions() {
    let path = PathBuf::from("/workspace/src/lib.rs");
    let mut store = DocumentStore::default();

    store.did_open(path.clone(), Some(1), "fn main() {}\n");
    store.did_change(path.clone(), Some(3), Some("fn main() {\n    work();\n}\n"));
    store.did_save(path.clone(), Some("fn main() {\n    work();\n}\n"));
    assert!(!store.is_dirty(&path));

    let change = store.did_change(path.clone(), Some(2), Some("fn main() {\n}\n"));
    assert!(!change.became_dirty);
    assert!(!change.became_clean);
    assert!(!store.is_dirty(&path));
}

#[test]
fn save_keeps_document_dirty_when_a_newer_edit_already_landed() {
    let path = PathBuf::from("/workspace/src/lib.rs");
    let mut store = DocumentStore::default();

    store.did_open(path.clone(), Some(1), "fn main() {}\n");
    store.did_change(
        path.clone(),
        Some(3),
        Some("fn main() {\n    unsaved();\n}\n"),
    );

    store.did_save(path.clone(), Some("fn main() {\n    saved();\n}\n"));
    assert!(store.is_dirty(&path));
}

#[test]
fn external_saved_change_updates_clean_open_document() {
    let path = PathBuf::from("/workspace/src/lib.rs");
    let mut store = DocumentStore::default();

    store.did_open(path.clone(), Some(1), "fn main() {}\n");
    store.external_saved_change(path.clone(), "fn main() {\n    external();\n}\n");

    let freshness = store.freshness(&path);
    assert!(!store.is_dirty(&path));
    assert_eq!(
        freshness.saved_len(),
        Some("fn main() {\n    external();\n}\n".len())
    );
    assert_eq!(
        freshness.live_len(),
        Some("fn main() {\n    external();\n}\n".len())
    );
}

#[test]
fn external_saved_change_keeps_dirty_live_text() {
    let path = PathBuf::from("/workspace/src/lib.rs");
    let mut store = DocumentStore::default();

    store.did_open(path.clone(), Some(1), "fn main() {}\n");
    store.did_change(
        path.clone(),
        Some(2),
        Some("fn main() {\n    unsaved();\n}\n"),
    );
    store.external_saved_change(path.clone(), "fn main() {\n    external();\n}\n");

    let DirtyDocumentSnapshotState::Dirty(snapshot) = store.dirty_snapshot(&path) else {
        panic!("dirty document should keep exposing its live editor snapshot");
    };
    assert_eq!(snapshot.text(), "fn main() {\n    unsaved();\n}\n");
}

#[test]
fn external_saved_change_cleans_dirty_document_that_matches_disk() {
    let path = PathBuf::from("/workspace/src/lib.rs");
    let mut store = DocumentStore::default();

    store.did_open(path.clone(), Some(1), "fn main() {}\n");
    store.did_change(
        path.clone(),
        Some(2),
        Some("fn main() {\n    external();\n}\n"),
    );
    store.external_saved_change(path.clone(), "fn main() {\n    external();\n}\n");

    assert!(!store.is_dirty(&path));
}

#[test]
fn external_saved_change_keeps_unknown_dirty_buffer_dirty() {
    let path = PathBuf::from("/workspace/src/lib.rs");
    let mut store = DocumentStore::default();

    store.did_open(path.clone(), Some(1), "fn main() {}\n");
    store.did_change(path.clone(), Some(2), None);
    store.external_saved_change(path.clone(), "fn main() {\n    external();\n}\n");

    assert!(store.is_dirty(&path));
    assert!(matches!(
        store.dirty_snapshot(&path),
        DirtyDocumentSnapshotState::DirtyWithoutText
    ));
}

#[test]
fn exposes_current_text_for_clean_and_dirty_full_sync_documents() {
    let path = PathBuf::from("/workspace/src/lib.rs");
    let mut store = DocumentStore::default();

    store.did_open(path.clone(), Some(1), "fn main() {}\n");
    assert_eq!(store.current_text(&path).as_deref(), Some("fn main() {}\n"));

    store.did_change(path.clone(), Some(2), Some("fn main() {\n    live();\n}\n"));
    assert_eq!(
        store.current_text(&path).as_deref(),
        Some("fn main() {\n    live();\n}\n")
    );
}

#[test]
fn current_text_is_absent_for_unknown_dirty_and_closed_documents() {
    let path = PathBuf::from("/workspace/src/lib.rs");
    let mut store = DocumentStore::default();

    assert_eq!(store.current_text(&path), None);

    store.did_open(path.clone(), Some(1), "fn main() {}\n");
    store.did_change(path.clone(), Some(2), None);
    assert_eq!(store.current_text(&path), None);

    store.did_close(&path);
    assert_eq!(store.current_text(&path), None);
}

#[test]
fn exposes_dirty_snapshot_when_full_live_text_is_available() {
    let path = PathBuf::from("/workspace/src/lib.rs");
    let mut store = DocumentStore::default();

    store.did_open(path.clone(), Some(1), "fn main() {}\n");
    store.did_change(path.clone(), Some(2), Some("fn main() {\n    live();\n}\n"));

    let DirtyDocumentSnapshotState::Dirty(snapshot) = store.dirty_snapshot(&path) else {
        panic!("dirty full-sync document should expose a dirty snapshot");
    };

    assert_eq!(snapshot.path(), path.as_path());
    assert_eq!(snapshot.version(), Some(2));
    assert_eq!(snapshot.text(), "fn main() {\n    live();\n}\n");
}
