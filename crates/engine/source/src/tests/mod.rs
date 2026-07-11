//! Cross-module tests for the source-generation lifecycle.

use std::fs;

use crate::{SourceError, SourceInventory};
use rg_std::MemorySize as _;

#[test]
fn saved_source_memory_disappears_after_eviction() {
    let dir = tempfile::tempdir().expect("temporary source directory should be created");
    let path = dir.path().join("lib.rs");
    let text = "pub struct ResidentSourceText;\n";
    fs::write(&path, text).expect("fixture source should be written");

    let inventory = SourceInventory::new();
    let entry = inventory
        .capture_saved(&path)
        .expect("fixture source should be captured");
    let resident = entry.memory_size();

    inventory.evict_saved_text();
    let evicted = entry.memory_size();

    assert!(
        resident >= evicted + text.len(),
        "resident accounting should include saved source text",
    );
}

#[test]
fn dirty_source_memory_survives_saved_text_eviction() {
    let dir = tempfile::tempdir().expect("temporary source directory should be created");
    let path = dir.path().join("lib.rs");
    fs::write(&path, "pub struct Saved;\n").expect("fixture source should be written");
    let path = path
        .canonicalize()
        .expect("fixture path should canonicalize");

    let inventory = SourceInventory::new();
    let entry = inventory
        .replace_in_memory(&path, "pub struct DirtySourceText;\n")
        .expect("dirty source should be captured");
    let before_eviction = entry.memory_size();

    inventory.evict_saved_text();

    assert_eq!(
        entry.memory_size(),
        before_eviction,
        "dirty text is the overlay authority and must remain resident",
    );
}

#[test]
fn successful_validation_discards_existence_probes() {
    let dir = tempfile::tempdir().expect("temporary source directory should be created");
    let source_path = dir.path().join("lib.rs");
    let missing_module_path = dir.path().join("missing.rs");
    fs::write(&source_path, "mod missing;\n").expect("fixture source should be written");

    let inventory = SourceInventory::new();
    inventory
        .capture_saved(&source_path)
        .expect("fixture source should be captured");
    assert!(
        !inventory
            .probe_exists(&missing_module_path)
            .expect("module existence should be captured")
    );
    inventory.seal();

    inventory
        .validate_saved()
        .expect("unchanged source observations should validate");

    assert!(matches!(
        inventory.probe_exists(&missing_module_path),
        Err(SourceError::Sealed { .. })
    ));
}

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
