use std::sync::Arc;

use rg_analysis::{ReferenceLocation, ReferenceQuery, RenameEdit};
use rg_body_ir::saved_body;
use rg_ir_model::{BodyKey, PrimitiveTy, Span, UnsignedIntTy};
use rg_semantic_ir::target_declarations;
use rg_std::CancellationToken;

use crate::{Project, SavedFileChange, testonly::ProjectSourceFixture};

#[test]
fn queries_bodies_in_primary_and_secondary_targets() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "eager_targets"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn primary() { let value = 1u8; let _ = val$primary$ue; }

//- /tests/secondary.rs
#[test]
fn secondary() { let value = 1u16; let _ = val$secondary$ue; }
"#,
    );

    let project = fixture.build_project();
    let generation = project.generation_id();

    for (marker, expected) in [
        ("secondary", UnsignedIntTy::U16),
        ("primary", UnsignedIntTy::U8),
    ] {
        let position = fixture.markers().position(marker);
        project.inspect(|snapshot| {
            let context = snapshot
                .file_context_for_path(fixture.path(&position.path))
                .expect("fixture context resolves")
                .expect("fixture target is indexed");
            let ty = snapshot
                .analysis()
                .type_at(context.crate_ref, context.file, position.offset)
                .expect("binding type resolves")
                .expect("binding has a type");
            assert_eq!(
                ty.primitive(),
                Some(PrimitiveTy::UnsignedInt(expected)),
                "{marker}"
            );
        });
    }
    assert_eq!(project.generation_id(), generation);
}

#[test]
fn warm_project_restores_generated_sources_and_releases_query_payloads() {
    let fixture = ProjectSourceFixture::build_with_fake_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "targets"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
macro_rules! make_module {
    () => { mod generated; };
}
make_module!();
pub use generated::{Imported, LIMIT};

//- /src/generated.rs
include!("included.rs");

//- /src/included.rs
pub struct Imported;
pub const LIMIT: usize = 2 + 3;

//- /src/main.rs
use targets::{Imported, LIMIT};
fn main() {
    fn nested() -> u8 { 42u8 }
    let value = nested();
}
"#,
    );
    let cold = fixture.build_project();
    let included = fixture
        .path("src/included.rs")
        .canonicalize()
        .expect("generated include exists");
    assert!(cold.state.parse_db().contains_file_path(&included));
    let file = cold.inspect(|snapshot| {
        snapshot
            .file_context_for_path(fixture.path("src/main.rs"))
            .expect("find binary file")
    });
    drop(cold);

    let warm = fixture.build_project();
    assert!(warm.state.parse_db().contains_file_path(&included));
    let path = fixture.path("src/main.rs");
    let text = std::fs::read_to_string(&path).expect("read binary source");
    let (definitions, body) = warm.inspect(|snapshot| {
        let context = snapshot
            .file_context_for_path(&path)
            .expect("find binary file");
        assert_eq!(context, file);
        let context = context.expect("binary belongs to a target");
        let analysis = snapshot.analysis();
        let offset = |needle| {
            u32::try_from(text.find(needle).expect("fixture marker")).expect("fixture offset fits")
        };
        for (name, expected) in [("value", "let value: u8"), ("LIMIT", "2 + 3; // 5")] {
            let hover = analysis
                .hover(context.crate_ref, context.file, offset(name))
                .expect("hover query")
                .expect("hover result");
            assert!(
                hover.blocks.iter().any(|block| block
                    .signature
                    .as_deref()
                    .is_some_and(|signature| signature.ends_with(expected))),
                "{hover:?}"
            );
        }
        let targets = analysis
            .goto_definition(context.crate_ref, context.file, offset("Imported"))
            .expect("imported type definition");
        assert_eq!(targets.len(), 1);
        assert_eq!(
            snapshot.file_path(targets[0].crate_ref.package, targets[0].file_id),
            Some(included.as_path())
        );

        let spans = text
            .match_indices("nested")
            .map(|(start, name)| {
                let start = u32::try_from(start).expect("fixture offset fits");
                Span {
                    start,
                    end: start + u32::try_from(name.len()).expect("name length fits"),
                }
            })
            .collect::<Vec<_>>();
        let expected = spans
            .into_iter()
            .map(|span| ReferenceLocation {
                crate_ref: context.crate_ref,
                file_id: context.file,
                span,
            })
            .collect::<Vec<_>>();
        let query = ReferenceQuery::file_scoped(context.crate_ref, context.file);
        let call = expected[1].span.start;
        assert_eq!(
            analysis
                .references(context.crate_ref, context.file, call, query)
                .expect("nested function references"),
            expected
        );
        let rename = analysis
            .rename(context.crate_ref, context.file, call, "renamed", query)
            .expect("rename query")
            .expect("nested function can be renamed");
        assert_eq!(
            rename.edits,
            expected
                .into_iter()
                .map(|location| RenameEdit {
                    crate_ref: location.crate_ref,
                    file_id: location.file_id,
                    span: location.span,
                    old_text: "nested".to_owned(),
                    new_text: "renamed".to_owned(),
                })
                .collect::<Vec<_>>()
        );

        let db = snapshot.query_context();
        let declarations = target_declarations(db, context.crate_ref).expect("binary declarations");
        let owners = declarations
            .body_owners
            .value(db)
            .expect("binary body owners");
        let body = db
            .query::<saved_body>(BodyKey {
                krate: context.crate_ref,
                owner: owners[0].owner,
            })
            .expect("main body");
        let definitions = declarations
            .definitions
            .value(db)
            .expect("binary definitions");
        (Arc::downgrade(&definitions), Arc::downgrade(&body))
    });
    assert!(definitions.upgrade().is_none());
    assert!(body.upgrade().is_none());
}

#[test]
fn saves_update_one_targets_membership_without_displacing_the_sibling() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "targets"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod shared;
mod new_child;

//- /src/main.rs
mod shared;
fn main() {}

//- /src/shared.rs
pub struct Shared;
"#,
    );
    let mut project = fixture.build_project();
    let shared = Project::inspect(&project, |snapshot| {
        snapshot
            .file_context_for_path(fixture.path("src/shared.rs"))
            .expect("route shared source")
            .expect("shared source belongs to both targets")
    });
    let owners = |project: &Project| {
        project.inspect(|snapshot| {
            snapshot
                .crates_for_file(shared.crate_ref.package, shared.file)
                .expect("find target owners")
        })
    };
    let before = owners(&project);
    assert_eq!(before.len(), 2);

    let root = fixture.path("src/lib.rs");
    let child = fixture.path("src/new_child.rs");
    std::fs::write(&root, "mod new_child;\n").expect("update library root");
    std::fs::write(&child, "pub struct NewChild;\n").expect("create new module");
    project
        .apply_changes([
            SavedFileChange::fs_path(&root),
            SavedFileChange::fs_path(&child),
        ])
        .expect("publish changed membership");

    let after = owners(&project);
    assert_eq!(after.len(), 1);
    assert!(before.contains(&after[0]));
    project.inspect(|snapshot| {
        let new_child = snapshot
            .file_context_for_path(&child)
            .expect("route new module")
            .expect("new module should be indexed");
        assert_ne!(after[0], new_child.crate_ref);
        assert_eq!(
            snapshot
                .file_context_for_path(fixture.path("src/main.rs"))
                .expect("route binary root")
                .expect("binary root")
                .crate_ref,
            after[0]
        );
    });
}

#[test]
fn deleting_a_cargo_target_keeps_the_remaining_workspace_queryable() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "deleted_target"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Library;

//- /src/bin/tool.rs
pub struct Tool;
fn main() {}
"#,
    );
    let mut project = fixture.build_project();
    let tool = fixture.path("src/bin/tool.rs");
    std::fs::remove_file(&tool).expect("remove autodiscovered target");
    project
        .apply_change(SavedFileChange::fs_path(tool))
        .expect("reload Cargo metadata after deleting a target");
    project.inspect(|snapshot| {
        let analysis = snapshot.analysis();
        assert!(
            analysis
                .workspace_symbols("Tool")
                .expect("query tool")
                .is_empty()
        );
        assert_eq!(
            analysis
                .workspace_symbols("Library")
                .expect("query library")
                .len(),
            1
        );
    });
}

#[test]
fn cancelled_snapshot_does_not_publish_an_answer_and_can_be_retried() {
    let fixture = ProjectSourceFixture::build(
        "//- /src/lib.rs crate:cancelled_snapshot\npub struct Visible;\n",
    );
    let project = fixture.build_project();
    let generation = project.generation_id();
    let token = CancellationToken::new();
    let error = project
        .with_snapshot(token.clone(), |snapshot| {
            let symbols = snapshot.analysis().workspace_symbols("Visible")?;
            token.cancel();
            Ok(symbols)
        })
        .expect_err("cancelled answer must not escape the operation");
    assert!(error.chain().any(|cause| {
        cause
            .downcast_ref::<ajika::Error>()
            .is_some_and(ajika::Error::is_cancelled)
    }));
    project.inspect(|snapshot| {
        assert_eq!(
            snapshot
                .analysis()
                .workspace_symbols("Visible")
                .expect("retry query")
                .len(),
            1
        )
    });
    assert_eq!(project.generation_id(), generation);
}
