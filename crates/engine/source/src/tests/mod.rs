//! Cross-module tests for the source-generation lifecycle.

use std::fs;

use rg_std::MemorySize as _;

use crate::{CapturedSource, SourceEntry, SourceError, SourceRevision, read_source_text};

#[test]
fn saved_source_memory_disappears_after_eviction() {
    let dir = tempfile::tempdir().expect("temporary source directory should be created");
    let path = dir.path().join("lib.rs");
    let text = "pub struct ResidentSourceText;\n";
    fs::write(&path, text).expect("fixture source should be written");

    let text = read_source_text(&path).expect("fixture source should be read");
    let entry = SourceEntry::saved(path, text);
    let len = entry.byte_len() as usize;
    let resident = entry.memory_size();

    entry.evict_saved_text();
    let evicted = entry.memory_size();

    assert!(
        resident >= evicted + len,
        "resident accounting should include saved source text",
    );
}

#[test]
fn evicted_saved_text_rejects_changed_disk_bytes() {
    let dir = tempfile::tempdir().expect("temporary source directory should be created");
    let path = dir.path().join("lib.rs");
    fs::write(&path, "pub struct Before;\n").expect("fixture source should be written");

    let text = read_source_text(&path).expect("fixture source should be read");
    let entry = SourceEntry::saved(path.clone(), text);
    entry.evict_saved_text();
    fs::write(&path, "pub struct After;\n").expect("fixture source should be replaced");

    let error = entry.text().expect_err("changed source should be rejected");
    assert!(matches!(error, SourceError::Stale { .. }));
}

#[test]
fn evicted_known_source_reports_disappearance_as_generation_invalidation() {
    let dir = tempfile::tempdir().expect("temporary source directory should be created");
    let path = dir.path().join("lib.rs");
    fs::write(&path, "pub struct Before;\n").expect("fixture source should be written");

    let text = read_source_text(&path).expect("fixture source should be read");
    let entry = SourceEntry::saved(path.clone(), text);
    entry.evict_saved_text();
    fs::remove_file(&path).expect("fixture source should be removed");

    let error = entry
        .text()
        .expect_err("a missing known source should invalidate the generation");
    assert!(matches!(error, SourceError::Missing { .. }));
    assert_eq!(error.stale_path(), Some(entry.path()));
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

    let entry = crate::SourceEntry::saved_captured(&change);

    fs::write(&path, "pub struct NewerDisk;\n").expect("fixture source should advance");
    let error = entry
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
