use std::collections::BTreeSet;

use expect_test::expect;

use super::utils::{HostFixture, HostObservation};
use crate::{
    SavedFileChange,
    testonly::{ProjectFixture, ProjectSourceFixture},
};

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
            HostObservation::file_context("generated module source", "src/custom_imp.rs"),
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

            file context `generated module source`
            - generated_module_fixture src/custom_imp.rs -> generated_module_fixture[lib]

            type names at `generated reexport`
            - Deep
        "#]],
    );
}

#[test]
fn generated_source_discovery_resolves_imported_macros_between_file_loads() {
    let fixture = HostFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "generated_import_chain"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod factory {
    macro_rules! make_leaf {
        () => { pub mod leaf; };
    }

    pub(crate) use make_leaf;
}

macro_rules! make_branch {
    () => {
        pub mod branch;

        macro_rules! make_sibling {
            () => { pub struct Sibling; };
        }
        make_sibling!();
    };
}

make_branch!();
pub use branch::Leaf;
pub fn consume(_: Le$type$af) {}

//- /src/branch.rs
use crate::factory::make_leaf;
make_leaf!();
pub use leaf::Leaf;

//- /src/branch/leaf.rs
pub struct Leaf;
"#,
    );

    // The first loaded file needs import resolution before it can discover the second file.
    // The sibling expansion must also survive the source request from the same macro output.
    fixture.check(
        &[
            HostObservation::workspace_symbols("Leaf"),
            HostObservation::workspace_symbols("Sibling"),
            HostObservation::file_context("second generated file", "src/branch/leaf.rs"),
            HostObservation::type_names_at("generated reexport", "generated_import_chain", "type"),
        ],
        expect![[r#"
            workspace symbols `Leaf`
            - module leaf @ generated_import_chain[lib] src/branch.rs
            - struct Leaf @ generated_import_chain[lib] src/branch/leaf.rs

            workspace symbols `Sibling`
            - struct Sibling @ generated_import_chain[lib] src/lib.rs

            file context `second generated file`
            - generated_import_chain src/branch/leaf.rs -> generated_import_chain[lib]

            type names at `generated reexport`
            - Leaf
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
            HostObservation::file_context("nested generated source", "src/models/child.rs"),
        ],
        expect![[r#"
            workspace symbols `Nested`
            - struct Nested @ generated_inline_context_fixture[lib] src/models/child.rs

            file context `nested generated source`
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
            HostObservation::file_context("nested call-site source", "src/outer/child/mod.rs"),
        ],
        expect![[r#"
            workspace symbols `CallSite`
            - struct CallSite @ generated_dependency_context_fixture[lib] src/outer/child/mod.rs

            workspace symbols `DefinitionSite`
            - <none>

            file context `nested call-site source`
            - generated_dependency_context_fixture src/outer/child/mod.rs -> generated_dependency_context_fixture[lib]
        "#]],
    );
}

#[test]
fn included_files_inherit_each_call_sites_module_context() {
    let fixture = HostFixture::build_with_fake_sysroot(
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
"#,
    );

    fixture.check(
        &[
            HostObservation::workspace_symbols("LeftOrdinary"),
            HostObservation::workspace_symbols("RightOrdinary"),
            HostObservation::workspace_symbols("LeftLate"),
            HostObservation::workspace_symbols("RightLate"),
            HostObservation::file_context("left generated source", "src/left/late.rs"),
            HostObservation::file_context("right generated source", "src/right/late.rs"),
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

            file context `left generated source`
            - generated_include_context_fixture src/left/late.rs -> generated_include_context_fixture[lib]

            file context `right generated source`
            - generated_include_context_fixture src/right/late.rs -> generated_include_context_fixture[lib]
        "#]],
    );
}

#[test]
fn cyclic_associated_include_does_not_block_project_build() {
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
fn generated_module_aliases_resolve_shared_sources_in_lib_and_bin() {
    let fixture = HostFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "generated_multi_target_fixture"
version = "0.1.0"
edition = "2024"

[[bin]]
name = "generated-bin"
path = "src/main.rs"

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
pub fn consume(_: first::Sha$library_first$red, _: second::Sha$library_second$red) {}

//- /src/main.rs
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
fn consume(_: first::Sha$binary_first$red, _: second::Sha$binary_second$red) {}
fn main() {}

//- /src/shared.rs
pub struct Shared;
"#,
    );

    fixture.check(
        &[
            HostObservation::file_context("shared source", "src/shared.rs"),
            HostObservation::type_names_at(
                "library first alias",
                "generated_multi_target_fixture",
                "library_first",
            ),
            HostObservation::type_names_at(
                "library second alias",
                "generated_multi_target_fixture",
                "library_second",
            ),
            HostObservation::type_names_at(
                "binary first alias",
                "generated_multi_target_fixture",
                "binary_first",
            ),
            HostObservation::type_names_at(
                "binary second alias",
                "generated_multi_target_fixture",
                "binary_second",
            ),
            HostObservation::workspace_symbols("missing"),
        ],
        expect![[r#"
            file context `shared source`
            - generated_multi_target_fixture src/shared.rs -> generated_multi_target_fixture[lib]

            type names at `library first alias`
            - Shared

            type names at `library second alias`
            - Shared

            type names at `binary first alias`
            - Shared

            type names at `binary second alias`
            - Shared

            workspace symbols `missing`
            - <none>
        "#]],
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
            HostObservation::file_context("created generated source", "src/optional.rs"),
        ],
        expect![[r#"
            changed files
            - generated_module_update_fixture src/optional.rs

            changed targets
            - generated_module_update_fixture[lib]

            workspace symbols `Appeared`
            - struct Appeared @ generated_module_update_fixture[lib] src/optional.rs

            file context `created generated source`
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
            HostObservation::file_context("pruned generated source", "src/optional.rs"),
        ],
        expect![[r#"
            changed files
            - generated_module_update_fixture src/lib.rs
            - generated_module_update_fixture src/optional.rs

            changed targets
            - generated_module_update_fixture[lib]

            workspace symbols `Renamed`
            - <none>

            file context `pruned generated source`
            - <none>
        "#]],
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

    fixture.project().inspect(|snapshot| {
        let parsed_paths = snapshot
            .parse_db()
            .package(0)
            .expect("request-change fixture package should exist")
            .parsed_files()
            .map(|file| file.path().to_path_buf())
            .collect::<BTreeSet<_>>();
        assert!(!parsed_paths.contains(&first));
        assert!(parsed_paths.contains(&second));

        let analysis = snapshot.analysis();
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
    });
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
        .state
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

    project.inspect(|snapshot| {
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
                .analysis()
                .workspace_symbols("ChildItem")
                .expect("deleted generated symbol query should resolve")
                .is_empty(),
        );
    });
}
