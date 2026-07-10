//! Cross-module tests for the source-generation lifecycle.

use std::fs;

use crate::{SourceError, SourceInventory};

#[test]
fn evicted_saved_text_rejects_changed_disk_bytes() {
    let dir = tempfile::tempdir().expect("temporary source directory should be created");
    let path = dir.path().join("lib.rs");
    fs::write(&path, "pub struct Before;\n").expect("fixture source should be written");

    let inventory = SourceInventory::new();
    let entry = inventory
        .capture_saved(&path)
        .expect("fixture source should be captured");
    inventory.seal();
    inventory.evict_saved_text();
    fs::write(&path, "pub struct After;\n").expect("fixture source should be replaced");

    let error = entry.text().expect_err("changed source should be rejected");
    assert!(matches!(error, SourceError::Stale { .. }));
}

#[test]
fn candidate_fork_does_not_replace_published_source() {
    let dir = tempfile::tempdir().expect("temporary source directory should be created");
    let path = dir.path().join("lib.rs");
    fs::write(&path, "pub struct Saved;\n").expect("fixture source should be written");

    let published = SourceInventory::new();
    let published_entry = published
        .capture_saved(&path)
        .expect("fixture source should be captured");
    published.seal();

    let candidate = published.fork();
    candidate.begin_capture();
    candidate
        .replace_in_memory(
            &path.canonicalize().expect("path should canonicalize"),
            "dirty",
        )
        .expect("candidate source should be replaced");

    assert_eq!(
        published_entry
            .text()
            .expect("published source should remain readable")
            .as_ref(),
        "pub struct Saved;\n"
    );
}
