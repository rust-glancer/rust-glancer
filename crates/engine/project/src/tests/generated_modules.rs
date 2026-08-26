use std::{
    collections::BTreeSet,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use expect_test::expect;

use super::{
    SourceMutationMemoryHooks,
    utils::{HostFixture, HostObservation},
};
use crate::{
    PackageResidencyPolicy, Project, ProjectMemoryHooks, ProjectMemoryPurgePoint, SavedFileChange,
    testonly::{ProjectFixture, ProjectSourceFixture},
};

/// Mutates one fixture path at a chosen ItemTree syntax-eviction boundary.
///
/// Generated discovery adds another eviction after the initial ItemTree pass. Counting those
/// boundaries lets the race test change a late file after capture but before source validation.
#[derive(Debug)]
struct NthItemTreeMutationMemoryHooks {
    remaining_item_tree_points: AtomicUsize,
    path: PathBuf,
    replacement: &'static str,
}

impl ProjectMemoryHooks for NthItemTreeMutationMemoryHooks {
    fn purge(&self, point: ProjectMemoryPurgePoint) {
        if point != ProjectMemoryPurgePoint::AfterItemTreeSyntaxEviction {
            return;
        }
        let should_mutate = self
            .remaining_item_tree_points
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok_and(|previous| previous == 1);
        if should_mutate {
            std::fs::write(&self.path, self.replacement)
                .expect("late source mutation hook should replace fixture source");
        }
    }
}

#[test]
fn fresh_build_rejects_generated_module_changed_during_discovery() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "generated_source_generation_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
macro_rules! make_late_module {
    () => {
        mod late;
    };
}

make_late_module!();

//- /src/late.rs
pub struct Before;
"#,
    );
    let hooks: Arc<dyn ProjectMemoryHooks> = Arc::new(NthItemTreeMutationMemoryHooks {
        remaining_item_tree_points: AtomicUsize::new(2),
        path: fixture.path("src/late.rs"),
        replacement: "pub struct After;\n",
    });

    let error = Project::builder(fixture.workspace_metadata())
        .memory_hooks(hooks)
        .build()
        .expect_err("a generated module change during discovery should invalidate the candidate");
    assert!(
        error.chain().any(|cause| matches!(
            cause.downcast_ref::<rg_source::SourceError>(),
            Some(rg_source::SourceError::Stale { .. })
        )),
        "build failure should retain the late source race: {error:#}",
    );
}

#[test]
fn fresh_build_rejects_generated_module_created_after_missing_probe() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "generated_source_existence_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
macro_rules! make_late_module {
    () => {
        mod late;
    };
}

make_late_module!();
"#,
    );
    let hooks: Arc<dyn ProjectMemoryHooks> = Arc::new(SourceMutationMemoryHooks::at_point(
        ProjectMemoryPurgePoint::AfterDefMapBuild,
        fixture.path("src/late.rs"),
        "pub struct Appeared;\n",
    ));

    let error = Project::builder(fixture.workspace_metadata())
        .memory_hooks(hooks)
        .build()
        .expect_err("a generated module appearing after its probe should invalidate the candidate");
    assert!(
        error.chain().any(|cause| matches!(
            cause.downcast_ref::<rg_source::SourceError>(),
            Some(rg_source::SourceError::ExistenceChanged { .. })
        )),
        "build failure should retain the generated source-existence cause: {error:#}",
    );
}

#[test]
fn macro_generated_out_of_line_modules_converge_through_real_files() {
    let fixture = HostFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "generated_module_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
macro_rules! make_platform {
    () => {
        pub mod platform;
    };
}

make_platform!();
pub use platform::{Deep, Nested, Ordinary};

pub fn consume(_: De$type$ep) {}

//- /src/platform.rs
macro_rules! make_imp {
    () => {
        #[path = "custom_imp.rs"]
        pub mod imp;
    };
}

make_imp!();
pub use imp::Deep;
pub mod ordinary;
pub use ordinary::Ordinary;

//- /src/custom_imp.rs
pub struct Deep;
pub mod nested;
pub use nested::Nested;

//- /src/nested.rs
pub struct Nested;

//- /src/platform/ordinary.rs
pub struct Ordinary;
"#,
    );

    fixture.check(
        &[
            HostObservation::workspace_symbols("Deep"),
            HostObservation::workspace_symbols("Nested"),
            HostObservation::workspace_symbols("Ordinary"),
            HostObservation::file_contexts("generated module source", "src/custom_imp.rs"),
            HostObservation::type_names_at(
                "generated reexport",
                "generated_module_fixture",
                "type",
            ),
        ],
        expect![[r#"
            workspace symbols `Deep`
            - struct Deep @ generated_module_fixture[lib] src/custom_imp.rs

            workspace symbols `Nested`
            - module nested @ generated_module_fixture[lib] src/custom_imp.rs
            - struct Nested @ generated_module_fixture[lib] src/nested.rs

            workspace symbols `Ordinary`
            - module ordinary @ generated_module_fixture[lib] src/platform.rs
            - struct Ordinary @ generated_module_fixture[lib] src/platform/ordinary.rs

            file contexts `generated module source`
            - generated_module_fixture src/custom_imp.rs -> generated_module_fixture[lib]

            type names at `generated reexport`
            - Deep
        "#]],
    );
}

#[test]
fn generated_inline_modules_descend_the_module_file_context() {
    let fixture = HostFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "generated_inline_context_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
macro_rules! make_outer {
    () => {
        #[path = "models"]
        pub mod outer {
            pub mod child;
        }
    };
}

make_outer!();
pub use outer::child::Nested;

//- /src/models/child.rs
pub struct Nested;
"#,
    );

    fixture.check(
        &[
            HostObservation::workspace_symbols("Nested"),
            HostObservation::file_contexts("nested generated source", "src/models/child.rs"),
        ],
        expect![[r#"
            workspace symbols `Nested`
            - struct Nested @ generated_inline_context_fixture[lib] src/models/child.rs

            file contexts `nested generated source`
            - generated_inline_context_fixture src/models/child.rs -> generated_inline_context_fixture[lib]
        "#]],
    );
}

#[test]
fn generated_module_collection_terminates_on_file_cycles() {
    let fixture = HostFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "generated_module_cycle_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
macro_rules! make_generated {
    () => {
        pub mod generated;
    };
}

make_generated!();

//- /src/generated.rs
pub struct Generated;

#[path = "cycle.rs"]
pub mod cycle;

//- /src/cycle.rs
pub struct Cycle;

#[path = "generated.rs"]
pub mod generated_again;
"#,
    );

    fixture.check(
        &[
            HostObservation::workspace_symbols("Generated"),
            HostObservation::workspace_symbols("Cycle"),
        ],
        expect![[r#"
            workspace symbols `Generated`
            - module generated @ generated_module_cycle_fixture[lib] src/lib.rs
            - module generated_again @ generated_module_cycle_fixture[lib] src/cycle.rs
            - struct Generated @ generated_module_cycle_fixture[lib] src/generated.rs

            workspace symbols `Cycle`
            - module cycle @ generated_module_cycle_fixture[lib] src/generated.rs
            - struct Cycle @ generated_module_cycle_fixture[lib] src/cycle.rs
        "#]],
    );
}

#[test]
fn generated_modules_in_custom_target_roots_resolve_beside_the_root() {
    let fixture = HostFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "generated_custom_root_fixture"
version = "0.1.0"
edition = "2024"

[lib]
path = "src/tool.rs"

//- /src/tool.rs
macro_rules! make_child {
    () => {
        pub mod child;
    };
}

make_child!();
pub use child::CustomRootChild;

//- /src/child.rs
pub struct CustomRootChild;
"#,
    );

    fixture.check(
        &[HostObservation::workspace_symbols("CustomRootChild")],
        expect![[r#"
            workspace symbols `CustomRootChild`
            - struct CustomRootChild @ generated_custom_root_fixture[lib] src/child.rs
        "#]],
    );
}

#[test]
fn dependency_macros_resolve_nested_modules_at_the_call_site() {
    let fixture = HostFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "generated_dependency_context_fixture"
version = "0.1.0"
edition = "2024"

[dependencies]
macro_dep = { path = "macro_dep" }

//- /src/lib.rs
pub mod outer {
    macro_dep::make_child!();
    pub use child::CallSite;
}

pub use outer::CallSite;

//- /src/outer/child/mod.rs
pub struct CallSite;

//- /macro_dep/Cargo.toml
[package]
name = "macro_dep"
version = "0.1.0"
edition = "2024"

//- /macro_dep/src/lib.rs
#[macro_export]
macro_rules! make_child {
    () => {
        pub mod child;
    };
}

//- /macro_dep/src/child.rs
pub struct DefinitionSite;
"#,
    );

    fixture.check(
        &[
            HostObservation::workspace_symbols("CallSite"),
            HostObservation::workspace_symbols("DefinitionSite"),
            HostObservation::file_contexts("nested call-site source", "src/outer/child/mod.rs"),
        ],
        expect![[r#"
            workspace symbols `CallSite`
            - struct CallSite @ generated_dependency_context_fixture[lib] src/outer/child/mod.rs

            workspace symbols `DefinitionSite`
            - <none>

            file contexts `nested call-site source`
            - generated_dependency_context_fixture src/outer/child/mod.rs -> generated_dependency_context_fixture[lib]
        "#]],
    );
}

#[test]
fn included_files_inherit_each_call_sites_module_context() {
    let fixture = HostFixture::build_with_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "generated_include_context_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub mod left {
    include!("included.rs");
}

pub mod right {
    include!("included.rs");
}

pub use left::{LeftLate, LeftOrdinary};
pub use right::{RightLate, RightOrdinary};

//- /src/included.rs
pub mod ordinary;
pub use ordinary::*;

macro_rules! make_included_module {
    () => {
        pub mod late;
    };
}

make_included_module!();
pub use late::*;

//- /src/left/ordinary.rs
pub struct LeftOrdinary;

//- /src/left/late.rs
pub struct LeftLate;

//- /src/right/ordinary.rs
pub struct RightOrdinary;

//- /src/right/late.rs
pub struct RightLate;

//- /sysroot/library/core/src/lib.rs
#[rustc_builtin_macro]
#[macro_export]
macro_rules! include {
    ($($args:tt)*) => {{ /* compiler built-in */ }};
}

pub mod prelude {
    pub mod rust_2024 {
        pub use crate::include;
    }
}

//- /sysroot/library/alloc/src/lib.rs
pub struct Alloc;

//- /sysroot/library/std/src/lib.rs
#[rustc_builtin_macro]
#[macro_export]
macro_rules! include {
    ($($args:tt)*) => {{ /* compiler built-in */ }};
}

pub mod prelude {
    pub mod rust_2024 {
        pub use crate::include;
    }
}

//- /sysroot/library/proc_macro/src/lib.rs
pub struct TokenStream;
"#,
    );

    fixture.check(
        &[
            HostObservation::workspace_symbols("LeftOrdinary"),
            HostObservation::workspace_symbols("RightOrdinary"),
            HostObservation::workspace_symbols("LeftLate"),
            HostObservation::workspace_symbols("RightLate"),
            HostObservation::file_contexts("left generated source", "src/left/late.rs"),
            HostObservation::file_contexts("right generated source", "src/right/late.rs"),
        ],
        expect![[r#"
            workspace symbols `LeftOrdinary`
            - struct LeftOrdinary @ generated_include_context_fixture[lib] src/left/ordinary.rs

            workspace symbols `RightOrdinary`
            - struct RightOrdinary @ generated_include_context_fixture[lib] src/right/ordinary.rs

            workspace symbols `LeftLate`
            - struct LeftLate @ generated_include_context_fixture[lib] src/left/late.rs

            workspace symbols `RightLate`
            - struct RightLate @ generated_include_context_fixture[lib] src/right/late.rs

            file contexts `left generated source`
            - generated_include_context_fixture src/left/late.rs -> generated_include_context_fixture[lib]

            file contexts `right generated source`
            - generated_include_context_fixture src/right/late.rs -> generated_include_context_fixture[lib]
        "#]],
    );
}

#[test]
fn cyclic_associated_include_expansions_remain_fail_soft() {
    let fixture = HostFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "cyclic_associated_include_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

#[rustc_builtin_macro]
macro_rules! include {
    ($($args:tt)*) => {};
}

impl User {
    include!("methods.rs");
}

//- /src/methods.rs
include!("methods.rs");
"#,
    );

    fixture.check(
        &[HostObservation::workspace_symbols("User")],
        expect![[r#"
            workspace symbols `User`
            - struct User @ cyclic_associated_include_fixture[lib] src/lib.rs
        "#]],
    );
}

#[test]
fn generated_module_files_preserve_cfg_and_macro_use_collection() {
    let fixture = HostFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "generated_macro_use_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
macro_rules! install_child_macros {
    () => {
        #[cfg(any())]
        mod disabled;

        #[macro_use]
        mod child_macros;

        make_child_item!();
    };
}

install_child_macros!();

//- /src/child_macros.rs
macro_rules! make_child_item {
    () => {
        pub struct FromChildMacro;
    };
}
"#,
    );

    fixture.check(
        &[HostObservation::workspace_symbols("FromChildMacro")],
        expect![[r#"
            workspace symbols `FromChildMacro`
            - struct FromChildMacro @ generated_macro_use_fixture[lib] src/lib.rs
        "#]],
    );
}

#[test]
fn generated_module_discovery_profiles_batches_and_coalesced_paths() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "generated_module_profile_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
macro_rules! make_modules {
    () => {
        #[path = "shared.rs"]
        pub mod first;

        #[path = "shared.rs"]
        pub mod second;

        pub mod missing;
    };
}

make_modules!();

//- /src/shared.rs
pub struct Shared;
"#,
    );
    let run = rg_profile::test_support::ProfileTest::start(
        crate::profile_descriptors(),
        "project.build.macro_source_files",
    );

    let project = Project::builder(fixture.workspace_metadata())
        .build()
        .expect("generated module profile fixture should build");
    let snapshot = run.finish();

    assert_eq!(
        project
            .snapshot()
            .parse_db()
            .package(0)
            .expect("generated module profile package should exist")
            .parsed_files()
            .count(),
        2,
        "a missing request should not add a placeholder file",
    );
    assert!(
        project
            .snapshot()
            .full_analysis()
            .expect("generated module profile analysis should build")
            .workspace_symbols("missing")
            .expect("missing generated module query should resolve")
            .is_empty(),
        "a missing request should not allocate a fake semantic module",
    );

    for (path, expected, message) in [
        (
            "project.build.macro_source_files.requests.seen",
            3,
            "one wave should return the two source aliases and one missing module",
        ),
        (
            "project.build.macro_source_files.requests.unique",
            3,
            "each differently spelled generated declaration should retain one request",
        ),
        (
            "project.build.macro_source_files.paths.unique",
            1,
            "the two path overrides should resolve to one package-local source",
        ),
        (
            "project.build.macro_source_files.paths.coalesced",
            1,
            "the second alias should reuse the first path's captured file",
        ),
        (
            "project.build.macro_source_files.paths.missing",
            1,
            "the absent conventional module should be recorded without a fake module",
        ),
        (
            "project.build.macro_source_files.files.discovered",
            1,
            "only shared.rs should enter the parsed package",
        ),
        (
            "project.build.macro_source_files.item_tree.files_lowered",
            1,
            "shared.rs should be lowered exactly once",
        ),
        (
            "project.build.macro_source_files.waves",
            1,
            "the first request batch should discover every available source",
        ),
        (
            "project.build.macro_source_files.def_map_resumes",
            1,
            "one resumable step should apply both source aliases",
        ),
        (
            "project.build.macro_source_files.cache.fingerprint_changes",
            1,
            "late discovery should change the source-built package fingerprint once",
        ),
    ] {
        snapshot.assert_counter_path_with_message(path, expected, message);
    }
}

#[test]
fn generated_module_source_waves_resume_one_def_map_fixed_point() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "generated_module_wave_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
macro_rules! make_outer {
    () => {
        pub mod outer;
    };
}

make_outer!();
pub use outer::inner::Deep;

//- /src/outer.rs
macro_rules! make_inner {
    () => {
        pub mod inner;
    };
}

make_inner!();

//- /src/outer/inner.rs
pub struct Deep;
"#,
    );
    let run = rg_profile::test_support::ProfileTest::start(
        crate::profile_descriptors(),
        "project.build.macro_source_files,def_map.finalization",
    );

    let project = Project::builder(fixture.workspace_metadata())
        .build()
        .expect("nested generated-module fixture should build");
    let profile = run.finish();

    assert_eq!(
        project
            .snapshot()
            .full_analysis()
            .expect("nested generated-module analysis should build")
            .workspace_symbols("Deep")
            .expect("nested generated-module query should resolve")
            .len(),
        1,
        "the second source wave should contribute its declarations",
    );
    profile.assert_counter_path_with_message(
        "project.build.macro_source_files.waves",
        2,
        "the nested macro chain should require two sequential source waves",
    );
    profile.assert_counter_path_with_message(
        "project.build.macro_source_files.def_map_resumes",
        2,
        "each answered source wave should resume the retained DefMap state",
    );
    profile.assert_counter_path_with_message(
        "def_map.finalization.rounds",
        2,
        "source capture should suspend one fixed point instead of starting rounds per wave",
    );
    profile.assert_counter_path_with_message(
        "def_map.finalization.import_resolution.runs",
        2,
        "imports should settle before expansion and once after the source chain drains",
    );
}

#[test]
fn generated_module_requests_coalesce_across_package_targets() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "generated_multi_target_fixture"
version = "0.1.0"
edition = "2024"

[lib]
path = "src/lib.rs"

[[bin]]
name = "generated-bin"
path = "src/main.rs"

//- /src/lib.rs
macro_rules! make_shared {
    () => {
        pub mod shared;
    };
}

make_shared!();

//- /src/main.rs
macro_rules! make_shared {
    () => {
        pub mod shared;
    };
}

make_shared!();

//- /src/shared.rs
pub struct SharedAcrossTargets;
"#,
    );
    let run = rg_profile::test_support::ProfileTest::start(
        crate::profile_descriptors(),
        "project.build.macro_source_files",
    );
    let project = fixture.build_project();
    let profile = run.finish();

    for (path, expected, message) in [
        (
            "project.build.macro_source_files.requests.seen",
            1,
            "equivalent requests from the library and binary should be deduplicated",
        ),
        (
            "project.build.macro_source_files.paths.unique",
            1,
            "both targets should share one package-local generated source",
        ),
        (
            "project.build.macro_source_files.files.discovered",
            1,
            "the shared source should enter the package file table once",
        ),
        (
            "project.build.macro_source_files.def_map_resumes",
            1,
            "one resumable step should apply the shared answer to both targets",
        ),
    ] {
        profile.assert_counter_path_with_message(path, expected, message);
    }

    let snapshot = project.snapshot();
    let contexts = snapshot
        .file_contexts_for_path(fixture.path("src/shared.rs"))
        .expect("shared generated source should have file contexts");
    assert_eq!(
        contexts.len(),
        1,
        "the source should have one package-local file id"
    );
    assert_eq!(
        contexts[0].crates.len(),
        2,
        "the shared source should belong to both package targets",
    );
}

#[test]
fn saved_updates_add_edit_and_prune_generated_module_sources() {
    let mut fixture = HostFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "generated_module_update_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
macro_rules! make_optional {
    () => {
        pub mod optional;
    };
}

make_optional!();
pub use optional::Appeared;
"#,
    );

    fixture.check(
        &[HostObservation::workspace_symbols("Appeared")],
        expect![[r#"
            workspace symbols `Appeared`
            - <none>
        "#]],
    );

    fixture.check_save(
        r#"
//- /src/optional.rs
pub struct Appeared;
"#,
        &[
            HostObservation::workspace_symbols("Appeared"),
            HostObservation::file_contexts("created generated source", "src/optional.rs"),
        ],
        expect![[r#"
            changed files
            - generated_module_update_fixture src/optional.rs

            affected packages
            - generated_module_update_fixture

            changed targets
            - generated_module_update_fixture[lib]

            workspace symbols `Appeared`
            - struct Appeared @ generated_module_update_fixture[lib] src/optional.rs

            file contexts `created generated source`
            - generated_module_update_fixture src/optional.rs -> generated_module_update_fixture[lib]
        "#]],
    );

    fixture.check_save(
        r#"
//- /src/optional.rs
pub struct Renamed;
"#,
        &[
            HostObservation::workspace_symbols("Appeared"),
            HostObservation::workspace_symbols("Renamed"),
        ],
        expect![[r#"
            changed files
            - generated_module_update_fixture src/optional.rs

            affected packages
            - generated_module_update_fixture

            changed targets
            - generated_module_update_fixture[lib]

            workspace symbols `Appeared`
            - <none>

            workspace symbols `Renamed`
            - struct Renamed @ generated_module_update_fixture[lib] src/optional.rs
        "#]],
    );

    fixture.check_save(
        r#"
//- /src/lib.rs
pub struct RootOnly;
"#,
        &[
            HostObservation::workspace_symbols("Renamed"),
            HostObservation::file_contexts("pruned generated source", "src/optional.rs"),
        ],
        expect![[r#"
            changed files
            - generated_module_update_fixture src/lib.rs

            affected packages
            - generated_module_update_fixture

            changed targets
            - generated_module_update_fixture[lib]

            workspace symbols `Renamed`
            - <none>

            file contexts `pruned generated source`
            - <none>
        "#]],
    );
}

#[test]
fn saved_generated_module_edits_rebuild_offloaded_reverse_dependents() {
    let mut fixture = ProjectFixture::build_with_package_residency_policy(
        r#"
//- /Cargo.toml
[package]
name = "generated_reverse_app"
version = "0.1.0"
edition = "2024"

[dependencies]
generated_reverse_dep = { path = "dep" }

//- /src/lib.rs
pub use generated_reverse_dep::*;

//- /dep/Cargo.toml
[package]
name = "generated_reverse_dep"
version = "0.1.0"
edition = "2024"

//- /dep/src/lib.rs
macro_rules! make_api {
    () => {
        pub mod api;
    };
}

make_api!();
pub use api::*;

//- /dep/src/api.rs
pub struct Before;
"#,
        PackageResidencyPolicy::AllOffloadable,
    );
    let app = fixture.package_slot_by_name("generated_reverse_app");
    let dep = fixture.package_slot_by_name("generated_reverse_dep");

    let summary = fixture.apply_saved_fixture(
        r#"
//- /dep/src/api.rs
pub struct After;
"#,
    );
    assert_eq!(
        summary
            .affected_packages
            .into_iter()
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([app, dep]),
        "editing a known generated source should rebuild its owner and reverse dependent",
    );

    let analysis = fixture
        .project()
        .snapshot()
        .full_analysis()
        .expect("offloaded reverse-dependent analysis should reload");
    assert!(
        analysis
            .workspace_symbols("Before")
            .expect("old generated symbol query should resolve")
            .is_empty(),
    );
    assert_eq!(
        analysis
            .workspace_symbols("After")
            .expect("new generated symbol query should resolve")
            .len(),
        1,
    );
}

#[test]
fn saved_macro_request_changes_replace_the_generated_source_set() {
    let mut fixture = ProjectFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "generated_request_change_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
macro_rules! choose_module {
    () => {
        pub mod first;
    };
}

choose_module!();

//- /src/first.rs
pub struct First;

//- /src/second.rs
pub struct Second;
"#,
    );
    let first = fixture
        .path("src/first.rs")
        .canonicalize()
        .expect("first generated source should canonicalize");
    let second = fixture
        .path("src/second.rs")
        .canonicalize()
        .expect("second generated source should canonicalize");

    fixture.apply_saved_fixture(
        r#"
//- /src/lib.rs
macro_rules! choose_module {
    () => {
        pub mod second;
    };
}

choose_module!();
"#,
    );

    let snapshot = fixture.project().snapshot();
    let parsed_paths = snapshot
        .parse_db()
        .package(0)
        .expect("request-change fixture package should exist")
        .parsed_files()
        .map(|file| file.path().to_path_buf())
        .collect::<BTreeSet<_>>();
    assert!(!parsed_paths.contains(&first));
    assert!(parsed_paths.contains(&second));

    let analysis = snapshot
        .full_analysis()
        .expect("request-change fixture analysis should build");
    assert!(
        analysis
            .workspace_symbols("First")
            .expect("old request symbol query should resolve")
            .is_empty(),
    );
    assert_eq!(
        analysis
            .workspace_symbols("Second")
            .expect("new request symbol query should resolve")
            .len(),
        2,
        "the replacement module and its struct should both be indexed",
    );
}

#[test]
fn saved_rebuild_renames_and_deletes_generated_module_sources() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "generated_module_rename_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
macro_rules! make_child {
    () => {
        pub mod child;
    };
}

make_child!();

//- /src/child.rs
pub struct ChildItem;
"#,
    );
    let flat_path = fixture.path("src/child.rs");
    let canonical_flat = flat_path
        .canonicalize()
        .expect("flat generated module path should canonicalize");
    let nested_path = fixture.path("src/child/mod.rs");
    let mut project = fixture.build_project();

    std::fs::create_dir_all(
        nested_path
            .parent()
            .expect("nested generated module path should have a parent"),
    )
    .expect("nested generated module directory should be created");
    std::fs::rename(&flat_path, &nested_path)
        .expect("generated module fixture should rename to nested form");
    project
        .apply_change(SavedFileChange::fs_path(&nested_path))
        .expect("the unknown nested path should trigger generated module rediscovery");

    let canonical_nested = nested_path
        .canonicalize()
        .expect("nested generated module path should canonicalize");
    let parsed_paths = project
        .snapshot()
        .parse_db()
        .package(0)
        .expect("rename fixture package should exist")
        .parsed_files()
        .map(|file| file.path().to_path_buf())
        .collect::<Vec<_>>();
    assert!(parsed_paths.contains(&canonical_nested));
    assert!(
        !parsed_paths.contains(&canonical_flat),
        "the removed flat path should leave the rebuilt package snapshot",
    );

    std::fs::remove_file(&nested_path).expect("nested generated fixture file should be removed");
    let lib_path = fixture.path("src/lib.rs");
    std::fs::write(
        &lib_path,
        r#"macro_rules! make_child {
    () => {
        pub mod child;
    };
}

make_child!();

// Force package rediscovery after child/mod.rs was deleted.
"#,
    )
    .expect("fixture root should be changed after generated module deletion");
    project
        .apply_change(SavedFileChange::fs_path(&lib_path))
        .expect("saving the declaring file should publish the missing generated module state");

    let snapshot = project.snapshot();
    assert_eq!(
        snapshot
            .parse_db()
            .package(0)
            .expect("deleted fixture package should exist")
            .parsed_files()
            .count(),
        1,
        "a missing generated module should not retain its historical package file",
    );
    assert!(
        snapshot
            .full_analysis()
            .expect("deleted generated module analysis should build")
            .workspace_symbols("ChildItem")
            .expect("deleted generated symbol query should resolve")
            .is_empty(),
    );
}

#[test]
fn warm_cache_restores_generated_module_files_without_rediscovery() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "generated_module_cache_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
macro_rules! make_cached {
    () => {
        pub mod cached;
    };
}

make_cached!();

//- /src/cached.rs
pub struct CachedGenerated;
"#,
    );
    let workspace = fixture.workspace_metadata();
    let cold = Project::builder(workspace.clone())
        .package_residency_policy(PackageResidencyPolicy::AllOffloadable)
        .build()
        .expect("cold generated module project should write its package artifact");
    assert_eq!(
        cold.snapshot()
            .parse_db()
            .package(0)
            .expect("fixture package should exist")
            .parsed_files()
            .count(),
        2,
        "the artifact parse snapshot should include the generated-discovered file",
    );
    drop(cold);

    let run = rg_profile::test_support::ProfileTest::start(
        crate::profile_descriptors(),
        "project.build.cache_probe,project.build.macro_source_files",
    );
    let warm = Project::builder(workspace)
        .package_residency_policy(PackageResidencyPolicy::AllOffloadable)
        .build()
        .expect("warm generated module project should restore from its package artifact");
    let profile = run.finish();

    profile.assert_counter_path_with_message(
        "project.build.cache_probe.results.hits",
        1,
        "the generated module artifact should remain a valid startup cache hit",
    );
    assert_eq!(
        profile
            .inner()
            .counter("project.build.macro_source_files.requests.seen")
            .unwrap_or(0),
        0,
        "a valid artifact should restore the final parse table without repeating discovery",
    );
    assert_eq!(
        warm.snapshot()
            .parse_db()
            .package(0)
            .expect("warm fixture package should exist")
            .parsed_files()
            .count(),
        2,
    );
    let symbols = warm
        .snapshot()
        .full_analysis()
        .expect("warm generated module analysis should load")
        .workspace_symbols("CachedGenerated")
        .expect("cached generated symbol query should resolve");
    assert_eq!(symbols.len(), 1);

    drop(warm);
    fixture.write_fixture_files(
        r#"
//- /src/cached.rs
pub struct RefreshedGenerated;
"#,
    );
    let run = rg_profile::test_support::ProfileTest::start(
        crate::profile_descriptors(),
        "project.build.cache_probe",
    );
    let rebuilt = Project::builder(fixture.workspace_metadata())
        .package_residency_policy(PackageResidencyPolicy::AllOffloadable)
        .build()
        .expect("editing a generated-discovered file should rebuild the stale artifact");
    let profile = run.finish();
    profile.assert_counter_path_with_message(
        "project.build.cache_probe.misses.parse_restore_error",
        1,
        "restoring the final parse snapshot should validate generated-discovered file revisions",
    );
    let analysis = rebuilt
        .snapshot()
        .full_analysis()
        .expect("rebuilt generated module analysis should load");
    assert!(
        analysis
            .workspace_symbols("CachedGenerated")
            .expect("old cached symbol query should resolve")
            .is_empty(),
    );
    assert_eq!(
        analysis
            .workspace_symbols("RefreshedGenerated")
            .expect("refreshed generated symbol query should resolve")
            .len(),
        1,
    );
    drop(analysis);
    drop(rebuilt);

    let second_warm = Project::builder(fixture.workspace_metadata())
        .package_residency_policy(PackageResidencyPolicy::AllOffloadable)
        .build()
        .expect("the rewritten generated module artifact should survive another warm restart");
    assert_eq!(
        second_warm
            .snapshot()
            .full_analysis()
            .expect("second warm generated module analysis should load")
            .workspace_symbols("RefreshedGenerated")
            .expect("second warm generated symbol query should resolve")
            .len(),
        1,
    );
}
