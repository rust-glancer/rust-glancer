use expect_test::expect;
use rg_cfg_eval::CfgOptions;
use rg_workspace::{TargetKind, WorkspaceLoweringConfig, WorkspaceMetadata};
use test_fixture::fixture_crate;

use super::utils;
use crate::{DefMapLoader, DefMapSource, testonly::DefMapFixture};

#[test]
fn retains_compilation_target_width_without_loading_offloaded_scopes() {
    let source = fixture_crate(
        r#"
//- /Cargo.toml
[package]
name = "target_width"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn use_it() {}
"#,
    );
    let workspace = WorkspaceMetadata::lower(
        source.metadata(),
        CfgOptions::from_rustc_cfg_output("target_pointer_width=\"32\""),
        WorkspaceLoweringConfig::default(),
    )
    .expect("fixture target metadata should lower");
    let fixture = DefMapFixture::build_from_crate(source, workspace);
    let crate_ref = fixture.crate_ref("target_width", TargetKind::Lib);
    let mut db = fixture.def_map_db().clone();

    for offload in [false, true] {
        if offload {
            db.offload_package(crate_ref.package)
                .expect("fixture package should exist");
        }
        let read = db.read_txn(DefMapLoader::resident_only(
            "target width needs no crate payload",
        ));
        assert_eq!(
            read.target_pointer_width(crate_ref)
                .expect("target width should be available"),
            Some(32),
            "compilation target width survives offloading: {offload}",
        );
    }
}

#[test]
fn semantic_crates_retain_their_originating_cargo_targets() {
    let fixture = DefMapFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "multi_target"
version = "0.1.0"
edition = "2024"

[[bin]]
name = "tool"
path = "src/main.rs"

//- /src/lib.rs
pub fn library() {}

//- /src/main.rs
fn main() {}
"#,
    );
    let package_slot = fixture.package_slot_by_name("multi_target");
    let parsed = fixture
        .package(package_slot)
        .expect("fixture package should be parsed");
    let def_maps = fixture
        .def_map_db()
        .resident_package(package_slot)
        .expect("fixture def-map package should be resident");

    assert_eq!(def_maps.crates().len(), parsed.targets().len());
    for cargo_target in parsed.targets() {
        let crate_id = def_maps
            .crate_for_cargo_target(cargo_target.id)
            .expect("every parsed Cargo target should produce one semantic crate");
        let crate_data = def_maps
            .crate_data(crate_id)
            .expect("allocated semantic crate should exist");
        assert_eq!(crate_data.cargo_target(), cargo_target.id,);
        assert_eq!(crate_data.target_kind(), &cargo_target.kind);
    }

    let lib = fixture.crate_ref("multi_target", TargetKind::Lib);
    let bin = fixture.crate_ref("multi_target", TargetKind::Bin);
    assert_ne!(lib.crate_id, bin.crate_id);
}

#[test]
fn target_kind_controls_visible_dependency_roots() {
    utils::check_project_def_map(
        r#"
//- /Cargo.toml
[workspace]
members = ["app", "build_helper", "dev_helper", "normal_dep"]
resolver = "3"

//- /app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
normal_dep = { path = "../normal_dep" }

[build-dependencies]
build_helper = { path = "../build_helper" }

[dev-dependencies]
dev_helper = { path = "../dev_helper" }

[[test]]
name = "smoke"
path = "tests/smoke.rs"

//- /app/src/lib.rs
use normal_dep::normal_work;
use build_helper::build_work;

pub fn lib() {}

//- /app/build.rs
use build_helper::build_work;
use normal_dep::normal_work;
use app::lib;

fn main() {}

//- /app/tests/smoke.rs
use app::lib;
use normal_dep::normal_work;
use dev_helper::dev_work;
use build_helper::build_work;

//- /build_helper/Cargo.toml
[package]
name = "build_helper"
version = "0.1.0"
edition = "2024"

//- /build_helper/src/lib.rs
pub fn build_work() {}

//- /dev_helper/Cargo.toml
[package]
name = "dev_helper"
version = "0.1.0"
edition = "2024"

//- /dev_helper/src/lib.rs
pub fn dev_work() {}

//- /normal_dep/Cargo.toml
[package]
name = "normal_dep"
version = "0.1.0"
edition = "2024"

//- /normal_dep/src/lib.rs
pub fn normal_work() {}
"#,
        expect![[r#"
            package app

            app [lib]
            crate
            - lib : value [pub fn app[lib]::crate::lib]
            - normal_work : value [fn normal_dep[lib]::crate::normal_work]
            unresolved imports
            - use build_helper::build_work

            app [test]
            crate
            - dev_work : value [fn dev_helper[lib]::crate::dev_work]
            - lib : value [fn app[lib]::crate::lib]
            - normal_work : value [fn normal_dep[lib]::crate::normal_work]
            unresolved imports
            - use build_helper::build_work

            app [custom-build]
            crate
            - build_work : value [fn build_helper[lib]::crate::build_work]
            - main : value [fn app[custom-build]::crate::main]
            unresolved imports
            - use normal_dep::normal_work
            - use app::lib

            package build_helper

            build_helper [lib]
            crate
            - build_work : value [pub fn build_helper[lib]::crate::build_work]

            package dev_helper

            dev_helper [lib]
            crate
            - dev_work : value [pub fn dev_helper[lib]::crate::dev_work]

            package normal_dep

            normal_dep [lib]
            crate
            - normal_work : value [pub fn normal_dep[lib]::crate::normal_work]
        "#]],
    );
}

#[test]
fn proc_macro_sysroot_root_is_visible_only_to_proc_macro_targets() {
    utils::check_project_path_resolution_with_fake_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "mixed_targets"
version = "0.1.0"
edition = "2024"

[lib]
proc-macro = true

[[bin]]
name = "ordinary"
path = "src/main.rs"

//- /src/lib.rs
use proc_macro::TokenStream;

fn consume(_stream: TokenStream) {}

//- /src/main.rs
use proc_macro::TokenStream;

fn main() {}
"#,
        &[
            utils::PathResolutionQuery::proc_macro(
                "mixed_targets",
                "crate",
                "proc_macro::TokenStream",
            ),
            utils::PathResolutionQuery::proc_macro("mixed_targets", "crate", "TokenStream"),
            utils::PathResolutionQuery::bin("mixed_targets", "crate", "proc_macro::TokenStream"),
            utils::PathResolutionQuery::bin("mixed_targets", "crate", "TokenStream"),
        ],
        expect![[r#"
            mixed_targets [proc-macro] crate resolves proc_macro::TokenStream -> struct proc_macro[lib]::crate::TokenStream
            mixed_targets [proc-macro] crate resolves TokenStream -> struct proc_macro[lib]::crate::TokenStream
            mixed_targets [bin] crate resolves proc_macro::TokenStream -> <none> (unresolved at segment #0)
            mixed_targets [bin] crate resolves TokenStream -> <none> (unresolved at segment #0)
        "#]],
    );
}
