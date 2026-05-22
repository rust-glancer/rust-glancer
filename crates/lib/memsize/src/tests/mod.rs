use std::{any, collections::BTreeMap};

use crate::{MemoryRecordKind, MemoryRecorder, MemoryRecorderMode};

#[test]
fn recorder_keeps_scoped_paths_and_totals() {
    let mut recorder = MemoryRecorder::new("project");
    recorder.scope("parse", |recorder| {
        recorder.record_heap::<String>(40);
        recorder.scope("files", |recorder| recorder.record_shallow::<usize>(2));
    });
    recorder.scope("body_ir", |recorder| {
        recorder.record_spare_capacity::<Vec<u8>>(8)
    });

    let totals = recorder.totals_by_path();
    assert_eq!(totals.get("project.parse"), Some(&40));
    assert_eq!(totals.get("project.parse.files"), Some(&2));
    assert_eq!(totals.get("project.body_ir"), Some(&8));
    assert_eq!(recorder.total_bytes(), 50);
}

#[test]
fn recorder_summarizes_by_kind() {
    let mut recorder = MemoryRecorder::new("root");
    recorder.record_shallow::<usize>(3);
    recorder.record_heap::<String>(5);
    recorder.record_heap::<Vec<u8>>(7);

    let mut expected = BTreeMap::new();
    expected.insert(MemoryRecordKind::Shallow, 3);
    expected.insert(MemoryRecordKind::Heap, 12);

    assert_eq!(recorder.totals_by_kind(), expected);
}

#[test]
fn recorder_attaches_type_names_to_records() {
    let mut recorder = MemoryRecorder::new("root");
    recorder.record_shallow::<usize>(8);
    recorder.record_heap::<String>(13);

    let records = recorder.records();
    assert!(
        records
            .iter()
            .any(|record| record.type_name == any::type_name::<usize>())
    );
    assert!(
        records
            .iter()
            .any(|record| record.type_name == any::type_name::<String>())
    );

    let totals = recorder.totals_by_type();
    assert_eq!(totals.get(any::type_name::<usize>()), Some(&8));
    assert_eq!(totals.get(any::type_name::<String>()), Some(&13));
}

#[test]
fn recorder_can_attach_custom_type_names() {
    let mut recorder = MemoryRecorder::new("root");
    recorder.record_type_name(MemoryRecordKind::Approximate, "example::TokenData", 21);

    let records = recorder.records();
    let record = &records[0];
    assert_eq!(record.type_name, "example::TokenData");
    assert_eq!(record.bytes, 21);
}

#[test]
fn recorder_aggregates_duplicate_contributions_by_default() {
    let mut recorder = MemoryRecorder::new("root");
    recorder.record_heap::<String>(5);
    recorder.record_heap::<String>(7);

    let records = recorder.records();
    assert_eq!(recorder.mode(), MemoryRecorderMode::Aggregate);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].bytes, 12);
    assert_eq!(recorder.raw_records(), None);
}

#[test]
fn detailed_recorder_keeps_raw_contributions() {
    let mut recorder = MemoryRecorder::detailed("root");
    recorder.record_heap::<String>(5);
    recorder.record_heap::<String>(7);

    let records = recorder.records();
    let raw_records = recorder
        .raw_records()
        .expect("detailed recorder should keep raw records");

    assert_eq!(recorder.mode(), MemoryRecorderMode::Detailed);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].bytes, 12);
    assert_eq!(raw_records.len(), 2);
    assert_eq!(raw_records[0].bytes, 5);
    assert_eq!(raw_records[1].bytes, 7);
}

#[test]
fn total_only_recorder_keeps_totals_without_records() {
    let mut recorder = MemoryRecorder::total_only("root");
    recorder.scope("ignored", |recorder| {
        recorder.record_heap::<String>(5);
        recorder.record_heap::<Vec<u8>>(7);
    });

    assert_eq!(recorder.mode(), MemoryRecorderMode::TotalOnly);
    assert_eq!(recorder.total_bytes(), 12);
    assert!(recorder.records().is_empty());
    assert!(recorder.totals_by_path().is_empty());
}
