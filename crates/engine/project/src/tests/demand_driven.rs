use std::{fmt::Write as _, fs};

use rg_analysis::{ReferenceLocation, ReferenceQuery};
use rg_ir_model::{DefMapRef, PrimitiveTy, Span, UnsignedIntTy};
use rg_profile::test_support::ProfileTest;
use rg_semantic_ir::{ItemLookupQuery, ItemReader};
use rg_std::CancellationToken;

use crate::{Project, SavedFileChange, profile_descriptors, testonly::ProjectSourceFixture};

#[test]
fn ranged_inlay_hints_acquire_only_overlapping_owners() {
    let source = r#"
//- /src/lib.rs crate:ranged_hints
#[rustc_builtin_macro]
macro_rules! include { ($($args:tt)*) => {}; }
fn take(count: u8) -> u8 { count }
$function_start$fn first() { let value = 1u8; }$function_end$
struct Value;
impl Value {
    fn next(&self) -> Self { Value }
    fn finish(&self) -> u16 { 1 }
    $method_start$fn selected(&self) {
        let value = take(42);
        let result = self
            .next()
            .finish();
        // Padding for a closing-brace hint.
    }$method_end$
    fn offscreen(&self) { let value = 2u32; }
}
struct Other;
impl Other { include!("methods.rs"); }
fn use_late() { let value = Other::wanted(); let _ = $late$value; }
//- /src/methods.rs
pub fn wanted() -> u8 { 1u8 }
"#
    .replace("// Padding for a closing-brace hint.", &"\n".repeat(20));
    let fixture = ProjectSourceFixture::build(&source);
    let project = fixture.build_project();
    let mut ranged = Vec::new();
    for (start, end, labels, inferred) in [
        ("function_start", "function_end", &[": u8"][..], 1),
        (
            "method_start",
            "method_end",
            &[": u8", "count:", ": u16", "Value", "// fn selected"][..],
            1,
        ),
        ("method_end", "method_end", &["// fn selected"][..], 0),
    ] {
        let range = Span {
            start: fixture.markers().position(start).offset,
            end: fixture.markers().position(end).offset,
        };
        let profile = ProfileTest::start(profile_descriptors(), "body_ir.owner");
        let hints = project.inspect(|snapshot| {
            let context = snapshot
                .file_context_for_path(fixture.path("src/lib.rs"))
                .expect("route source")
                .expect("source target");
            snapshot
                .analysis()
                .inlay_hints(context.crate_ref, context.file, range, None)
                .expect("viewport hints")
        });
        assert_eq!(
            hints
                .iter()
                .map(|hint| hint.label.as_str())
                .collect::<Vec<_>>(),
            labels,
            "{start}"
        );
        let report = profile.finish();
        for counter in ["body_ir.owner.owners", "body_ir.owner.inference_owners"] {
            assert_eq!(
                report.inner().counter(counter).unwrap_or_default(),
                inferred,
                "{start}: {counter}"
            );
        }
        ranged.push((range, hints));
    }

    // Even the outline used by closing-brace hints must leave unrelated member macros lazy.
    fs::write(
        fixture.path("src/methods.rs"),
        "pub fn wanted() -> u16 { 1u16 }\n",
    )
    .expect("write first include observation");
    project.inspect(|snapshot| {
        let context = snapshot
            .file_context_for_path(fixture.path("src/lib.rs"))
            .expect("route source")
            .expect("source target");
        let analysis = snapshot.analysis();
        assert_eq!(
            analysis
                .type_at(
                    context.crate_ref,
                    context.file,
                    fixture.markers().position("late").offset
                )
                .expect("included signature")
                .expect("binding type")
                .primitive(),
            Some(PrimitiveTy::UnsignedInt(UnsignedIntTy::U16))
        );

        // Selecting owners early must give the same answers as filtering a complete request.
        let text = snapshot
            .sources()
            .text(context.file)
            .expect("fixture source");
        let range = Span::full_file(text.len() as u32);
        let all = analysis
            .inlay_hints(context.crate_ref, context.file, range, None)
            .expect("whole-file hints");
        for (range, hints) in ranged {
            let expected: Vec<_> = all
                .iter()
                .filter(|hint| range.touches(hint.text_offset()))
                .cloned()
                .collect();
            assert_eq!(hints, expected, "{range:?}");
        }
    });
}

#[test]
fn macro_binding_references_keep_their_local_scan_scope() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /src/lib.rs crate:macro_local_scope
mod other;
macro_rules! bind { ($name:ident) => { $name }; }
fn demo() {
    let bind!($binding$value) = 1u8;
    let _ = $usage$value;
}
//- /src/other.rs
fn first() { let value = 2u8; let _ = value; }
fn second() { let value = 3u8; let _ = value; }
"#,
    );
    let project = fixture.build_project();
    let profile = ProfileTest::start(profile_descriptors(), "body_ir.owner");
    project.inspect(|snapshot| {
        let context = snapshot
            .file_context_for_path(fixture.path("src/lib.rs"))
            .expect("route source")
            .expect("source target");
        let analysis = snapshot.analysis();
        let crates = [context.crate_ref];
        let start = fixture.markers().position("usage").offset;
        let expected = vec![ReferenceLocation {
            crate_ref: context.crate_ref,
            file_id: context.file,
            span: Span {
                start,
                end: start + 5,
            },
        }];
        // Both the copied binding and its ordinary use identify the same local. Whether the
        // declaration can be edited must not send either search into unrelated files.
        for marker in ["binding", "usage"] {
            assert_eq!(
                analysis
                    .references(
                        context.crate_ref,
                        context.file,
                        fixture.markers().position(marker).offset,
                        ReferenceQuery::find_references(&crates, false),
                    )
                    .expect("binding references"),
                expected,
                "references from {marker}",
            );
        }
    });
    profile.finish().assert_counter_path_with_message(
        "body_ir.owner.owners",
        1,
        "only the binding's function needs body preparation",
    );
}

#[test]
fn discovers_references_before_demanding_bodies_and_reuses_them_after_restart() {
    let mut source = r#"
//- /src/lib.rs crate:demanded
mod untouched;
pub struct $declaration$Wanted;
use $import$Wanted as Alias;
macro_rules! wrap { ($value:expr) => { $value }; }
fn ordinary() { let _ = $ordinary$Wanted; }
fn with_macro() { let _ = wrap!($macro$Wanted); }
impl $implementation$Wanted {
    fn new() -> $signature$Self { $self_type$Self }
}
struct Other;
impl Other { fn new() -> Self { Self } }
fn alias() { let _ = Alias; }
fn comment() { /* Wanted */ let text = "Wanted"; }

//- /src/untouched.rs
"#
    .to_owned();
    // A broad reference request visits this file, but its many unrelated owners must stay cold.
    for index in 0..80 {
        writeln!(
            source,
            "fn untouched_{index}() {{ let value = 1u64; let _ = value; }}"
        )
        .expect("fixture source writes");
    }
    let fixture = ProjectSourceFixture::build(&source);
    let profile = ProfileTest::start(profile_descriptors(), "body_ir.owner");
    let project = fixture.build_project();
    assert_eq!(
        profile
            .finish()
            .inner()
            .counter("body_ir.owner.owners")
            .unwrap_or_default(),
        0
    );

    // Keep the existing scanner's source policy: generated constructor expressions are not
    // projected as written references, and aliases keep their separate spelling.
    let expected = [
        "declaration",
        "import",
        "ordinary",
        "implementation",
        "signature",
    ];
    let query = |project: &Project| {
        project.inspect(|snapshot| {
            let context = snapshot
                .file_context_for_path(fixture.path("src/lib.rs"))
                .expect("route source")
                .expect("source belongs to target");
            let mut locations: Vec<_> = expected
                .iter()
                .map(|name| {
                    let position = fixture.markers().position(name);
                    let len = if matches!(*name, "signature" | "self_type") {
                        4
                    } else {
                        6
                    };
                    ReferenceLocation {
                        crate_ref: context.crate_ref,
                        file_id: context.file,
                        span: Span {
                            start: position.offset,
                            end: position.offset + len,
                        },
                    }
                })
                .collect();
            locations.sort_by_key(|location| location.span.start);
            let crates = [context.crate_ref];
            let query = ReferenceQuery::find_references(&crates, true);
            let analysis = snapshot.analysis();
            let offset = fixture.markers().position("declaration").offset;
            assert_eq!(
                analysis
                    .references(context.crate_ref, context.file, offset, query)
                    .expect("references"),
                locations
            );
            let rename = analysis
                .rename(context.crate_ref, context.file, offset, "Renamed", query)
                .expect("rename")
                .expect("type can be renamed");
            assert_eq!(
                rename
                    .edits
                    .iter()
                    .map(|edit| edit.span)
                    .collect::<Vec<_>>(),
                ["declaration", "import", "ordinary", "implementation"]
                    .iter()
                    .map(|name| {
                        let start = fixture.markers().position(name).offset;
                        Span {
                            start,
                            end: start + 6,
                        }
                    })
                    .collect::<Vec<_>>()
            );
        });
    };
    for expected_owners in [3, 0] {
        let profile = ProfileTest::start(profile_descriptors(), "body_ir.owner");
        query(&project);
        assert_eq!(
            profile
                .finish()
                .inner()
                .counter("body_ir.owner.owners")
                .unwrap_or_default(),
            expected_owners
        );
    }

    let profile = ProfileTest::start(profile_descriptors(), "body_ir.owner");
    project.inspect(|snapshot| {
        let analysis = snapshot.analysis();
        assert_eq!(
            analysis
                .workspace_symbols("Wanted")
                .expect("symbol search")
                .len(),
            1
        );
        let context = snapshot
            .file_context_for_path(fixture.path("src/untouched.rs"))
            .expect("route large file")
            .expect("large file belongs to target");
        let text = fs::read_to_string(fixture.path("src/untouched.rs")).expect("large source");
        let offset =
            u32::try_from(text.find("value;").expect("binding use")).expect("small offset");
        assert_eq!(
            analysis
                .type_at(context.crate_ref, context.file, offset)
                .expect("cursor type")
                .expect("binding type")
                .primitive(),
            Some(PrimitiveTy::UnsignedInt(UnsignedIntTy::U64))
        );
    });
    assert_eq!(
        profile
            .finish()
            .inner()
            .counter("body_ir.owner.owners")
            .unwrap_or_default(),
        1
    );

    let profile = ProfileTest::start(profile_descriptors(), "body_ir.owner");
    project.inspect(|snapshot| {
        let context = snapshot
            .file_context_for_path(fixture.path("src/untouched.rs"))
            .expect("route large file")
            .expect("large file belongs to target");
        let text = snapshot
            .sources()
            .text(context.file)
            .expect("fixture source");
        let range = Span::full_file(text.len() as u32);
        assert!(
            !snapshot
                .analysis()
                .inlay_hints(context.crate_ref, context.file, range, None)
                .expect("file hints")
                .is_empty()
        );
    });
    assert_eq!(
        profile
            .finish()
            .inner()
            .counter("body_ir.owner.owners")
            .unwrap_or_default(),
        79
    );

    let cancellation = CancellationToken::new();
    project
        .with_snapshot(cancellation.clone(), |snapshot| {
            let context = snapshot
                .file_context_for_path(fixture.path("src/lib.rs"))?
                .expect("source target");
            let text = snapshot.sources().text(context.file)?;
            let range = Span::full_file(text.len() as u32);
            let result =
                snapshot
                    .analysis()
                    .inlay_hints(context.crate_ref, context.file, range, None)?;
            cancellation.cancel();
            Ok(result)
        })
        .expect_err("cancelled request cannot publish");
    query(&project);
    drop(project);

    let profile = ProfileTest::start(profile_descriptors(), "body_ir.owner");
    let warm = fixture.build_project();
    query(&warm);
    assert_eq!(
        profile
            .finish()
            .inner()
            .counter("body_ir.owner.owners")
            .unwrap_or_default(),
        0
    );
}

#[test]
fn candidate_absence_tracks_routing_and_payload_shards() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /dep/src/lib.rs crate:dep root:dep
pub struct Value;
//- /app/src/lib.rs crate:app root:app deps:dep
fn read() { let value = dep::Value; let result = value.get(); let _ = res$result$ult; }
"#,
    );
    let mut project = fixture.build_project();
    let candidates = |project: &Project| {
        project.inspect(|snapshot| {
            let context = snapshot
                .file_context_for_path(fixture.path("app/src/lib.rs"))
                .expect("app routing")
                .expect("app target");
            let items = ItemReader(snapshot.declarations());
            let dep = snapshot
                .state
                .targets
                .iter()
                .find(|target| target.package != context.crate_ref.package)
                .copied()
                .expect("dependency target");
            let ty = items
                .all_items(DefMapRef::Crate(dep))
                .expect("dependency items")
                .into_iter()
                .find_map(|item| item.type_def())
                .expect("Value type");
            let lookup =
                ItemLookupQuery::build_from(items, context.crate_ref, &CancellationToken::new())
                    .expect("lookup view");
            lookup
                .inherent_functions_for_type_and_name(ty, "get")
                .expect("candidate question")
                .len()
        })
    };
    assert_eq!(candidates(&project), 0);
    assert_eq!(candidates(&project), 0);
    let path = fixture.path("dep/src/lib.rs");
    // Introducing the receiver changes the routing proof. The missing `get` answer is equal,
    // but it must now observe the selected payload shard before that shard gains the method.
    fs::write(
        &path,
        "pub struct Value;\nimpl Value { pub fn existing(&self) {} }\n",
    )
    .expect("add a receiver shard");
    project
        .apply_changes(vec![SavedFileChange::fs_path(path.clone())])
        .expect("accept receiver shard");
    assert_eq!(candidates(&project), 0);
    fs::write(
        &path,
        "pub struct Value;\nimpl Value { pub fn existing(&self) {} pub fn get(&self) -> u8 { 1u8 } }\n",
    )
    .expect("add dependency method");
    project
        .apply_changes(vec![SavedFileChange::fs_path(path)])
        .expect("accept dependency edit");
    assert_eq!(candidates(&project), 1);
    let position = fixture.markers().position("result");
    project.inspect(|snapshot| {
        let context = snapshot
            .file_context_for_path(fixture.path(&position.path))
            .expect("app routing")
            .expect("app target");
        assert_eq!(
            snapshot
                .analysis()
                .type_at(context.crate_ref, context.file, position.offset)
                .expect("method result")
                .expect("known result")
                .primitive(),
            Some(PrimitiveTy::UnsignedInt(UnsignedIntTy::U8))
        );
    });
}

#[test]
fn relative_module_keywords_select_their_scope_before_body_inference() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /src/lib.rs crate:relative_paths
mod $declaration$outer {
    pub fn item() {}
    fn same() { $same$self::item(); }
    mod child {
        fn nested() { $parent$super::item(); }
        mod grandchild {
            fn twice() { super::$ancestor$super::item(); }
        }
    }
}
fn absolute() { crate::$absolute$outer::item(); }
mod other {
    fn item() {}
    fn unrelated() { self::item(); }
}
"#,
    );
    let project = fixture.build_project();
    let profile = ProfileTest::start(profile_descriptors(), "body_ir.owner");
    project.inspect(|snapshot| {
        let context = snapshot
            .file_context_for_path(fixture.path("src/lib.rs"))
            .expect("route source")
            .expect("source target");
        let mut expected: Vec<_> = [
            ("declaration", 5),
            ("same", 4),
            ("parent", 5),
            ("ancestor", 5),
            ("absolute", 5),
        ]
        .into_iter()
        .map(|(name, len)| {
            let start = fixture.markers().position(name).offset;
            ReferenceLocation {
                crate_ref: context.crate_ref,
                file_id: context.file,
                span: Span {
                    start,
                    end: start + len,
                },
            }
        })
        .collect();
        expected.sort_by_key(|location| location.span.start);
        let crates = [context.crate_ref];
        assert_eq!(
            snapshot
                .analysis()
                .references(
                    context.crate_ref,
                    context.file,
                    fixture.markers().position("declaration").offset,
                    ReferenceQuery::find_references(&crates, true)
                )
                .expect("module references"),
            expected
        );
    });
    assert_eq!(
        profile
            .finish()
            .inner()
            .counter("body_ir.owner.owners")
            .unwrap_or_default(),
        4
    );
}
