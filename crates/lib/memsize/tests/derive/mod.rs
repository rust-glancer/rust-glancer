use std::{collections::BTreeMap, marker::PhantomData, mem};

use rg_memsize::{MemoryRecordKind, MemoryRecorder, MemorySize};

#[derive(rg_memsize::MemorySize)]
#[allow(dead_code)]
struct ProjectMemory {
    name: String,
    #[memsize(scope = "roots")]
    target_roots: Vec<String>,
    #[memsize(skip)]
    scratch: String,
}

#[test]
fn derive_records_struct_fields_under_field_scopes() {
    let value = ProjectMemory {
        name: "core".to_owned(),
        target_roots: vec!["src/lib.rs".to_owned()],
        scratch: "temporary".to_owned(),
    };

    let mut recorder = MemoryRecorder::new("project");
    value.record_memory_children(&mut recorder);

    let totals = recorder.totals_by_path();
    assert!(totals.contains_key("project.name"));
    assert!(totals.contains_key("project.roots.items"));
    assert!(!totals.keys().any(|path| path.contains("scratch")));
}

#[derive(rg_memsize::MemorySize)]
enum Resolution {
    Local(String),
    Pair {
        left: String,
        #[memsize(inline)]
        right: String,
    },
    #[memsize(scope = "unknown")]
    Unknown {
        #[memsize(scope = "payload", with = "record_unknown_payload")]
        payload: String,
    },
}

fn record_unknown_payload(payload: &String, recorder: &mut MemoryRecorder) {
    recorder.record_approximate::<String>(payload.len());
}

#[test]
fn derive_keeps_single_field_enum_variants_transparent() {
    let mut recorder = MemoryRecorder::new("resolution");
    Resolution::Local("binding".to_owned()).record_memory_children(&mut recorder);

    let totals = recorder.totals_by_path();
    assert!(totals.contains_key("resolution"));
    assert!(!totals.contains_key("resolution.0"));
}

#[test]
fn derive_supports_field_and_variant_scoping_overrides() {
    let mut recorder = MemoryRecorder::new("resolution");
    Resolution::Pair {
        left: "left".to_owned(),
        right: "right".to_owned(),
    }
    .record_memory_children(&mut recorder);

    let totals = recorder.totals_by_path();
    assert!(totals.contains_key("resolution.left"));
    assert!(totals.contains_key("resolution"));
    assert!(!totals.contains_key("resolution.right"));

    let mut recorder = MemoryRecorder::new("resolution");
    Resolution::Unknown {
        payload: "???".to_owned(),
    }
    .record_memory_children(&mut recorder);

    let totals = recorder.totals_by_path();
    assert_eq!(totals.get("resolution.unknown.payload"), Some(&3));
}

#[derive(rg_memsize::MemorySize)]
#[memsize(leaf)]
#[allow(dead_code)]
struct LeafId(String);

#[test]
fn derive_leaf_records_no_children() {
    let value = LeafId("opaque".to_owned());

    let mut recorder = MemoryRecorder::new("id");
    value.record_memory_children(&mut recorder);

    assert!(recorder.records().is_empty());

    value.record_memory_size(&mut recorder);
    assert_eq!(recorder.total_bytes(), mem::size_of::<LeafId>());
}

#[derive(rg_memsize::MemorySize)]
struct GenericRecord<T> {
    value: Option<T>,
    #[memsize(skip)]
    marker: PhantomData<T>,
}

#[derive(rg_memsize::MemorySize)]
#[memsize(no_auto_bound)]
struct CustomGenericRecord<T> {
    #[memsize(with = "record_custom_generic")]
    value: T,
}

fn record_custom_generic<T>(_value: &T, recorder: &mut MemoryRecorder) {
    recorder.record_approximate::<CustomGenericRecord<T>>(1);
}

#[test]
fn derive_handles_generics_and_custom_recorders() {
    let value = GenericRecord {
        value: Some("payload".to_owned()),
        marker: PhantomData,
    };
    let mut recorder = MemoryRecorder::new("generic");
    value.record_memory_children(&mut recorder);
    assert!(recorder.total_bytes() > 0);

    let value = CustomGenericRecord {
        value: PhantomData::<String>,
    };
    let mut recorder = MemoryRecorder::new("custom");
    value.record_memory_children(&mut recorder);

    let mut expected = BTreeMap::new();
    expected.insert(MemoryRecordKind::Approximate, 1);
    assert_eq!(recorder.totals_by_kind(), expected);
}
