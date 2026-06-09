use std::{collections::BTreeMap, marker::PhantomData, mem};

use rg_std::{MemoryRecordKind, MemoryRecorder, MemorySize, Shrink};

#[derive(MemorySize)]
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

#[derive(MemorySize)]
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

fn record_unknown_payload(payload: &str, recorder: &mut MemoryRecorder) {
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

#[derive(MemorySize)]
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

#[derive(MemorySize)]
struct GenericRecord<T> {
    value: Option<T>,
    #[memsize(skip)]
    marker: PhantomData<T>,
}

#[derive(MemorySize)]
#[memsize(no_auto_bound)]
struct CustomGenericRecord<T> {
    #[memsize(with = "record_custom_generic")]
    value: T,
}

#[derive(MemorySize)]
#[memsize(with = "record_whole_value")]
struct WholeValueRecord {
    value: String,
}

fn record_custom_generic<T>(_value: &T, recorder: &mut MemoryRecorder) {
    recorder.record_approximate::<CustomGenericRecord<T>>(1);
}

fn record_whole_value(value: &WholeValueRecord, recorder: &mut MemoryRecorder) {
    recorder.record_approximate::<WholeValueRecord>(value.value.len());
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

#[test]
fn derive_supports_type_level_custom_recorders() {
    let value = WholeValueRecord {
        value: "custom".to_owned(),
    };

    let mut recorder = MemoryRecorder::new("whole");
    value.record_memory_children(&mut recorder);

    assert_eq!(
        recorder
            .totals_by_kind()
            .get(&MemoryRecordKind::Approximate),
        Some(&6)
    );
    assert_eq!(recorder.totals_by_path().get("whole"), Some(&6));
}

#[derive(Debug, Default, PartialEq, Eq)]
struct ShrinkProbe {
    calls: usize,
}

impl Shrink for ShrinkProbe {
    fn shrink_to_fit(&mut self) {
        self.calls += 1;
    }
}

#[derive(Shrink)]
struct ShrinkProject {
    value: ShrinkProbe,
    values: Vec<ShrinkProbe>,
    #[shrink(skip)]
    scratch: ShrinkProbe,
}

#[test]
fn shrink_derive_compacts_struct_fields_and_skips_marked_fields() {
    let mut value = ShrinkProject {
        value: ShrinkProbe::default(),
        values: vec![ShrinkProbe::default(), ShrinkProbe::default()],
        scratch: ShrinkProbe::default(),
    };

    value.shrink_to_fit();

    assert_eq!(value.value.calls, 1);
    assert_eq!(value.values[0].calls, 1);
    assert_eq!(value.values[1].calls, 1);
    assert_eq!(value.scratch.calls, 0);
}

#[derive(Shrink)]
enum ShrinkResolution {
    Local(ShrinkProbe),
    Pair {
        left: ShrinkProbe,
        #[shrink(skip)]
        right: ShrinkProbe,
    },
    #[shrink(skip)]
    Unknown {
        payload: ShrinkProbe,
    },
}

#[test]
fn shrink_derive_compacts_only_active_enum_variant_fields() {
    let mut value = ShrinkResolution::Local(ShrinkProbe::default());
    value.shrink_to_fit();
    let ShrinkResolution::Local(probe) = value else {
        panic!("variant should be preserved");
    };
    assert_eq!(probe.calls, 1);

    let mut value = ShrinkResolution::Pair {
        left: ShrinkProbe::default(),
        right: ShrinkProbe::default(),
    };
    value.shrink_to_fit();
    let ShrinkResolution::Pair { left, right } = value else {
        panic!("variant should be preserved");
    };
    assert_eq!(left.calls, 1);
    assert_eq!(right.calls, 0);

    let mut value = ShrinkResolution::Unknown {
        payload: ShrinkProbe::default(),
    };
    value.shrink_to_fit();
    let ShrinkResolution::Unknown { payload } = value else {
        panic!("variant should be preserved");
    };
    assert_eq!(payload.calls, 0);
}

#[derive(Shrink)]
struct GenericShrink<T> {
    value: Option<T>,
    #[shrink(skip)]
    marker: PhantomData<T>,
}

#[derive(Shrink)]
struct CustomShrink {
    #[shrink(with = "shrink_custom_value")]
    value: ShrinkProbe,
}

fn shrink_custom_value(value: &mut ShrinkProbe) {
    value.calls += 10;
}

#[derive(Shrink)]
#[shrink(no_auto_bound)]
struct CustomGenericShrink<T> {
    #[shrink(skip)]
    marker: PhantomData<T>,
}

#[test]
fn shrink_derive_handles_generics_and_custom_fields() {
    let mut value = GenericShrink {
        value: Some(ShrinkProbe::default()),
        marker: PhantomData,
    };
    value.shrink_to_fit();
    assert_eq!(value.value.expect("option should stay populated").calls, 1);

    let mut value = CustomShrink {
        value: ShrinkProbe::default(),
    };
    value.shrink_to_fit();
    assert_eq!(value.value.calls, 10);

    let mut value = CustomGenericShrink::<String> {
        marker: PhantomData,
    };
    value.shrink_to_fit();
}
