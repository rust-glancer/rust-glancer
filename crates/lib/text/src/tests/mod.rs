use std::collections::{BTreeSet, HashMap};

use rg_std::{MemoryRecordKind, MemoryRecorder, MemorySize};
use wincode::io::std_read::ReadAdapter;

use crate::Name;

#[test]
fn constructors_and_decoders_preserve_canonical_spelling_and_string_bytes() {
    let cases = [
        ("User", "User"),
        ("r#type", "type"),
        ("'r#fn", "'fn"),
        ("r#LongSharedIdentifier", "LongSharedIdentifier"),
        ("'r#LongSharedLifetime", "'LongSharedLifetime"),
        ("r#пользователь", "пользователь"),
        ("", ""),
    ];
    for (source, expected) in cases {
        // Write the source spelling as a string, then read it as a Name through both reader paths.
        // A decoded name should remove `r#` just like one built directly from source.
        let source_bytes = wincode::serialize(source).expect("source spelling should serialize");
        let expected_bytes =
            wincode::serialize(expected).expect("canonical spelling should serialize");
        let names = [
            Name::new(source),
            Name::from(source),
            Name::from(source.to_owned()),
            wincode::deserialize_exact::<Name>(&source_bytes)
                .expect("slice name should deserialize"),
            wincode::deserialize_from::<Name>(ReadAdapter::new(source_bytes.as_slice()))
                .expect("stream name should deserialize"),
        ];
        for name in names {
            assert_eq!(name.as_str(), expected, "canonical spelling for {source}");
            assert_eq!(name.to_string(), expected, "display for {source}");
            assert_eq!(format!("{name:?}"), format!("{expected:?}"));
            assert_eq!(
                wincode::serialize(&name).expect("name should serialize"),
                expected_bytes,
                "string encoding for {source}",
            );
        }
    }
    assert!(Name::missing().is_missing());
    assert_eq!(Name::missing(), "<missing>");
}

#[test]
fn borrowed_lookup_and_ordering_follow_strings() {
    let spellings = ["ZuluSharedName", "a", "AlphaSharedName", "пользователь"];
    let map = spellings
        .iter()
        .enumerate()
        .map(|(index, spelling)| (Name::new(spelling), index))
        .collect::<HashMap<_, _>>();
    let set = map.keys().cloned().collect::<BTreeSet<_>>();
    for (index, spelling) in spellings.into_iter().enumerate() {
        assert_eq!(
            map.get(spelling),
            Some(&index),
            "hash lookup for {spelling}"
        );
        assert_eq!(set.get(spelling), Some(&Name::new(spelling)));
    }

    let mut expected = spellings.to_vec();
    expected.sort();
    assert_eq!(set.iter().map(Name::as_str).collect::<Vec<_>>(), expected);
}

#[test]
fn workers_share_independently_constructed_and_decoded_names() {
    // Keep one long name alive while each worker builds matching names on its own.
    // Comparing text addresses checks that they reuse its allocation. Short names would
    // live inside each value and would not have a shared address to compare.
    let original = Name::new("SharedAcrossQueryWorkers");
    let bytes = wincode::serialize(&original).expect("shared name should serialize");
    let names = std::thread::scope(|scope| {
        let workers = (0..4)
            .map(|_| {
                let bytes = &bytes;
                scope.spawn(move || {
                    [
                        Name::new("r#SharedAcrossQueryWorkers"),
                        Name::from("SharedAcrossQueryWorkers".to_owned()),
                        wincode::deserialize_exact::<Name>(bytes)
                            .expect("worker slice name should deserialize"),
                        wincode::deserialize_from::<Name>(ReadAdapter::new(bytes.as_slice()))
                            .expect("worker stream name should deserialize"),
                    ]
                })
            })
            .collect::<Vec<_>>();
        workers
            .into_iter()
            .flat_map(|worker| worker.join().expect("name worker should finish"))
            .collect::<Vec<_>>()
    });
    drop(bytes);
    for name in names {
        assert_eq!(name, original);
        assert_eq!(name.as_str().as_ptr(), original.as_str().as_ptr());
    }
}

#[test]
fn memory_reports_count_shared_payload_once_and_inline_names_without_heap() {
    let mut recorder = MemoryRecorder::new("names");
    Name::new("User").record_memory_children(&mut recorder);
    assert!(
        !recorder
            .totals_by_kind()
            .contains_key(&MemoryRecordKind::Heap)
    );

    let first = Name::new("LongSharedIdentifier");
    let duplicate = Name::new("LongSharedIdentifier");
    let other = Name::new("AnotherSharedIdentifier");
    for name in [&first, &duplicate, &other] {
        name.record_memory_children(&mut recorder);
    }
    assert_eq!(
        recorder.totals_by_kind().get(&MemoryRecordKind::Heap),
        Some(&(first.len() + other.len())),
    );
}
