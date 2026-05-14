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
