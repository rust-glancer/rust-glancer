mod utils;

use expect_test::expect;
use rg_workspace::WorkspaceMetadata;
use test_fixture::fixture_crate;

use self::utils::{HostFixture, HostObservation, parse_dirty_text};
use crate::{PackageResidencyPolicy, Project};

#[test]
fn timing_profile_reports_phase_checkpoints_without_memory_sampling() {
    let fixture = fixture_crate(
        r#"
//- /Cargo.toml
[package]
name = "timing_profile_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;
"#,
    );
    let workspace = WorkspaceMetadata::from_cargo(fixture.metadata())
        .expect("fixture workspace metadata should build");
    let (_project, profile) = Project::builder(workspace)
        .profile_build_timing(true)
        .build()
        .expect("timing-profiled project build should succeed")
        .into_parts();
    let profile = profile.expect("timing profiling should produce a profile");

    assert_eq!(
        profile.checkpoints().len(),
        10,
        "timing profile should report the same build checkpoints as memory profiling"
    );
    assert!(
        profile
            .checkpoints()
            .iter()
            .all(|checkpoint| checkpoint.phase_elapsed <= checkpoint.elapsed),
        "phase durations should be bounded by cumulative elapsed time"
    );
    assert!(
        profile.checkpoints().iter().all(|checkpoint| {
            checkpoint.retained_bytes.is_none()
                && checkpoint.active_retained_bytes.is_none()
                && checkpoint.allocated_bytes.is_none()
                && checkpoint.active_bytes.is_none()
                && checkpoint.resident_bytes.is_none()
        }),
        "timing-only profiling should not run memory samplers"
    );
}

#[test]
fn profiled_build_reports_phase_checkpoints_without_exposing_phase_dbs() {
    let fixture = fixture_crate(
        r#"
//- /Cargo.toml
[package]
name = "profile_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;
"#,
    );
    let workspace = WorkspaceMetadata::from_cargo(fixture.metadata())
        .expect("fixture workspace metadata should build");
    let (_project, profile) = Project::builder(workspace)
        .measure_retained_memory(true)
        .build()
        .expect("profiled project build should succeed")
        .into_parts();
    let profile = profile.expect("retained memory profiling should produce a profile");

    let labels = profile
        .checkpoints()
        .iter()
        .map(|checkpoint| checkpoint.label)
        .collect::<Vec<_>>();
    assert_eq!(
        labels,
        [
            "after parse",
            "after cache probe",
            "after item-tree",
            "after cache source fingerprints",
            "after def-map",
            "after semantic-ir",
            "after item-tree drop",
            "after body-ir",
            "after parse syntax eviction",
            "after project",
        ]
    );

    assert!(
        profile
            .checkpoints()
            .iter()
            .filter(|checkpoint| checkpoint.retained_bytes.is_some())
            .all(|checkpoint| checkpoint
                .retained_bytes
                .expect("retained bytes should exist")
                > 0),
        "retained checkpoints should record non-zero memory"
    );
    assert!(
        profile
            .checkpoints()
            .iter()
            .all(|checkpoint| checkpoint.active_retained_bytes.is_some()),
        "retained profiling should record active live-state memory for every checkpoint"
    );

    let item_tree_drop = profile
        .checkpoints()
        .iter()
        .find(|checkpoint| checkpoint.label == "after item-tree drop")
        .expect("profile should contain item-tree drop checkpoint");
    assert_eq!(
        item_tree_drop.retained_bytes, None,
        "process-only checkpoints should not pretend to sample a dropped phase object"
    );
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

            affected packages
            - host_update_fixture

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
fn dirty_overlay_rebuilds_analysis_without_mutating_saved_project() {
    let fixture = HostFixture::build_with_package_residency_policy(
        r#"
//- /Cargo.toml
[package]
name = "dirty_overlay_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod other;

pub struct Saved;

pub fn saved_body(value: Saved) {
    let _ = value;
}

//- /src/other.rs
pub fn untouched_body() {}
"#,
        PackageResidencyPolicy::AllOffloadable,
    );
    let (dirty_text, cursors) = parse_dirty_text(
        r#"
mod other;

pub struct Dirty {
    pub field: u8,
}

pub fn dirty_body(value: Dirty) {
    value.$receiver$
}
"#,
    );

    let saved_before = fixture.render(&[
        HostObservation::resident_stats("saved before dirty overlay"),
        HostObservation::workspace_symbols("Saved"),
        HostObservation::workspace_symbols("Dirty"),
    ]);
    let overlay = fixture.dirty_overlay("src/lib.rs", &dirty_text);
    let dirty_overlay = fixture.render_dirty_project(
        &overlay,
        &dirty_text,
        &[
            HostObservation::resident_stats("dirty overlay"),
            HostObservation::body_ir_stats("dirty overlay"),
            HostObservation::workspace_symbols("Dirty"),
            HostObservation::workspace_symbols("Saved"),
            HostObservation::completions_at("dirty receiver", "src/lib.rs", cursors["receiver"]),
        ],
    );
    let saved_after = fixture.render(&[
        HostObservation::resident_stats("saved after dirty overlay"),
        HostObservation::workspace_symbols("Saved"),
        HostObservation::workspace_symbols("Dirty"),
    ]);

    let actual = format!(
        "saved project before dirty overlay\n{}\n\ndirty overlay\n{}\n\nsaved project after dirty overlay\n{}\n",
        saved_before.trim_end(),
        dirty_overlay.trim_end(),
        saved_after.trim_end(),
    );
    expect![[r#"
        saved project before dirty overlay
        resident stats `saved before dirty overlay`
        - def-map targets 0
        - semantic targets 0
        - body targets 0

        workspace symbols `Saved`
        - fn saved_body @ dirty_overlay_fixture[lib] src/lib.rs
        - struct Saved @ dirty_overlay_fixture[lib] src/lib.rs

        workspace symbols `Dirty`
        - <none>

        dirty overlay
        resident stats `dirty overlay`
        - def-map targets 1
        - semantic targets 1
        - body targets 1

        body ir stats `dirty overlay`
        - targets 1
        - bodies 1

        workspace symbols `Dirty`
        - fn dirty_body @ dirty_overlay_fixture[lib] src/lib.rs
        - struct Dirty @ dirty_overlay_fixture[lib] src/lib.rs

        workspace symbols `Saved`
        - <none>

        completions at `dirty receiver`
        - field field

        saved project after dirty overlay
        resident stats `saved after dirty overlay`
        - def-map targets 0
        - semantic targets 0
        - body targets 0

        workspace symbols `Saved`
        - fn saved_body @ dirty_overlay_fixture[lib] src/lib.rs
        - struct Saved @ dirty_overlay_fixture[lib] src/lib.rs

        workspace symbols `Dirty`
        - <none>
    "#]]
    .assert_eq(&actual);
}

#[test]
fn dirty_overlay_completes_keywords_after_parse_syntax_eviction() {
    let fixture = HostFixture::build_with_package_residency_policy(
        r#"
//- /Cargo.toml
[package]
name = "dirty_overlay_keyword_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Saved;
"#,
        PackageResidencyPolicy::AllOffloadable,
    );
    let (dirty_text, cursors) = parse_dirty_text(
        r#"
f$item$
im$impl_item$

pub struct Dirty;

pub fn use_it() {
    le$statement$
    let _value = ("https://example.test", ma$expression$);
    let _bare = $bare_expression$;
}
"#,
    );

    let overlay = fixture.dirty_overlay("src/lib.rs", &dirty_text);
    let actual = fixture.render_dirty_project(
        &overlay,
        &dirty_text,
        &[
            HostObservation::completions_at("dirty item keyword", "src/lib.rs", cursors["item"]),
            HostObservation::completions_at(
                "dirty impl keyword",
                "src/lib.rs",
                cursors["impl_item"],
            ),
            HostObservation::completions_at(
                "dirty statement keyword",
                "src/lib.rs",
                cursors["statement"],
            ),
            HostObservation::completions_at(
                "dirty expression keyword",
                "src/lib.rs",
                cursors["expression"],
            ),
            HostObservation::completions_at(
                "dirty bare expression keyword",
                "src/lib.rs",
                cursors["bare_expression"],
            ),
        ],
    );

    expect![[r#"
        completions at `dirty item keyword`
        - keyword fn

        completions at `dirty impl keyword`
        - keyword impl
        - keyword impl for

        completions at `dirty statement keyword`
        - struct Dirty
        - keyword let
        - fn use_it

        completions at `dirty expression keyword`
        - struct Dirty
        - keyword match
        - fn use_it

        completions at `dirty bare expression keyword`
        - keyword async
        - keyword false
        - keyword if
        - keyword loop
        - keyword match
        - keyword move
        - keyword return
        - keyword true
    "#]]
    .assert_eq(&actual);
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

            affected packages
            - host_new_module_fixture

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

            affected packages
            - app
            - dep

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
        &[HostObservation::file_contexts("tool file", "src/tool.rs")],
        expect![[r#"
            file contexts `tool file`
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
        &[HostObservation::file_contexts("tool file", "src/tool.rs")],
        expect![[r#"
            changed files
            - <none>

            affected packages
            - target_fixture

            changed targets
            - target_fixture[bin]
            - target_fixture[lib]

            file contexts `tool file`
            - target_fixture src/tool.rs -> target_fixture[bin]
        "#]],
    );
}

#[test]
fn workspace_graph_rebuild_reports_changed_targets_when_packages_are_offloaded() {
    let mut fixture = HostFixture::build_with_package_residency_policy(
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
        PackageResidencyPolicy::AllOffloadable,
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

            affected packages
            - offloaded_target_fixture

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
        &[HostObservation::file_contexts(
            "smoke test",
            "tests/smoke.rs",
        )],
        expect![[r#"
            changed files
            - autotarget_fixture tests/smoke.rs

            affected packages
            - autotarget_fixture

            changed targets
            - autotarget_fixture[lib]
            - autotarget_fixture[test]

            file contexts `smoke test`
            - autotarget_fixture tests/smoke.rs -> autotarget_fixture[test]
        "#]],
    );
}

#[test]
fn updates_existing_auto_discovered_target_without_full_rebuild() {
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

            affected packages
            - autotarget_update_fixture

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

            affected packages
            - lock_fixture

            changed targets
            - lock_fixture[lib]
        "#]],
    );
}

#[test]
fn resolves_lsp_file_contexts_from_paths() {
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
            HostObservation::file_contexts("lib root", "src/lib.rs"),
            HostObservation::file_contexts("bin root", "src/main.rs"),
            HostObservation::file_contexts("shared module", "src/shared.rs"),
            HostObservation::file_contexts("orphan file", "src/orphan.rs"),
        ],
        expect![[r#"
            file contexts `lib root`
            - file_context_fixture src/lib.rs -> file_context_fixture[lib]

            file contexts `bin root`
            - file_context_fixture src/main.rs -> file_context_fixture[bin]

            file contexts `shared module`
            - file_context_fixture src/shared.rs -> file_context_fixture[bin], file_context_fixture[lib]

            file contexts `orphan file`
            - <none>
        "#]],
    );
}

#[test]
fn rebuilds_package_roots_for_new_saved_module_files() {
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

            affected packages
            - app
            - dep

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

            affected packages
            - app
            - dep

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
            - dep crates/dep/src/lib.rs

            affected packages
            - app
            - dep

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
fn reports_reverse_dependent_packages_as_affected() {
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
pub fn use_dep(_: dep::Api) {}
"#,
    );

    fixture.check_save(
        r#"
//- /crates/dep/src/lib.rs
pub struct Api;
pub struct Extra;
"#,
        &[],
        expect![[r#"
            changed files
            - dep crates/dep/src/lib.rs

            affected packages
            - app
            - dep

            changed targets
            - dep[lib]
        "#]],
    );
}

#[test]
fn rebuilds_reverse_dependent_packages_after_dependency_changes() {
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

            affected packages
            - app
            - dep

            changed targets
            - dep[lib]

            type names at `app marker 0`
            - <none>
        "#]],
    );
}

#[test]
fn rebuilds_offloaded_path_dependency_after_source_change() {
    let mut fixture = HostFixture::build_with_package_residency_policy(
        r#"
//- /Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "dep" }

//- /src/lib.rs
pub fn use_dep(_: dep::Api) {}

//- /dep/Cargo.toml
[package]
name = "dep"
version = "0.1.0"
edition = "2024"

//- /dep/src/lib.rs
pub struct Api;
"#,
        PackageResidencyPolicy::WorkspaceResident,
    );

    fixture.check(
        &[HostObservation::workspace_symbols("Api")],
        expect![[r#"
            workspace symbols `Api`
            - struct Api @ dep[lib] dep/src/lib.rs
        "#]],
    );

    fixture.check_save(
        r#"
//- /dep/src/lib.rs
pub struct Renamed;
"#,
        &[
            HostObservation::workspace_symbols("Renamed"),
            HostObservation::workspace_symbols("Api"),
        ],
        expect![[r#"
            changed files
            - dep dep/src/lib.rs

            affected packages
            - app
            - dep

            changed targets
            - dep[lib]

            workspace symbols `Renamed`
            - struct Renamed @ dep[lib] dep/src/lib.rs

            workspace symbols `Api`
            - <none>
        "#]],
    );
}

#[test]
fn queries_report_missing_offloaded_package_cache_artifacts() {
    let fixture = HostFixture::build_with_package_residency_policy(
        r#"
//- /Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "dep" }

//- /src/lib.rs
pub fn use_dep(_: dep::Api) {}

//- /dep/Cargo.toml
[package]
name = "dep"
version = "0.1.0"
edition = "2024"

//- /dep/src/lib.rs
pub struct Api;
"#,
        PackageResidencyPolicy::WorkspaceResident,
    );

    assert!(fixture.package_cache_artifact_exists("dep"));
    fixture.remove_cache_namespace();
    assert!(!fixture.package_cache_artifact_exists("dep"));

    let error = fixture.workspace_symbols_error("Api");
    assert!(
        error.contains("offloaded package slot PackageSlot(1) is missing from backing storage"),
        "{error}",
    );
    assert!(!fixture.package_cache_artifact_exists("dep"));
}

#[test]
fn queries_report_corrupt_offloaded_package_cache_artifacts() {
    let fixture = HostFixture::build_with_package_residency_policy(
        r#"
//- /Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "dep" }

//- /src/lib.rs
pub fn use_dep(_: dep::Api) {}

//- /dep/Cargo.toml
[package]
name = "dep"
version = "0.1.0"
edition = "2024"

//- /dep/src/lib.rs
pub struct Api;
"#,
        PackageResidencyPolicy::WorkspaceResident,
    );

    fixture.corrupt_package_cache_artifact("dep");

    let error = fixture.workspace_symbols_error("Api");
    assert!(
        error.contains("offloaded package slot PackageSlot(1) has malformed cache data"),
        "{error}",
    );
    assert!(fixture.package_cache_artifact_exists("dep"));
}

#[test]
fn file_local_queries_do_not_materialize_unrelated_offloaded_packages() {
    let fixture = HostFixture::build_with_package_residency_policy(
        r#"
//- /Cargo.toml
[workspace]
members = ["app", "dep", "unrelated"]
resolver = "3"

//- /app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "../dep" }

//- /app/src/lib.rs
pub struct Local;
pub fn use_dep(_: dep::Api) {}

//- /dep/Cargo.toml
[package]
name = "dep"
version = "0.1.0"
edition = "2024"

//- /dep/src/lib.rs
pub struct Api;

//- /unrelated/Cargo.toml
[package]
name = "unrelated"
version = "0.1.0"
edition = "2024"

//- /unrelated/src/lib.rs
pub struct Unrelated;
"#,
        PackageResidencyPolicy::AllOffloadable,
    );

    assert!(fixture.package_cache_artifact_exists("unrelated"));
    fixture.remove_package_cache_artifact("unrelated");
    assert!(!fixture.package_cache_artifact_exists("unrelated"));

    assert_eq!(
        fixture.document_symbol_names("app/src/lib.rs"),
        vec!["Local", "use_dep"],
    );
    assert!(
        !fixture.package_cache_artifact_exists("unrelated"),
        "narrow file-local queries should not recover artifacts outside their package subset",
    );
}

#[test]
fn source_updates_do_not_materialize_unrelated_offloaded_packages() {
    let mut fixture = HostFixture::build_with_package_residency_policy(
        r#"
//- /Cargo.toml
[workspace]
members = ["app", "dep", "unrelated"]
resolver = "3"

//- /app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "../dep" }

//- /app/src/lib.rs
pub struct Before;
pub fn use_dep(_: dep::Api) {}

//- /dep/Cargo.toml
[package]
name = "dep"
version = "0.1.0"
edition = "2024"

//- /dep/src/lib.rs
pub struct Api;

//- /unrelated/Cargo.toml
[package]
name = "unrelated"
version = "0.1.0"
edition = "2024"

//- /unrelated/src/lib.rs
pub struct Unrelated;
"#,
        PackageResidencyPolicy::AllOffloadable,
    );

    assert!(fixture.package_cache_artifact_exists("unrelated"));
    fixture.remove_package_cache_artifact("unrelated");
    assert!(!fixture.package_cache_artifact_exists("unrelated"));

    fixture.check_save(
        r#"
//- /app/src/lib.rs
pub struct After;
pub fn use_dep(_: dep::Api) {}
"#,
        &[HostObservation::resident_stats("after save")],
        expect![[r#"
            changed files
            - app app/src/lib.rs

            affected packages
            - app

            changed targets
            - app[lib]

            resident stats `after save`
            - def-map targets 0
            - semantic targets 0
            - body targets 0
        "#]],
    );

    assert_eq!(
        fixture.document_symbol_names("app/src/lib.rs"),
        vec!["After", "use_dep"],
    );
    assert!(
        !fixture.package_cache_artifact_exists("unrelated"),
        "source updates should not recover artifacts outside their rebuild package subset",
    );
}

#[test]
fn source_updates_rebuild_missing_offloaded_package_cache_artifacts() {
    let mut fixture = HostFixture::build_with_package_residency_policy(
        r#"
//- /Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "dep" }

//- /src/lib.rs
use dep::Api;
pub struct Before;
pub fn use_dep(_: Api) {}

//- /dep/Cargo.toml
[package]
name = "dep"
version = "0.1.0"
edition = "2024"

//- /dep/src/lib.rs
pub struct Api;
"#,
        PackageResidencyPolicy::WorkspaceResident,
    );

    fixture.remove_cache_namespace();

    fixture.check_save(
        r#"
//- /src/lib.rs
use dep::Api;
pub struct After;
pub fn use_dep(_: Api) {}
"#,
        &[
            HostObservation::workspace_symbols("After"),
            HostObservation::workspace_symbols("Api"),
        ],
        expect![[r#"
            changed files
            - app src/lib.rs

            affected packages
            - app

            changed targets
            - app[lib]

            workspace symbols `After`
            - struct After @ app[lib] src/lib.rs

            workspace symbols `Api`
            - struct Api @ dep[lib] dep/src/lib.rs
        "#]],
    );
    assert!(fixture.package_cache_artifact_exists("dep"));
}

#[test]
fn source_updates_rebuild_corrupt_offloaded_package_cache_artifacts() {
    let mut fixture = HostFixture::build_with_package_residency_policy(
        r#"
//- /Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "dep" }

//- /src/lib.rs
use dep::Api;
pub struct Before;
pub fn use_dep(_: Api) {}

//- /dep/Cargo.toml
[package]
name = "dep"
version = "0.1.0"
edition = "2024"

//- /dep/src/lib.rs
pub struct Api;
"#,
        PackageResidencyPolicy::WorkspaceResident,
    );

    fixture.corrupt_package_cache_artifact("dep");

    fixture.check_save(
        r#"
//- /src/lib.rs
use dep::Api;
pub struct After;
pub fn use_dep(_: Api) {}
"#,
        &[
            HostObservation::workspace_symbols("After"),
            HostObservation::workspace_symbols("Api"),
        ],
        expect![[r#"
            changed files
            - app src/lib.rs

            affected packages
            - app

            changed targets
            - app[lib]

            workspace symbols `After`
            - struct After @ app[lib] src/lib.rs

            workspace symbols `Api`
            - struct Api @ dep[lib] dep/src/lib.rs
        "#]],
    );
    assert!(fixture.package_cache_artifact_exists("dep"));
}

#[test]
fn source_updates_restore_offloaded_residency_for_unchanged_packages() {
    let mut fixture = HostFixture::build_with_package_residency_policy(
        r#"
//- /Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "dep" }

//- /src/lib.rs
pub struct Before;
pub fn use_dep(_: dep::Api) {}

//- /dep/Cargo.toml
[package]
name = "dep"
version = "0.1.0"
edition = "2024"

//- /dep/src/lib.rs
pub struct Api;
"#,
        PackageResidencyPolicy::AllOffloadable,
    );

    fixture.check(
        &[HostObservation::resident_stats("after build")],
        expect![[r#"
            resident stats `after build`
            - def-map targets 0
            - semantic targets 0
            - body targets 0
        "#]],
    );

    fixture.check_save(
        r#"
//- /src/lib.rs
pub struct After;
pub fn use_dep(_: dep::Api) {}
"#,
        &[
            HostObservation::resident_stats("after save"),
            HostObservation::workspace_symbols("After"),
        ],
        expect![[r#"
            changed files
            - app src/lib.rs

            affected packages
            - app

            changed targets
            - app[lib]

            resident stats `after save`
            - def-map targets 0
            - semantic targets 0
            - body targets 0

            workspace symbols `After`
            - struct After @ app[lib] src/lib.rs
        "#]],
    );
}

#[test]
fn rebuilds_transitive_reverse_dependent_packages_after_dependency_changes() {
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

            affected packages
            - app
            - dep
            - mid

            changed targets
            - dep[lib]

            type names at `app marker 0`
            - <none>
        "#]],
    );
}
