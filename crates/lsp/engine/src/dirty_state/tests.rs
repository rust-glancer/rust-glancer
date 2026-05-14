use std::path::PathBuf;

use super::{DirtyDocumentIdentity, DirtyState};
use crate::documents::{DirtyDocumentSnapshotState, DocumentStore};

#[test]
fn dirty_state_rejects_old_dirty_snapshots() {
    let path = PathBuf::from("/workspace/src/lib.rs");
    let mut store = DocumentStore::default();
    let dirty_state = DirtyState::default();

    store.did_open(path.clone(), Some(1), "fn main() {}\n");
    dirty_state.sync_document(&path, &store.dirty_snapshot(&path));

    store.did_change(path.clone(), Some(2), Some("fn main() {\n    v2();\n}\n"));
    let snapshot_v2 = store.dirty_snapshot(&path);
    dirty_state.sync_document(&path, &snapshot_v2);
    let DirtyDocumentSnapshotState::Dirty(snapshot_v2) = snapshot_v2 else {
        panic!("dirty full-sync document should expose version 2 snapshot");
    };
    assert!(dirty_state.is_current_identity(&DirtyDocumentIdentity::from_snapshot(&snapshot_v2)));

    store.did_change(path.clone(), Some(3), Some("fn main() {\n    v3();\n}\n"));
    let snapshot_v3 = store.dirty_snapshot(&path);
    dirty_state.sync_document(&path, &snapshot_v3);
    let DirtyDocumentSnapshotState::Dirty(snapshot_v3) = snapshot_v3 else {
        panic!("dirty full-sync document should expose version 3 snapshot");
    };
    assert!(!dirty_state.is_current_identity(&DirtyDocumentIdentity::from_snapshot(&snapshot_v2)));
    assert!(dirty_state.is_current_identity(&DirtyDocumentIdentity::from_snapshot(&snapshot_v3)));

    store.did_save(path.clone(), Some("fn main() {\n    v3();\n}\n"));
    dirty_state.sync_document(&path, &store.dirty_snapshot(&path));
    assert!(!dirty_state.is_current_identity(&DirtyDocumentIdentity::from_snapshot(&snapshot_v3)));
}
