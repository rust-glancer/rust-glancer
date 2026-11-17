mod body_locations;
mod cargo_build_outputs;
mod current_source;
mod declaration_locations;
mod demand_driven;
mod file_queries;
mod format_captures;
mod generated_modules;
mod initializers;
mod member_identity;
mod utils;

use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
};

use expect_test::expect;
use rg_analysis::ReferenceQuery;
use rg_ir_model::{PackageSlot, Span};
use rg_source::CapturedSource;

use self::utils::{HostFixture, HostObservation};
use crate::{
    AnalysisChangeSummary, Project, ProjectMemoryHooks, ProjectMemoryPurgePoint, SavedFileChange,
    testonly::{ProjectFixture, ProjectSourceFixture},
};

#[derive(Debug)]
struct RecordingMemoryHooks {
    points: Arc<Mutex<Vec<ProjectMemoryPurgePoint>>>,
}

impl ProjectMemoryHooks for RecordingMemoryHooks {
    fn purge(&self, point: ProjectMemoryPurgePoint) {
        self.points
            .lock()
            .expect("recorded memory hook points should not be poisoned")
            .push(point);
    }
}

#[test]
fn captured_saved_source_publishes_the_captured_text_and_revision() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "captured_saved_source_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Before;
"#,
    );
    let path = fixture.path("src/lib.rs");
    let mut project = fixture.build_project();
    let previous_generation = project.generation_id();
    let captured = CapturedSource::new(&path, "pub struct Captured;\n")
        .expect("existing fixture source should capture");
    std::fs::write(&path, captured.text()).expect("disk should contain the proposed saved value");

    let summary = project
        .apply_change(SavedFileChange::captured(captured.clone()))
        .expect("matching captured source should publish");

    assert_ne!(project.generation_id(), previous_generation);
    let canonical = path
        .canonicalize()
        .expect("published fixture source should canonicalize");
    let entry = project.state.parse_db().packages()[0]
        .parsed_files()
        .find(|file| file.path() == canonical)
        .expect("published captured source should remain available");
    assert_eq!(summary.changed_files.len(), 1);
    assert_eq!(entry.source_revision(), captured.revision());
    assert_eq!(
        entry
            .source_text()
            .expect("published captured source should reload from matching disk")
            .as_ref(),
        "pub struct Captured;\n"
    );
}

#[test]
fn captured_saved_source_rejects_newer_disk_without_changing_the_published_project() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "captured_saved_source_race_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Published;
"#,
    );
    let path = fixture.path("src/lib.rs");
    let mut project = fixture.build_project();
    let published_generation = project.generation_id();
    let file_id = ProjectFixture::file_id_for_path_in(project.state.parse_db(), &path);

    let captured = CapturedSource::new(&path, "pub struct Proposed;\n")
        .expect("existing fixture source should capture");
    std::fs::write(&path, "pub struct Newer;\n")
        .expect("disk should advance beyond the captured proposal");
    let error = project
        .apply_change(SavedFileChange::captured(captured))
        .expect_err("a newer disk value must reject the captured candidate");

    let canonical = path
        .canonicalize()
        .expect("fixture source should canonicalize");
    assert_eq!(
        Project::stale_source_path(&error),
        Some(canonical.as_path()),
        "validation should preserve the typed stale source identity"
    );
    assert_eq!(project.generation_id(), published_generation);

    // Restore the disk proof for the still-published generation, then verify that the failed
    // proposal never leaked into its ParseDb.
    std::fs::write(&path, "pub struct Published;\n")
        .expect("published fixture source should be restored");
    let text = project.inspect(|snapshot| {
        snapshot
            .file_text_for_span(PackageSlot(0), file_id, Span { start: 0, end: 21 })
            .expect("published source should load after rejected proposal")
            .expect("published source span should exist")
    });
    assert_eq!(text, "pub struct Published;");
}

#[test]
fn graph_rebuild_cannot_acknowledge_bytes_newer_than_a_captured_source() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "captured_graph_rebuild_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Published;
"#,
    );
    let source = fixture.path("src/lib.rs");
    let mut project = fixture.build_project();
    let published_generation = project.generation_id();
    let captured = CapturedSource::new(&source, "pub struct Proposed;\n")
        .expect("existing fixture source should capture");
    std::fs::write(&source, "pub struct Newer;\n")
        .expect("disk should advance beyond the captured watcher value");

    let error = project
        .apply_changes([
            SavedFileChange::fs_path(fixture.path("Cargo.toml")),
            SavedFileChange::captured(captured),
        ])
        .expect_err("graph rebuilding must retain the captured Rust-source identity");

    assert!(
        Project::stale_source_path(&error).is_some(),
        "the graph candidate should fail through saved-source validation: {error:#}"
    );
    assert_eq!(project.generation_id(), published_generation);
}

#[test]
fn unchanged_captured_save_does_not_publish_a_new_generation() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "unchanged_captured_save_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct AlreadySaved;
"#,
    );
    let path = fixture.path("src/lib.rs");
    let mut project = fixture.build_project();
    let generation = project.generation_id();
    let captured = CapturedSource::new(&path, "pub struct AlreadySaved;\n")
        .expect("existing fixture source should capture");

    let summary = project
        .apply_change(SavedFileChange::captured(captured))
        .expect("unchanged captured save should be accepted");

    assert_eq!(summary, AnalysisChangeSummary::default());
    assert_eq!(project.generation_id(), generation);
}

#[test]
fn missing_only_saved_change_batch_preserves_published_generation() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "source_generation_noop_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Published;
"#,
    );
    let mut project = fixture.build_project();
    let published_generation = project.generation_id();

    let summary = project
        .apply_changes([SavedFileChange::fs_path(fixture.path("src/disappeared.rs"))])
        .expect("an obsolete saved change should be a successful no-op");

    assert_eq!(summary, AnalysisChangeSummary::default());
    assert_eq!(
        project.generation_id(),
        published_generation,
        "a no-op batch must not publish a new project generation",
    );
}

#[test]
fn saved_module_rename_retires_historical_source() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "source_generation_module_rename_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod old;

//- /src/old.rs
pub struct Before;
"#,
    );
    let mut project = fixture.build_project();
    let root = fixture.path("src/lib.rs");
    let old = fixture.path("src/old.rs");
    let old_canonical = old
        .canonicalize()
        .expect("old module path should canonicalize before rename");
    let new = fixture.path("src/new.rs");

    std::fs::rename(&old, &new).expect("fixture module should be renamed");
    std::fs::write(&root, "mod new;\n").expect("fixture root should name the new module");
    let summary = project
        .apply_changes([
            SavedFileChange::fs_path(&root),
            SavedFileChange::fs_path(&new),
        ])
        .expect("module rename should publish a new source generation");

    assert_eq!(summary.changed_files.len(), 3);
    assert!(
        !project.state.parse_db().contains_file_path(&old_canonical),
        "renamed module source should leave the package file table",
    );

    // A later save proves that the retired path cannot poison every candidate forked from this
    // generation.
    std::fs::write(&new, "pub struct After;\n")
        .expect("renamed module should accept a later saved edit");
    project
        .apply_change(SavedFileChange::fs_path(&new))
        .expect("later save should not validate the retired module path");
}

#[test]
fn saved_line_index_rejects_newer_disk_revision() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "source_generation_line_index_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Before;
"#,
    );
    let path = fixture.path("src/lib.rs");
    let project = fixture.build_project();
    let file_id = ProjectFixture::file_id_for_path_in(project.state.parse_db(), &path);
    std::fs::write(&path, "\n\npub struct After;\n")
        .expect("newer fixture source should be written");

    let error = project
        .with_snapshot(rg_std::CancellationToken::new(), |snapshot| {
            snapshot
                .file_line_index(PackageSlot(0), file_id)
                .map(|index| index.cloned())
        })
        .expect_err("line index reload should reject a newer source revision");
    assert!(
        error.chain().any(|cause| matches!(
            cause.downcast_ref::<rg_source::SourceError>(),
            Some(rg_source::SourceError::Stale { .. })
        )),
        "line-index failure should retain the typed stale-source cause: {error:#}",
    );
}

#[test]
fn batched_saves_publish_both_changes_and_skip_missing_paths() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "batch_rebuild_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod account;
mod user;

//- /src/account.rs
pub struct Account;

//- /src/user.rs
pub struct User;
"#,
    );
    let points = Arc::new(Mutex::new(Vec::new()));
    let hooks: Arc<dyn ProjectMemoryHooks> = Arc::new(RecordingMemoryHooks {
        points: Arc::clone(&points),
    });
    let mut project = Project::builder(fixture.workspace_metadata())
        .memory_hooks(hooks)
        .build()
        .expect("analysis project should build");
    points
        .lock()
        .expect("recorded memory hook points should not be poisoned")
        .clear();

    let saved_files = fixture.write_fixture_files(
        r#"
//- /src/account.rs
pub struct SavedAccount;

//- /src/user.rs
pub struct SavedUser;
"#,
    );
    let mut changes = saved_files
        .files()
        .iter()
        .map(|file| SavedFileChange::fs_path(fixture.path(file.relative_path())))
        .collect::<Vec<_>>();
    changes.insert(
        1,
        SavedFileChange::fs_path(fixture.path("src/disappeared.rs")),
    );

    let summary = project
        .apply_changes(changes)
        .expect("batched source changes should apply");

    assert_eq!(
        summary.changed_files.len(),
        2,
        "both saved module files should be reported as changed"
    );
    assert_eq!(
        summary.changed_crates.len(),
        1,
        "both changed modules belong to one target"
    );
    assert_eq!(
        points
            .lock()
            .expect("recorded memory hook points should not be poisoned")
            .as_slice(),
        [ProjectMemoryPurgePoint::AfterProjectChange],
        "a multi-file batch should publish one generation",
    );
}

#[test]
fn repeated_saved_source_notifications_preserve_generation() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "unchanged_notification_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct AlreadyApplied;
"#,
    );
    let points = Arc::new(Mutex::new(Vec::new()));
    let hooks: Arc<dyn ProjectMemoryHooks> = Arc::new(RecordingMemoryHooks {
        points: Arc::clone(&points),
    });
    let mut project = Project::builder(fixture.workspace_metadata())
        .memory_hooks(hooks)
        .build()
        .expect("analysis project should build");
    let generation = project.generation_id();
    points
        .lock()
        .expect("recorded memory hook points should not be poisoned")
        .clear();

    let summary = project
        .apply_change(SavedFileChange::fs_path(fixture.path("src/lib.rs")))
        .expect("an unchanged watcher replay should be accepted");

    assert_eq!(summary, AnalysisChangeSummary::default());
    assert_eq!(
        project.generation_id(),
        generation,
        "an unchanged watcher replay must not publish a replacement generation",
    );
    assert!(
        points
            .lock()
            .expect("recorded memory hook points should not be poisoned")
            .is_empty(),
        "an unchanged watcher replay must not purge a replaced generation",
    );
}

#[test]
fn saved_queries_include_body_references_and_rename() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "reference_surface_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod other;

pub struct User {
    pub na$subject$me: usize,
}

//- /src/other.rs
use crate::User;

pub fn demo(user: User) -> usize {
    user.name
}
"#,
    );
    let workspace = fixture.workspace_metadata();

    let project = Project::builder(workspace)
        .build()
        .expect("eager project build should succeed");

    let subject = fixture.markers().position("subject");
    project.inspect(|snapshot| {
        let lib_context = snapshot
            .file_context_for_path(fixture.path(&subject.path))
            .expect("subject file contexts should resolve")
            .expect("subject file should have one context");
        let other_context = snapshot
            .file_context_for_path(fixture.path("src/other.rs"))
            .expect("other file contexts should resolve")
            .expect("other file should have one context");
        let target = lib_context.crate_ref;

        let search_files = {
            let analysis = snapshot.analysis();
            let declaration_targets = analysis
                .goto_definition(target, lib_context.file, subject.offset)
                .expect("definition lookup should resolve")
                .into_iter()
                .map(|target| target.crate_ref)
                .collect::<Vec<_>>();
            let search_targets = snapshot
                .reference_search_crates(
                    lib_context.crate_ref.package,
                    &declaration_targets,
                    &rg_std::CancellationToken::new(),
                )
                .expect("plan reference crates");
            let labels = analysis
                .reference_search_labels(target, lib_context.file, subject.offset)
                .expect("reference labels should resolve");
            let files = snapshot
                .reference_search_files_matching_labels(
                    &search_targets,
                    &labels,
                    &rg_std::CancellationToken::new(),
                )
                .expect("reference text prefilter should resolve")
                .expect("reference text prefilter should find label-bearing files");

            assert!(
                files
                    .iter()
                    .any(|file| file.crate_ref == target && file.file_id == other_context.file),
                "reference scan surface should include the file with body references",
            );

            files
        };

        let analysis = snapshot.analysis();
        let query = ReferenceQuery::find_references_in_files(&search_files, true);
        let references = analysis
            .references(target, lib_context.file, subject.offset, query)
            .expect("references should resolve after indexing");
        assert!(
            references
                .iter()
                .any(|reference| reference.file_id == other_context.file),
            "references should include body occurrences from the indexed scan surface",
        );

        let query = ReferenceQuery::find_references_in_files(&search_files, true);
        let rename = analysis
            .rename(target, lib_context.file, subject.offset, "label", query)
            .expect("rename should resolve after indexing")
            .expect("rename should be available for the selected field");
        assert!(
            rename.edits.iter().any(|edit| {
                edit.file_id == other_context.file
                    && edit.old_text == "name"
                    && edit.new_text == "label"
            }),
            "rename should edit body occurrences from the indexed scan surface",
        );
    });
}

#[test]
fn reexpands_items_from_a_cached_dependency_macro() {
    let mut fixture = ProjectFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "macro_cache_app"
version = "0.1.0"
edition = "2024"
[dependencies]
macro_cache_dep = { path = "dep" }
//- /src/lib.rs
macro_cache_dep::make!(Before);
//- /dep/Cargo.toml
[package]
name = "macro_cache_dep"
version = "0.1.0"
edition = "2024"
//- /dep/src/lib.rs
#[macro_export]
macro_rules! make {
    ($name:ident) => {
        /// Generated documentation.
        pub struct $name;
    };
}
"#,
    );
    fixture.apply_saved_fixture("//- /src/lib.rs\nmacro_cache_dep::make!(After);\n");
    fixture.project().inspect(|snapshot| {
        let analysis = snapshot.analysis();
        assert_eq!(
            analysis
                .workspace_symbols("After")
                .expect("symbol query should succeed")
                .len(),
            1
        );
        assert!(
            analysis
                .workspace_symbols("Before")
                .expect("symbol query should succeed")
                .is_empty()
        );
    });
}

#[test]
fn macro_expansion_limit_summary_refreshes_on_save() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "macro_limit_summary_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
macro_rules! recurse {
    () => {
        recurse!();
    };
}

recurse!();
"#,
    );
    let source_path = fixture.path("src/lib.rs");
    let mut project = Project::builder(fixture.workspace_metadata())
        .build()
        .expect("recursive macro fixture should build a bounded project");

    let summary = project.macro_expansion_limit_summary();
    assert_eq!(summary.affected_crate_count(), 1);
    assert!(summary.skipped_macro_call_count() > 0);
    assert_eq!(
        summary.listed_crates(),
        ["macro_limit_summary_fixture/macro_limit_summary_fixture"]
    );
    assert_eq!(summary.omitted_crate_count(), 0);

    std::fs::write(&source_path, "pub struct Finished;\n")
        .expect("fixture source should accept a finite replacement");
    let change = project
        .apply_change(SavedFileChange::fs_path(source_path))
        .expect("finite replacement should rebuild the affected package");
    assert_eq!(change.changed_crates.len(), 1);
    assert!(
        project.macro_expansion_limit_summary().is_empty(),
        "a later successful expansion build should not retain the previous warning summary",
    );
}

#[test]
fn reference_search_crates_follow_workspace_reverse_dependencies() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[workspace]
members = ["crates/dep", "crates/mid", "crates/app", "crates/independent"]
resolver = "3"

//- /crates/dep/Cargo.toml
[package]
name = "dep"
version = "0.1.0"
edition = "2024"

//- /crates/dep/src/lib.rs
pub struct Dep;

//- /crates/mid/Cargo.toml
[package]
name = "mid"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "../dep" }

//- /crates/mid/src/lib.rs
pub struct Mid(dep::Dep);

//- /crates/app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
mid = { path = "../mid" }

//- /crates/app/src/lib.rs
pub struct App(mid::Mid);

//- /crates/independent/Cargo.toml
[package]
name = "independent"
version = "0.1.0"
edition = "2024"

//- /crates/independent/src/lib.rs
pub struct Independent;
"#,
    );
    let project = fixture.build_project();
    project.inspect(|snapshot| {
        let dep_package = ProjectFixture::package_slot_by_name_in(snapshot.parse_db(), "dep");
        let app_package = ProjectFixture::package_slot_by_name_in(snapshot.parse_db(), "app");
        let dep_file = ProjectFixture::file_id_for_path_in(
            snapshot.parse_db(),
            &fixture.path("crates/dep/src/lib.rs"),
        );
        let dep_target = snapshot
            .crates_for_file(dep_package, dep_file)
            .expect("dep target lookup should succeed")
            .into_iter()
            .next()
            .expect("dep lib file should belong to the dep lib target");

        let package_names = snapshot
            .reference_search_crates(
                app_package,
                &[dep_target],
                &rg_std::CancellationToken::new(),
            )
            .expect("plan reference crates")
            .into_iter()
            .map(|target| {
                snapshot
                    .parse_db()
                    .package(target.package.0)
                    .expect("reference search target package should exist")
                    .package_name()
                    .to_owned()
            })
            .collect::<BTreeSet<_>>();

        assert_eq!(
            package_names,
            BTreeSet::from(["app".to_string(), "dep".to_string(), "mid".to_string()]),
            "workspace reference searches should skip unrelated workspace packages"
        );
    });
}

#[test]
fn reparses_known_file_in_place() {
    let mut fixture = HostFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "host_update_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;
"#,
    );
    let before_file_id = fixture.file_id_for_path("src/lib.rs");

    fixture.check(
        &[HostObservation::workspace_symbols("User")],
        expect![[r#"
            workspace symbols `User`
            - struct User @ host_update_fixture[lib] src/lib.rs
        "#]],
    );

    fixture.check_save(
        r#"
//- /src/lib.rs
pub struct Account;
"#,
        &[
            HostObservation::workspace_symbols("Account"),
            HostObservation::workspace_symbols("User"),
        ],
        expect![[r#"
            changed files
            - host_update_fixture src/lib.rs

            changed targets
            - host_update_fixture[lib]

            workspace symbols `Account`
            - struct Account @ host_update_fixture[lib] src/lib.rs

            workspace symbols `User`
            - <none>
        "#]],
    );

    let after_file_id = fixture.file_id_for_path("src/lib.rs");
    assert_eq!(
        after_file_id, before_file_id,
        "known file reparses should keep the package-local FileId stable"
    );
}

#[test]
fn reads_saved_disk_text_for_modules_discovered_after_the_change() {
    let mut fixture = HostFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "host_new_module_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Root;

//- /src/api.rs
pub struct DiskOnly;
"#,
    );

    fixture.check_save(
        r#"
//- /src/api.rs
pub struct SavedOnly;

//- /src/lib.rs
mod api;
"#,
        &[
            HostObservation::workspace_symbols("SavedOnly"),
            HostObservation::workspace_symbols("DiskOnly"),
        ],
        expect![[r#"
            changed files
            - host_new_module_fixture src/api.rs
            - host_new_module_fixture src/lib.rs

            changed targets
            - host_new_module_fixture[lib]

            workspace symbols `SavedOnly`
            - struct SavedOnly @ host_new_module_fixture[lib] src/api.rs

            workspace symbols `DiskOnly`
            - <none>
        "#]],
    );
}

#[test]
fn rebuilds_project_after_manifest_adds_dependency() {
    let mut fixture = HostFixture::build(
        r#"
//- /Cargo.toml
[workspace]
members = ["crates/dep", "crates/app"]
resolver = "3"

//- /crates/dep/Cargo.toml
[package]
name = "dep"
version = "0.1.0"
edition = "2024"

//- /crates/dep/src/lib.rs
pub struct Api;

//- /crates/app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

//- /crates/app/src/lib.rs
pub fn use_dep(value: dep::Api) {
    let same = val$0ue;
}
"#,
    );

    fixture.check(
        &[HostObservation::type_names_at("app marker 0", "app", "0")],
        expect![[r#"
            type names at `app marker 0`
            - <none>
        "#]],
    );

    fixture.check_save(
        r#"
//- /crates/app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "../dep" }
"#,
        &[HostObservation::type_names_at("app marker 0", "app", "0")],
        expect![[r#"
            changed files
            - <none>

            changed targets
            - app[lib]
            - dep[lib]

            type names at `app marker 0`
            - Api
        "#]],
    );
}

#[test]
fn rebuilds_project_after_manifest_adds_target() {
    let mut fixture = HostFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "target_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Lib;

//- /src/tool.rs
fn main() {}
"#,
    );

    fixture.check(
        &[HostObservation::file_context("tool file", "src/tool.rs")],
        expect![[r#"
            file context `tool file`
            - <none>
        "#]],
    );

    fixture.check_save(
        r#"
//- /Cargo.toml
[package]
name = "target_fixture"
version = "0.1.0"
edition = "2024"

[[bin]]
name = "tool"
path = "src/tool.rs"
"#,
        &[HostObservation::file_context("tool file", "src/tool.rs")],
        expect![[r#"
            changed files
            - <none>

            changed targets
            - target_fixture[bin]
            - target_fixture[lib]

            file context `tool file`
            - target_fixture src/tool.rs -> target_fixture[bin]
        "#]],
    );
}

#[test]
fn rebuilds_project_after_new_workspace_member_manifest_is_saved() {
    let mut fixture = HostFixture::build(
        r#"
//- /Cargo.toml
[workspace]
members = ["crates/*"]
resolver = "3"

//- /crates/app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

//- /crates/app/src/lib.rs
pub struct App;
"#,
    );

    fixture.check_save(
        r#"
//- /crates/new_pkg/Cargo.toml
[package]
name = "new_pkg"
version = "0.1.0"
edition = "2024"

//- /crates/new_pkg/src/lib.rs
pub struct NewType;
"#,
        &[HostObservation::workspace_symbols("NewType")],
        expect![[r#"
            changed files
            - new_pkg crates/new_pkg/src/lib.rs

            changed targets
            - app[lib]
            - new_pkg[lib]

            workspace symbols `NewType`
            - struct NewType @ new_pkg[lib] crates/new_pkg/src/lib.rs
        "#]],
    );
}

#[test]
fn discovers_new_workspace_member_after_manifest_becomes_valid() {
    let mut fixture = HostFixture::build(
        r#"
//- /Cargo.toml
[workspace]
members = ["crates/*"]
resolver = "3"

//- /crates/app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

//- /crates/app/src/lib.rs
pub struct App;
"#,
    );

    fixture.check_save(
        r#"
//- /crates/new_pkg/Cargo.toml
[package]
name = "new_pkg"
version = "0.1.0"
edition = "2024"
"#,
        &[HostObservation::workspace_symbols("NewType")],
        expect![[r#"
            changed files
            - <none>

            changed targets
            - <none>

            workspace symbols `NewType`
            - <none>
        "#]],
    );

    fixture.check_save(
        r#"
//- /crates/new_pkg/src/lib.rs
pub struct NewType;
"#,
        &[HostObservation::workspace_symbols("NewType")],
        expect![[r#"
            changed files
            - new_pkg crates/new_pkg/src/lib.rs

            changed targets
            - app[lib]
            - new_pkg[lib]

            workspace symbols `NewType`
            - struct NewType @ new_pkg[lib] crates/new_pkg/src/lib.rs
        "#]],
    );
}

#[test]
fn workspace_graph_change_reports_changed_crates() {
    let mut fixture = HostFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "offloaded_target_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Lib;

//- /src/tool.rs
fn main() {}
"#,
    );

    fixture.check_save(
        r#"
//- /Cargo.toml
[package]
name = "offloaded_target_fixture"
version = "0.1.0"
edition = "2024"

[[bin]]
name = "tool"
path = "src/tool.rs"
"#,
        &[],
        expect![[r#"
            changed files
            - <none>

            changed targets
            - offloaded_target_fixture[bin]
            - offloaded_target_fixture[lib]
        "#]],
    );
}

#[test]
fn rebuilds_project_after_auto_discovered_target_is_added() {
    let mut fixture = HostFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "autotarget_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Lib;
"#,
    );

    fixture.check_save(
        r#"
//- /tests/smoke.rs
#[test]
fn smoke() {}
"#,
        &[HostObservation::file_context(
            "smoke test",
            "tests/smoke.rs",
        )],
        expect![[r#"
            changed files
            - autotarget_fixture tests/smoke.rs

            changed targets
            - autotarget_fixture[lib]
            - autotarget_fixture[test]

            file context `smoke test`
            - autotarget_fixture tests/smoke.rs -> autotarget_fixture[test]
        "#]],
    );
}

#[test]
fn updates_existing_auto_discovered_target() {
    let mut fixture = HostFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "autotarget_update_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Lib;

//- /tests/smoke.rs
#[test]
fn smoke() {}
"#,
    );

    fixture.check_save(
        r#"
//- /tests/smoke.rs
#[test]
fn changed_smoke() {}
"#,
        &[],
        expect![[r#"
            changed files
            - autotarget_update_fixture tests/smoke.rs

            changed targets
            - autotarget_update_fixture[test]
        "#]],
    );
}

#[test]
fn rebuilds_project_after_lockfile_changes() {
    let mut fixture = HostFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "lock_fixture"
version = "0.1.0"
edition = "2024"

//- /Cargo.lock
# This file is automatically @generated by Cargo.
# It is not intended for manual editing.
version = 3

[[package]]
name = "lock_fixture"
version = "0.1.0"

//- /src/lib.rs
pub struct Lib;
"#,
    );

    fixture.check_save(
        r#"
//- /Cargo.lock
# This file is automatically @generated by Cargo.
# It is not intended for manual editing.
# Saved lockfile change.
version = 3

[[package]]
name = "lock_fixture"
version = "0.1.0"
"#,
        &[],
        expect![[r#"
            changed files
            - <none>

            changed targets
            - lock_fixture[lib]
        "#]],
    );
}

#[test]
fn selects_lsp_file_context_from_paths() {
    let fixture = HostFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "file_context_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod shared;

//- /src/main.rs
mod shared;

fn main() {}

//- /src/shared.rs
pub struct Shared;

//- /src/orphan.rs
pub struct Orphan;
"#,
    );

    fixture.check(
        &[
            HostObservation::file_context("lib root", "src/lib.rs"),
            HostObservation::file_context("bin root", "src/main.rs"),
            HostObservation::file_context("shared module", "src/shared.rs"),
            HostObservation::file_context("orphan file", "src/orphan.rs"),
        ],
        expect![[r#"
            file context `lib root`
            - file_context_fixture src/lib.rs -> file_context_fixture[lib]

            file context `bin root`
            - file_context_fixture src/main.rs -> file_context_fixture[bin]

            file context `shared module`
            - file_context_fixture src/shared.rs -> file_context_fixture[lib]

            file context `orphan file`
            - <none>
        "#]],
    );
}

#[test]
fn discovers_new_saved_module_files() {
    let mut fixture = HostFixture::build(
        r#"
//- /Cargo.toml
[workspace]
members = ["crates/dep", "crates/app"]
resolver = "3"

//- /crates/dep/Cargo.toml
[package]
name = "dep"
version = "0.1.0"
edition = "2024"

//- /crates/dep/src/lib.rs
pub struct Root;

//- /crates/app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "../dep" }

//- /crates/app/src/lib.rs
pub fn use_dep(value: dep::api::Api) {
    let same = val$0ue;
}
"#,
    );

    fixture.check_save(
        r#"
//- /crates/dep/src/lib.rs
pub mod api;
pub struct Root;
"#,
        &[HostObservation::type_names_at("app marker 0", "app", "0")],
        expect![[r#"
            changed files
            - dep crates/dep/src/lib.rs

            changed targets
            - dep[lib]

            type names at `app marker 0`
            - <none>
        "#]],
    );

    fixture.check_save(
        r#"
//- /crates/dep/src/api.rs
pub struct Api;
"#,
        &[HostObservation::type_names_at("app marker 0", "app", "0")],
        expect![[r#"
            changed files
            - dep crates/dep/src/api.rs

            changed targets
            - dep[lib]

            type names at `app marker 0`
            - Api
        "#]],
    );
}

#[test]
fn removes_modules_from_index_after_mod_declarations_are_removed() {
    let mut fixture = HostFixture::build(
        r#"
//- /Cargo.toml
[workspace]
members = ["crates/dep", "crates/app"]
resolver = "3"

//- /crates/dep/Cargo.toml
[package]
name = "dep"
version = "0.1.0"
edition = "2024"

//- /crates/dep/src/lib.rs
pub mod api;
pub struct Root;

//- /crates/dep/src/api.rs
pub struct Api;

//- /crates/app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "../dep" }

//- /crates/app/src/lib.rs
pub fn use_dep(value: dep::api::Api) {
    let same = val$0ue;
}
"#,
    );

    fixture.check(
        &[
            HostObservation::type_names_at("app marker 0", "app", "0"),
            HostObservation::workspace_symbols("Api"),
        ],
        expect![[r#"
            type names at `app marker 0`
            - Api

            workspace symbols `Api`
            - module api @ dep[lib] crates/dep/src/lib.rs
            - struct Api @ dep[lib] crates/dep/src/api.rs
        "#]],
    );

    fixture.check_save(
        r#"
//- /crates/dep/src/lib.rs
pub struct Root;
"#,
        &[
            HostObservation::type_names_at("app marker 0", "app", "0"),
            HostObservation::workspace_symbols("Api"),
        ],
        expect![[r#"
            changed files
            - dep crates/dep/src/api.rs
            - dep crates/dep/src/lib.rs

            changed targets
            - dep[lib]

            type names at `app marker 0`
            - <none>

            workspace symbols `Api`
            - <none>
        "#]],
    );
}

#[test]
fn persisted_lookup_env_follows_dependency_declarations() {
    let source = r#"
//- /Cargo.toml
[workspace]
members = ["app", "dep"]
resolver = "3"

//- /dep/Cargo.toml
[package]
name = "dep"
version = "0.1.0"
edition = "2024"

//- /dep/src/lib.rs
pub trait Existing {}
#[lang = "deref"]
pub trait FirstDeref {}

//- /app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "../dep" }

//- /app/src/lib.rs
pub struct App;
"#;
    let mut fixture = ProjectFixture::build(source);
    let app = rg_ir_model::CrateRef {
        package: fixture.package_slot_by_name("app"),
        crate_id: rg_ir_model::CrateId(0),
    };
    let selected_name = |project: &Project| {
        project.inspect(|snapshot| {
            let reader = rg_semantic_ir::ItemReader(snapshot.declarations());
            let items = &reader;
            let query = rg_semantic_ir::ItemLookupQuery::build_from(
                reader,
                app,
                &rg_std::CancellationToken::new(),
            )
            .expect("saved lookup environment should load");
            let selected = query
                .lang_trait(rg_item_tree::LangItem::Deref)
                .expect("dependency supplies the language item");
            rg_semantic_ir::ItemStoreQuery::new(items)
                .trait_data(selected)
                .expect("selected trait should load")
                .expect("selected trait exists")
                .name
                .to_string()
        })
    };
    assert_eq!(selected_name(fixture.project()), "FirstDeref");

    // Move the language item to another trait id. A dependent's stale saved mapping would
    // select Existing instead of SecondDeref even if the dependency's own items were refreshed.
    fixture.apply_saved_fixture(
        r#"
//- /dep/src/lib.rs
#[lang = "deref"]
pub trait SecondDeref {}
pub trait Existing {}
"#,
    );
    assert_eq!(selected_name(fixture.project()), "SecondDeref");

    let source = fixture.source();
    let reloaded = source.build_project();
    assert_eq!(selected_name(&reloaded), "SecondDeref");
}

#[test]
fn updates_dependent_lookup_after_dependency_changes() {
    let mut fixture = HostFixture::build(
        r#"
//- /Cargo.toml
[workspace]
members = ["crates/dep", "crates/app"]
resolver = "3"

//- /crates/dep/Cargo.toml
[package]
name = "dep"
version = "0.1.0"
edition = "2024"

//- /crates/dep/src/lib.rs
pub struct Api;

//- /crates/app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "../dep" }

//- /crates/app/src/lib.rs
pub fn use_dep(value: dep::Api) {
    let same = val$0ue;
}
"#,
    );

    fixture.check(
        &[HostObservation::type_names_at("app marker 0", "app", "0")],
        expect![[r#"
            type names at `app marker 0`
            - Api
        "#]],
    );

    fixture.check_save(
        r#"
//- /crates/dep/src/lib.rs
pub struct Renamed;
"#,
        &[HostObservation::type_names_at("app marker 0", "app", "0")],
        expect![[r#"
            changed files
            - dep crates/dep/src/lib.rs

            changed targets
            - dep[lib]

            type names at `app marker 0`
            - <none>
        "#]],
    );
}

#[test]
fn updates_transitive_dependent_lookup_after_dependency_changes() {
    let mut fixture = HostFixture::build(
        r#"
//- /Cargo.toml
[workspace]
members = ["crates/dep", "crates/mid", "crates/app"]
resolver = "3"

//- /crates/dep/Cargo.toml
[package]
name = "dep"
version = "0.1.0"
edition = "2024"

//- /crates/dep/src/lib.rs
pub struct Api;

//- /crates/mid/Cargo.toml
[package]
name = "mid"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "../dep" }

//- /crates/mid/src/lib.rs
pub fn make() -> dep::Api {
    loop {}
}

//- /crates/app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
mid = { path = "../mid" }

//- /crates/app/src/lib.rs
pub fn use_mid() {
    let value = mid::make();
    let same = val$0ue;
}
"#,
    );

    fixture.check(
        &[HostObservation::type_names_at("app marker 0", "app", "0")],
        expect![[r#"
            type names at `app marker 0`
            - Api
        "#]],
    );

    fixture.check_save(
        r#"
//- /crates/dep/src/lib.rs
pub struct Renamed;
"#,
        &[HostObservation::type_names_at("app marker 0", "app", "0")],
        expect![[r#"
            changed files
            - dep crates/dep/src/lib.rs

            changed targets
            - dep[lib]

            type names at `app marker 0`
            - <none>
        "#]],
    );
}
