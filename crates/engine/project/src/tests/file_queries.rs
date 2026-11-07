use rg_ir_model::PackageSlot;
use rg_item_tree::file_declarations;
use rg_source::{CapturedSource, SourcePresence};
use rg_std::NativeOsString;

use crate::{
    PackageResidencyPolicy, Project, SavedFileChange, StartupCacheLoad,
    testonly::{ProjectFixture, ProjectSourceFixture},
};

#[test]
fn file_declarations_reuse_across_restart_and_discover_a_new_child() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /src/lib.rs crate:file_queries
pub struct Before;
mod child;
"#,
    );
    let build = || {
        // Bypass the legacy package artifact so startup has to demand the file query.
        Project::builder(fixture.workspace_metadata())
            .package_residency_policy(PackageResidencyPolicy::AllOffloadable)
            .startup_cache_load(StartupCacheLoad::Disabled)
            .build()
            .expect("file query fixture should build")
    };
    let path = fixture.path("src/lib.rs");
    let child = fixture.path("src/child.rs");
    let child_probe = path
        .canonicalize()
        .expect("root file should canonicalize")
        .with_file_name("child.rs");
    let child_key = NativeOsString::from_os_str(child_probe.as_os_str());
    let mut cold = build();
    let file = ProjectFixture::file_id_for_path_in(cold.state.parse_db(), &path);
    assert!(cold.queries.database.statistics().bytes_written > 0);
    let initial = cold
        .queries
        .database
        .run(|db| file_declarations(db, PackageSlot(0), file))
        .expect("same-process file query should reuse declarations");
    assert_eq!(cold.queries.database.statistics().executed, 0);
    drop(cold);

    let mut warm = build();
    assert_eq!(
        ProjectFixture::file_id_for_path_in(warm.state.parse_db(), &path),
        file
    );
    assert_eq!(warm.queries.database.statistics().executed, 0);
    assert!(warm.queries.database.statistics().hits > 0);
    let presence = warm
        .queries
        .database
        .run(|db| SourcePresence::at(child_key.clone()).exists(db))
        .expect("missing child should have a captured presence value");
    assert!(!presence);

    std::fs::write(&child, "pub struct Appeared;\n").expect("fixture child should be written");
    let changes = warm
        .apply_change(SavedFileChange::fs_path(&child))
        .expect("adding a previously missing module should publish");
    assert_eq!(changes.changed_files.len(), 1);
    assert!(
        warm.queries.database.statistics().hits > 0,
        "the unchanged parent tree should be reused"
    );
    let symbols = warm
        .snapshot()
        .full_analysis(rg_std::CancellationToken::new())
        .expect("saved analysis should load")
        .workspace_symbols("Appeared")
        .expect("module declarations should be searchable");
    assert_eq!(symbols.len(), 1);
    assert!(
        warm.queries
            .database
            .run(|db| SourcePresence::at(child_key).exists(db))
            .expect("new child presence should be captured")
    );

    std::fs::write(&path, "pub struct After;\nmod child;\n").expect("fixture root should change");
    warm.apply_change(SavedFileChange::fs_path(&path))
        .expect("edited root should publish");
    assert!(warm.queries.database.statistics().executed > 0);
    let changed = warm
        .queries
        .database
        .run(|db| file_declarations(db, PackageSlot(0), file))
        .expect("edited file declarations should be available");
    assert_ne!(changed, initial);
}

#[test]
fn declined_candidate_restores_the_accepted_file_query_inputs() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /src/lib.rs crate:file_query_rejection
pub struct Published;
"#,
    );
    let path = fixture.path("src/lib.rs");
    let mut project = fixture.build_project();
    let generation = project.generation_id();
    let file = ProjectFixture::file_id_for_path_in(project.state.parse_db(), &path);
    let published = project
        .queries
        .database
        .run(|db| file_declarations(db, PackageSlot(0), file))
        .expect("published declarations should be available");
    let source =
        CapturedSource::new(&path, "pub struct Declined;\n").expect("capture candidate source");

    let declined = project
        .try_publish_generation_when(
            |candidate, database| {
                candidate
                    .parse_db_mut()
                    .refresh_captured_saved_file(&source)?;
                database.update_inputs(|update| {
                    candidate.parse_db().source_inventory().write_inputs(update)
                })?;
                Ok(database.run(|db| file_declarations(db, PackageSlot(0), file))?)
            },
            |_| false,
        )
        .expect("building the candidate should succeed");
    assert_ne!(declined, published);
    assert_eq!(project.generation_id(), generation);
    let current = project
        .queries
        .database
        .run(|db| file_declarations(db, PackageSlot(0), file))
        .expect("next operation should use the accepted source");
    assert_eq!(current, published);
}

#[test]
fn one_canonical_file_keeps_distinct_package_editions() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[workspace]
members = ["older", "newer"]
resolver = "3"

//- /older/Cargo.toml
[package]
name = "older"
version = "0.1.0"
edition = "2021"
[lib]
path = "../shared.rs"

//- /newer/Cargo.toml
[package]
name = "newer"
version = "0.1.0"
edition = "2024"
[lib]
path = "../shared.rs"

//- /shared.rs
pub fn gen() {}
"#,
    );
    let mut project = fixture.build_project();
    let older = ProjectFixture::package_slot_by_name_in(project.state.parse_db(), "older");
    let newer = ProjectFixture::package_slot_by_name_in(project.state.parse_db(), "newer");
    let root = |slot: PackageSlot| {
        project
            .state
            .parse_db()
            .package(slot.0)
            .expect("fixture package should exist")
            .targets()[0]
            .root_file
    };
    let file = root(older);
    assert_eq!(file, root(newer));

    let declares_gen = |tree: &rg_item_tree::FileTree| {
        tree.items.iter().any(|item| {
            item.name
                .as_ref()
                .is_some_and(|name| name.as_str() == "gen")
        })
    };
    let (old_tree, new_tree) = project
        .queries
        .database
        .run(|db| {
            Ok((
                file_declarations(db, older, file)?,
                file_declarations(db, newer, file)?,
            ))
        })
        .expect("both package interpretations should be available");
    assert!(declares_gen(&old_tree));
    assert!(!declares_gen(&new_tree));
}
