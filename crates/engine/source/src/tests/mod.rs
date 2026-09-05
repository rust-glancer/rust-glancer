//! Cross-module tests for the source-generation lifecycle.

use std::fs;

use crate::{CapturedSource, SourceError, SourceInventory, SourceRevision};
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
    let with_existence_probe = inventory.memory_size();
    inventory.seal();

    inventory
        .validate_saved()
        .expect("unchanged source observations should validate");
    let after_validation = inventory.memory_size();

    assert!(
        after_validation < with_existence_probe,
        "successful validation should release existence-map keys and capacity",
    );
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
fn evicted_known_source_reports_disappearance_as_generation_invalidation() {
    let dir = tempfile::tempdir().expect("temporary source directory should be created");
    let path = dir.path().join("lib.rs");
    fs::write(&path, "pub struct Before;\n").expect("fixture source should be written");

    let inventory = SourceInventory::new();
    let entry = inventory
        .capture_saved(&path)
        .expect("fixture source should be captured");
    inventory.seal();
    inventory.evict_saved_text();
    fs::remove_file(&path).expect("fixture source should be removed");

    let error = entry
        .text()
        .expect_err("a missing known source should invalidate the generation");
    assert!(matches!(error, SourceError::Missing { .. }));
    assert_eq!(error.stale_path(), Some(entry.path()));
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
    let source = CapturedSource::new(published_entry.path(), "pub struct Updated;\n")
        .expect("updated source should keep the canonical path identity");
    candidate
        .replace_saved(&source)
        .expect("candidate source should be replaced");

    assert_eq!(
        published_entry
            .text()
            .expect("published source should remain readable")
            .as_ref(),
        "pub struct Saved;\n"
    );
}

#[test]
fn captured_saved_source_validation_checks_disk_without_replacing_captured_text() {
    let dir = tempfile::tempdir().expect("temporary source directory should be created");
    let path = dir.path().join("lib.rs");
    let text = "pub struct Capturé;\n";
    fs::write(&path, text).expect("fixture source should be written");
    let change = CapturedSource::new(&path, text).expect("saved source should capture");

    assert_eq!(
        change.path(),
        path.canonicalize()
            .expect("fixture path should canonicalize")
    );
    assert_eq!(change.text(), text);
    assert_eq!(
        change.revision(),
        SourceRevision::from_bytes(text.as_bytes())
    );
    assert_eq!(change.byte_len(), text.len() as u64);
    assert_ne!(text.len(), text.chars().count());

    let inventory = SourceInventory::new();
    let entry = inventory
        .replace_saved(&change)
        .expect("captured source should enter an open inventory");

    fs::write(&path, "pub struct NewerDisk;\n").expect("fixture source should advance");
    let error = inventory
        .validate_saved()
        .expect_err("newer disk source should reject the captured candidate");

    assert!(matches!(error, SourceError::Stale { .. }));
    assert_eq!(
        entry
            .text()
            .expect("resident captured text should remain readable")
            .as_ref(),
        text
    );
}
