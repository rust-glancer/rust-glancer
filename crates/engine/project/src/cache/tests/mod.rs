mod instance;
mod utils;

use expect_test::expect;
use rg_workspace::{PackageSlot, WorkspaceLoweringConfig, WorkspaceMetadata};
use test_fixture::fixture_crate;

use crate::cache::WorkspaceCachePlan;

#[test]
fn plans_cache_artifacts_from_analyzed_targets() {
    utils::check_cache_plan(
        r#"
//- /Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep_alias = { path = "dep", package = "dep-pkg" }

[build-dependencies]
build_support = { path = "build-helper", package = "build-helper" }

[dev-dependencies]
dev_support = { path = "dev-helper", package = "dev-helper" }

[[example]]
name = "demo"
path = "examples/demo.rs"

[[test]]
name = "smoke"
path = "tests/smoke.rs"

//- /build.rs
fn main() {}

//- /src/lib.rs
pub struct App;

//- /src/main.rs
fn main() {}

//- /examples/demo.rs
fn main() {}

//- /tests/smoke.rs
#[test]
fn smoke() {}

//- /dep/Cargo.toml
[package]
name = "dep-pkg"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "dep-tool"
path = "src/bin/dep_tool.rs"

//- /dep/src/lib.rs
pub struct Dep;

//- /dep/src/bin/dep_tool.rs
fn main() {}

//- /build-helper/Cargo.toml
[package]
name = "build-helper"
version = "0.1.0"
edition = "2021"

//- /build-helper/src/lib.rs
pub struct BuildHelper;

//- /dev-helper/Cargo.toml
[package]
name = "dev-helper"
version = "0.1.0"
edition = "2018"

//- /dev-helper/src/lib.rs
pub struct DevHelper;
"#,
        expect![[r#"
            workspace cache plan

            package #0 app
            schema 3
            id path+file://./#app@0.1.0
            source workspace
            edition 2024
            manifest Cargo.toml
            targets
            - app [lib] src/lib.rs
            - app [bin] src/main.rs
            - demo [example] examples/demo.rs
            - smoke [test] tests/smoke.rs
            - build-script-build [custom-build] build.rs
            dependencies
            - build_support -> build-helper (#1) [build]
            - dep_alias -> dep-pkg (#2) [normal]
            - dev_support -> dev-helper (#3) [dev]

            package #1 build-helper
            schema 3
            id path+file://./build-helper#0.1.0
            source path
            edition 2021
            manifest build-helper/Cargo.toml
            targets
            - build_helper [lib] build-helper/src/lib.rs
            dependencies
            - <none>

            package #2 dep-pkg
            schema 3
            id path+file://./dep#dep-pkg@0.1.0
            source path
            edition 2021
            manifest dep/Cargo.toml
            targets
            - dep_pkg [lib] dep/src/lib.rs
            dependencies
            - <none>

            package #3 dev-helper
            schema 3
            id path+file://./dev-helper#0.1.0
            source path
            edition 2018
            manifest dev-helper/Cargo.toml
            targets
            - dev_helper [lib] dev-helper/src/lib.rs
            dependencies
            - <none>
        "#]],
    );
}

#[test]
fn cfg_test_changes_workspace_package_cache_identity() {
    let fixture = fixture_crate(
        r#"
//- /Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "dep" }

//- /src/lib.rs
pub struct App;

//- /dep/Cargo.toml
[package]
name = "dep"
version = "0.1.0"
edition = "2024"

//- /dep/src/lib.rs
pub struct Dep;
"#,
    );
    let normal_workspace =
        WorkspaceMetadata::for_tests(fixture.metadata(), WorkspaceLoweringConfig::default())
            .expect("fixture workspace metadata should build");
    let test_workspace = WorkspaceMetadata::for_tests(
        fixture.metadata(),
        WorkspaceLoweringConfig::default().cfg_test(true),
    )
    .expect("fixture workspace metadata should build");
    let normal_plan = WorkspaceCachePlan::build(&normal_workspace);
    let test_plan = WorkspaceCachePlan::build(&test_workspace);

    let app = package_slot(&normal_workspace, "app");
    let dep = package_slot(&normal_workspace, "dep");
    assert_ne!(
        normal_plan
            .package(app)
            .expect("normal app package should exist")
            .fingerprint(normal_workspace.workspace_root()),
        test_plan
            .package(app)
            .expect("test app package should exist")
            .fingerprint(test_workspace.workspace_root()),
        "cfg(test) should select a distinct workspace package cache identity",
    );
    assert_eq!(
        normal_plan
            .package(dep)
            .expect("normal dep package should exist")
            .fingerprint(normal_workspace.workspace_root()),
        test_plan
            .package(dep)
            .expect("test dep package should exist")
            .fingerprint(test_workspace.workspace_root()),
        "dependency package cache identity should not change for workspace cfg(test)",
    );
}

fn package_slot(workspace: &WorkspaceMetadata, package_name: &str) -> PackageSlot {
    workspace
        .packages()
        .iter()
        .position(|package| package.name == package_name)
        .map(PackageSlot)
        .unwrap_or_else(|| panic!("fixture package `{package_name}` should exist"))
}

#[test]
fn roundtrips_package_cache_header_codec() {
    utils::check_cache_header_codec(expect![[r#"
        encoded header bytes 315
        0300000007000000000000002000000000000000706174682b66696c653a2f2f
        2f776f726b73706163652361707040302e312e30030000000000000061707000
        0000000300000015000000000000002f776f726b73706163652f436172676f2e
        746f6d6c00000000000000000000000000000000020000000000000003000000
        000000006170700000000015000000000000002f776f726b73706163652f7372
        632f6c69622e727307000000000000006170702d636c69010000001600000000
        0000002f776f726b73706163652f7372632f6d61696e2e727301000000000000
        002400000000000000706174682b66696c653a2f2f2f776f726b73706163652f
        6465702364657040302e312e3003000000000000006465700100000707070707
        070707070707070707070707070707070707070707070707070707

        decoded header
        schema 3
        source fingerprint 0707070707070707070707070707070707070707070707070707070707070707
        package #7 app
        id path+file:///workspace#app@0.1.0
        source workspace
        edition 2024
        manifest /workspace/Cargo.toml
        targets
        - app [lib] /workspace/src/lib.rs
        - app-cli [bin] /workspace/src/main.rs
        dependencies
        - dep -> path+file:///workspace/dep#dep@0.1.0 [normal]
    "#]]);
}

#[test]
fn roundtrips_minimal_package_cache_artifact_codec() {
    utils::check_minimal_cache_artifact_codec(expect![[r#"
        encoded artifact has bytes true
        0300000007000000000000002200000000000000706174682b66696c653a2f2f
        2f776f726b737061636523656d70747940302e312e3000000000000000000000
        00000300000015000000000000002f776f726b73706163652f436172676f2e74
        6f6d6c0000000000000000000000000000000000000000000000000000000000
        0000000707070707070707070707070707070707070707070707070707070707
        0707070000000000000000000000000000000000000000000000000000000000
        0000000000000000000000000000000000000000000000000000000000000000
        000000

        decoded artifact
        schema 3
        source fingerprint 0707070707070707070707070707070707070707070707070707070707070707
        package #7 
        header targets 0
        parse files 0
        parse target roots 0
        def-map package  targets 0
        semantic IR targets 0
        body IR built targets 0
    "#]]);
}

#[test]
fn roundtrips_fixture_package_cache_artifact_codec() {
    utils::check_fixture_cache_artifact_codec(
        r#"
//- /Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct App;
"#,
        expect![[r#"
            encoded artifact has bytes true
            decoded artifact
            schema 3
            source fingerprint 5cb07c1684eeeb2c51a750cf465c7cd8f62d74e2e1dbdace0df9b4481058d206
            package #0 app
            header targets 1
            parse files 1
            parse target roots 1
            def-map package app targets 1
            semantic IR targets 1
            body IR built targets 1
        "#]],
    );
}

#[test]
fn stores_package_cache_artifacts_on_disk() {
    utils::check_cache_store_artifact_io(
        r#"
//- /Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct App;
"#,
        expect![[r#"
            cache store artifact I/O
            missing before write true
            written artifact has bytes true
            loaded package #0 app
            corrupt read has typed decode error true
            missing after invalidation true
        "#]],
    );
}

#[test]
fn removes_stale_package_cache_generations() {
    utils::check_cache_store_generation_cleanup(
        r#"
//- /Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct App;
"#,
        expect![[r#"
            cache store generation cleanup
            current artifact before cleanup true
            stale generation after cleanup false
            current artifact after cleanup true
        "#]],
    );
}

#[test]
fn residency_policy_controls_package_artifact_writes() {
    utils::check_residency_policy_controls_artifact_writes(
        r#"
//- /Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "dep" }

//- /src/lib.rs
pub struct AppBefore;
pub fn use_dep(_: dep::Dep) {}

//- /dep/Cargo.toml
[package]
name = "dep"
version = "0.1.0"
edition = "2024"

//- /dep/src/lib.rs
pub struct Dep;
"#,
        expect![[r#"
            artifact writes by residency policy
            all-resident
            - app artifact false
            - dep artifact false

            workspace-resident
            - app artifact false
            - dep artifact true

            all-offloadable
            - app artifact true
            - dep artifact true
        "#]],
    );
}

#[test]
fn lazy_loads_offloaded_packages_for_queries() {
    utils::check_offloaded_dependency_query(
        r#"
//- /Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "dep" }

//- /src/lib.rs
pub struct App;

//- /dep/Cargo.toml
[package]
name = "dep"
version = "0.1.0"
edition = "2024"

[[bin]]
name = "dep-tool"
path = "src/bin/dep_tool.rs"

//- /dep/src/lib.rs
pub struct DepType;

//- /dep/src/bin/dep_tool.rs
fn main() {}
"#,
        expect![[r#"
            offloaded dependency query
            dep resident false
            symbols
            - struct DepType @ dep[lib]
        "#]],
    );
}

#[test]
fn startup_indexing_uses_matching_offloaded_package_artifacts() {
    utils::check_startup_cache_uses_matching_artifact(expect![[r#"
        startup artifact-backed indexing
        dep resident false
        old symbols 1
        new symbols 0
    "#]]);
}

#[test]
fn artifact_snapshot_source_fingerprint_matches_discovered_package_sources() {
    utils::check_artifact_snapshot_source_fingerprint_matches_package_sources(expect![[r#"
        artifact snapshot source fingerprint
        package dep
        parse files 2
        matches true
    "#]]);
}

#[test]
fn startup_cache_profile_reports_probe_hits() {
    utils::check_startup_cache_probe_profile(expect![[r#"
        startup cache probe profile
        packages 2
        resident 1
        offloadable 1
        hits 1
        misses 0
    "#]]);
}

#[test]
fn startup_indexing_rejects_artifacts_when_body_ir_policy_needs_more_bodies() {
    utils::check_startup_cache_rejects_body_ir_policy_mismatch(expect![[r#"
        startup body IR policy mismatch
        hits 0
        misses 1
        body policy mismatches 1
        body IR target statuses
        - target 0 built
    "#]]);
}

#[test]
fn startup_indexing_rejects_artifacts_when_out_of_line_modules_change() {
    utils::check_startup_cache_rejects_stale_out_of_line_module(expect![[r#"
        startup stale out-of-line module
        dep resident false
        old symbols 0
        new symbols 1
    "#]]);
}
