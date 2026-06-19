mod utils;

use std::{collections::BTreeSet, fs};

use expect_test::expect;
use test_fixture::{CrateFixture, fixture_crate};

use crate::{
    CargoMetadataConfig, CargoMetadataTarget, PackageSource, RustcTarget, SysrootSources,
    TargetKind, WorkspaceLoweringConfig, WorkspaceMetadata, WorkspaceMetadataError,
};

#[test]
fn dumps_normalized_workspace_metadata() {
    utils::check_workspace_metadata(
        r#"
//- /Cargo.toml
[workspace]
members = ["crates/app", "crates/dep"]
resolver = "3"

//- /crates/app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep_alias = { path = "../dep", package = "dep" }

[build-dependencies]
build_support = { path = "../../vendor/build_helper", package = "build_helper" }

[dev-dependencies]
dev_support = { path = "../../vendor/dev_helper", package = "dev_helper" }

[[example]]
name = "demo"
path = "examples/demo.rs"

[[test]]
name = "smoke"
path = "tests/smoke.rs"

[[bench]]
name = "api"
path = "benches/api.rs"

//- /crates/app/build.rs
fn main() {}

//- /crates/app/src/lib.rs
pub fn work() {}

//- /crates/app/src/main.rs
fn main() {}

//- /crates/app/examples/demo.rs
fn main() {}

//- /crates/app/tests/smoke.rs
#[test]
fn smoke() {}

//- /crates/app/benches/api.rs
fn main() {}

//- /crates/dep/Cargo.toml
[package]
name = "dep"
version = "0.1.0"
edition = "2024"

[dependencies]
helper_tools = { path = "../../vendor/helper", package = "helper" }

//- /crates/dep/src/lib.rs
pub fn dep() {}

//- /vendor/helper/Cargo.toml
[package]
name = "helper"
version = "0.1.0"
edition = "2024"

//- /vendor/helper/src/lib.rs
pub fn helper() {}

//- /vendor/build_helper/Cargo.toml
[package]
name = "build_helper"
version = "0.1.0"
edition = "2024"

//- /vendor/build_helper/src/lib.rs
pub fn build_helper() {}

//- /vendor/dev_helper/Cargo.toml
[package]
name = "dev_helper"
version = "0.1.0"
edition = "2024"

//- /vendor/dev_helper/src/lib.rs
pub fn dev_helper() {}
"#,
        expect![[r#"
            workspace .

            package app [member]
            manifest crates/app/Cargo.toml
            source workspace
            edition 2024
            targets
            - app [lib] crates/app/src/lib.rs
            - app [bin] crates/app/src/main.rs
            - demo [example] crates/app/examples/demo.rs
            - smoke [test] crates/app/tests/smoke.rs
            - api [bench] crates/app/benches/api.rs
            - build-script-build [custom-build] crates/app/build.rs
            dependencies
            - build_support -> build_helper [build]
            - dep_alias -> dep
            - dev_support -> dev_helper [dev]

            package build_helper [member]
            manifest vendor/build_helper/Cargo.toml
            source workspace
            edition 2024
            targets
            - build_helper [lib] vendor/build_helper/src/lib.rs
            dependencies
            - <none>

            package dep [member]
            manifest crates/dep/Cargo.toml
            source workspace
            edition 2024
            targets
            - dep [lib] crates/dep/src/lib.rs
            dependencies
            - helper_tools -> helper

            package dev_helper [member]
            manifest vendor/dev_helper/Cargo.toml
            source workspace
            edition 2024
            targets
            - dev_helper [lib] vendor/dev_helper/src/lib.rs
            dependencies
            - <none>

            package helper [member]
            manifest vendor/helper/Cargo.toml
            source workspace
            edition 2024
            targets
            - helper [lib] vendor/helper/src/lib.rs
            dependencies
            - <none>
        "#]],
    );
}

#[test]
fn retains_missing_workspace_target_path_during_metadata_normalization() {
    let fixture = fixture_crate(
        r#"
//- /Cargo.toml
[package]
name = "missing_target_fixture"
version = "0.1.0"
edition = "2024"

[[example]]
name = "demo"
path = "examples/demo.rs"

//- /src/lib.rs
pub struct Lib;

//- /examples/demo.rs
fn main() {}
"#,
    );
    let metadata = fixture.metadata();
    fs::remove_file(fixture.path("examples/demo.rs"))
        .expect("fixture example file should be removable after metadata is loaded");

    let workspace = WorkspaceMetadata::for_tests(metadata, WorkspaceLoweringConfig::default())
        .expect("missing optional target should normalize");
    let package = workspace
        .workspace_packages()
        .find(|package| package.name == "missing_target_fixture")
        .expect("fixture package should be present");
    let package_root = fixture
        .path("Cargo.toml")
        .canonicalize()
        .expect("fixture manifest should canonicalize")
        .parent()
        .expect("fixture manifest should have a parent")
        .to_path_buf();

    assert!(
        package.targets.iter().any(|target| {
            target.kind == TargetKind::Example
                && target.src_path == package_root.join("examples/demo.rs")
        }),
        "missing example target path should remain rooted at the canonical package directory"
    );
}

#[test]
fn skips_missing_non_workspace_target_sources() {
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

[[example]]
name = "demo"
path = "examples/demo.rs"

//- /dep/src/lib.rs
pub struct Dep;

//- /dep/examples/demo.rs
fn main() {}
"#,
    );
    let metadata = fixture.metadata();
    fs::remove_file(fixture.path("dep/examples/demo.rs"))
        .expect("fixture dependency example file should be removable after metadata is loaded");

    let workspace = WorkspaceMetadata::for_tests(metadata, WorkspaceLoweringConfig::default())
        .expect("missing dependency target should normalize");
    let package = workspace
        .packages()
        .iter()
        .find(|package| package.name == "dep")
        .expect("dependency package should be present");

    assert!(
        package
            .targets
            .iter()
            .any(|target| target.kind == TargetKind::Lib),
        "dependency library target should remain available"
    );
    assert!(
        !package
            .targets
            .iter()
            .any(|target| target.kind == TargetKind::Example),
        "missing dependency example target should be omitted"
    );
}

#[test]
fn classifies_known_cargo_package_sources() {
    let fixture = fixture_crate(
        r#"
//- /Cargo.toml
[package]
name = "source_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Lib;
"#,
    );
    let cases = [
        (None, PackageSource::Path),
        (Some("path+file:///tmp/source_fixture"), PackageSource::Path),
        (
            Some("registry+https://github.com/rust-lang/crates.io-index"),
            PackageSource::Registry,
        ),
        (
            Some("sparse+https://index.crates.io/"),
            PackageSource::SparseRegistry,
        ),
        (Some("git+https://example.com/repo.git"), PackageSource::Git),
        (
            Some("local-registry+file:///tmp/registry"),
            PackageSource::LocalRegistry,
        ),
        (
            Some("directory+file:///tmp/vendor"),
            PackageSource::Directory,
        ),
    ];

    for (source, expected_source) in cases {
        let mut metadata = fixture.metadata();
        metadata.workspace_members.clear();
        metadata.packages[0].source = source.map(|source| cargo_metadata::Source {
            repr: source.to_string(),
        });

        let workspace = WorkspaceMetadata::for_tests(metadata, WorkspaceLoweringConfig::default())
            .expect("known package source should normalize");
        assert_eq!(
            workspace.packages()[0].source,
            expected_source,
            "source {source:?} should be classified as {expected_source}"
        );
    }
}

#[test]
fn rejects_unknown_cargo_package_sources() {
    let fixture = fixture_crate(
        r#"
//- /Cargo.toml
[package]
name = "unknown_source_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Lib;
"#,
    );
    let mut metadata = fixture.metadata();
    metadata.workspace_members.clear();
    metadata.packages[0].source = Some(cargo_metadata::Source {
        repr: "mystery+https://example.com".to_string(),
    });

    let error = WorkspaceMetadata::for_tests(metadata, WorkspaceLoweringConfig::default())
        .expect_err("unknown source should be rejected");

    assert!(
        matches!(
            error,
            WorkspaceMetadataError::UnsupportedPackageSource { .. }
        ),
        "unexpected error: {error}"
    );
}

#[test]
fn injects_sysroot_packages_as_normalized_dependencies() {
    utils::check_workspace_metadata_with_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct App;

//- /sysroot/library/core/src/lib.rs
pub mod marker {
    pub struct Core;
}

//- /sysroot/library/alloc/src/lib.rs
pub mod marker {
    pub struct Alloc;
}

//- /sysroot/library/std/src/lib.rs
pub mod marker {
    pub struct Std;
}
"#,
        expect![[r#"
            workspace .

            package alloc [sysroot]
            manifest sysroot/library/alloc/Cargo.toml
            source sysroot
            edition 2024
            targets
            - alloc [lib] sysroot/library/alloc/src/lib.rs
            dependencies
            - core -> core

            package app [member]
            manifest Cargo.toml
            source workspace
            edition 2024
            targets
            - app [lib] src/lib.rs
            dependencies
            - alloc -> alloc [normal, build, dev]
            - core -> core [normal, build, dev]
            - std -> std [normal, build, dev]

            package core [sysroot]
            manifest sysroot/library/core/Cargo.toml
            source sysroot
            edition 2024
            targets
            - core [lib] sysroot/library/core/src/lib.rs
            dependencies
            - <none>

            package std [sysroot]
            manifest sysroot/library/std/Cargo.toml
            source sysroot
            edition 2024
            targets
            - std [lib] sysroot/library/std/src/lib.rs
            dependencies
            - alloc -> alloc
            - core -> core
        "#]],
    );
}

#[test]
fn sysroot_cfg_options_do_not_inherit_package_features() {
    let fixture = fixture_crate(
        r#"
//- /Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[features]
default = ["extra"]
extra = []

//- /src/lib.rs
pub struct App;

//- /sysroot/library/core/src/lib.rs
pub mod marker {
    pub struct Core;
}

//- /sysroot/library/alloc/src/lib.rs
pub mod marker {
    pub struct Alloc;
}

//- /sysroot/library/std/src/lib.rs
pub mod marker {
    pub struct Std;
}
"#,
    );
    let sysroot = SysrootSources::from_library_root(fixture.path("sysroot/library"))
        .expect("fixture sysroot should be complete");
    let workspace =
        WorkspaceMetadata::for_tests(fixture.metadata(), WorkspaceLoweringConfig::default())
            .expect("fixture workspace metadata should build")
            .with_sysroot_sources(Some(sysroot));
    let app = workspace
        .packages()
        .iter()
        .find(|package| package.name == "app")
        .expect("fixture app package should exist");

    assert!(
        app.cfg_options.contains_key_value("feature", "extra"),
        "fixture should exercise package-local feature cfgs",
    );

    for name in ["core", "alloc", "std"] {
        let package = workspace
            .packages()
            .iter()
            .find(|package| package.name == name)
            .unwrap_or_else(|| panic!("fixture sysroot package `{name}` should exist"));
        assert!(
            !package
                .cfg_options
                .key_values()
                .iter()
                .any(|value| value.key() == "feature"),
            "sysroot package `{name}` should use target cfg without package features",
        );
    }
}

#[test]
fn cfg_test_applies_to_workspace_packages_only() {
    let fixture = fixture_crate(
        r#"
//- /Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "vendor/dep" }

//- /src/lib.rs
pub struct App;

//- /vendor/dep/Cargo.toml
[package]
name = "dep"
version = "0.1.0"
edition = "2024"

//- /vendor/dep/src/lib.rs
pub struct Dep;
"#,
    );
    let workspace = WorkspaceMetadata::for_tests(
        fixture.metadata(),
        WorkspaceLoweringConfig::default().cfg_test(true),
    )
    .expect("fixture workspace metadata should build");
    let app = workspace
        .packages()
        .iter()
        .find(|package| package.name == "app")
        .expect("fixture app package should exist");
    let dep = workspace
        .packages()
        .iter()
        .find(|package| package.name == "dep")
        .expect("fixture dep package should exist");

    assert!(
        app.cfg_options.contains_atom("test"),
        "workspace packages should receive the requested cfg(test) atom",
    );
    assert!(
        !dep.cfg_options.contains_atom("test"),
        "dependency packages should not inherit workspace cfg(test) analysis mode",
    );
}

#[test]
fn custom_cfg_atoms_apply_to_all_cargo_packages() {
    let fixture = fixture_crate(
        r#"
//- /Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "vendor/dep" }

//- /src/lib.rs
pub struct App;

//- /vendor/dep/Cargo.toml
[package]
name = "dep"
version = "0.1.0"
edition = "2024"

//- /vendor/dep/src/lib.rs
pub struct Dep;
"#,
    );
    let workspace = WorkspaceMetadata::for_tests(
        fixture.metadata(),
        WorkspaceLoweringConfig::default().custom_cfg_atoms(["tokio_unstable"]),
    )
    .expect("fixture workspace metadata should build");
    let app = workspace
        .packages()
        .iter()
        .find(|package| package.name == "app")
        .expect("fixture app package should exist");
    let dep = workspace
        .packages()
        .iter()
        .find(|package| package.name == "dep")
        .expect("fixture dep package should exist");

    assert!(
        app.cfg_options.contains_atom("tokio_unstable"),
        "workspace packages should receive custom cfg atoms",
    );
    assert!(
        dep.cfg_options.contains_atom("tokio_unstable"),
        "dependency packages should receive custom cfg atoms too",
    );
}

#[test]
fn custom_cargo_features_are_additive_with_defaults() {
    let fixture = cargo_feature_fixture();
    let cfg_options = package_cfg_options_for_config(
        &fixture,
        CargoMetadataConfig::default().custom_features(["extra"]),
        "app",
    );

    assert!(
        cfg_options.contains_key_value("feature", "default_on"),
        "default features should remain active when custom features are added",
    );
    assert!(
        cfg_options.contains_key_value("feature", "extra"),
        "custom features should be active in lowered package cfg options",
    );
}

#[test]
fn no_default_cargo_features_keep_explicit_features() {
    let fixture = cargo_feature_fixture();
    let cfg_options = package_cfg_options_for_config(
        &fixture,
        CargoMetadataConfig::default()
            .no_default_features(true)
            .custom_features(["extra"]),
        "app",
    );

    assert!(
        !cfg_options.contains_key_value("feature", "default_on"),
        "default features should be disabled when no-default-features is set",
    );
    assert!(
        cfg_options.contains_key_value("feature", "extra"),
        "explicit custom features should still be active with no-default-features",
    );
}

#[test]
fn all_cargo_features_can_be_combined_with_other_feature_options() {
    let fixture = cargo_feature_fixture();
    let cfg_options = package_cfg_options_for_config(
        &fixture,
        CargoMetadataConfig::default()
            .all_features(true)
            .no_default_features(true)
            .custom_features(["extra"]),
        "app",
    );

    assert!(
        cfg_options.contains_key_value("feature", "default_on"),
        "all-features should keep enabling default feature members even when no-default-features is also requested",
    );
    assert!(
        cfg_options.contains_key_value("feature", "extra"),
        "all-features should enable non-default feature members",
    );
}

#[test]
fn workspace_member_discovery_ignores_feature_selection() {
    let fixture = fixture_crate(
        r#"
//- /Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct App;
"#,
    );
    let manifests = CargoMetadataConfig::default()
        .custom_features(["missing"])
        .load_workspace_member_manifest_paths(fixture.path("Cargo.toml"))
        .expect("member discovery should not depend on full analysis feature selection");

    assert_eq!(
        manifests,
        vec![
            fixture
                .path("Cargo.toml")
                .canonicalize()
                .expect("fixture manifest should canonicalize")
        ],
    );
}

#[test]
fn computes_transitive_reverse_dependency_closure() {
    let fixture = fixture_crate(
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
    let workspace =
        WorkspaceMetadata::for_tests(fixture.metadata(), WorkspaceLoweringConfig::default())
            .expect("fixture workspace metadata should build");
    let dep_id = workspace
        .packages()
        .iter()
        .find(|package| package.name == "dep")
        .expect("dep package should exist")
        .id
        .clone();
    let affected_names = workspace
        .reverse_dependency_closure(&[dep_id])
        .into_iter()
        .map(|slot| workspace.packages()[slot].name.clone())
        .collect::<BTreeSet<_>>();

    assert_eq!(
        affected_names,
        BTreeSet::from(["app".to_string(), "dep".to_string(), "mid".to_string()]),
        "reverse dependency closure should include transitive dependents only"
    );
}

#[test]
fn finds_packages_containing_source_paths() {
    let fixture = fixture_crate(
        r#"
//- /Cargo.toml
[workspace]
members = ["crates/app", "crates/dep"]
resolver = "3"

//- /crates/app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

//- /crates/app/src/lib.rs
pub struct App;

//- /crates/dep/Cargo.toml
[package]
name = "dep"
version = "0.1.0"
edition = "2024"

//- /crates/dep/src/lib.rs
pub struct Dep;
"#,
    );
    let workspace =
        WorkspaceMetadata::for_tests(fixture.metadata(), WorkspaceLoweringConfig::default())
            .expect("fixture workspace metadata should build");

    let app_api = fixture
        .path("crates/app/src")
        .canonicalize()
        .expect("fixture src dir should canonicalize")
        .join("api.rs");
    let generated_api = fixture
        .path("")
        .canonicalize()
        .expect("fixture root should canonicalize")
        .join("generated/api.rs");

    let package_names = workspace
        .package_slots_containing_path(&app_api)
        .into_iter()
        .map(|slot| workspace.packages()[slot].name.clone())
        .collect::<BTreeSet<_>>();

    assert_eq!(
        package_names,
        BTreeSet::from(["app".to_string()]),
        "a new source path should map to the package root that contains it"
    );
    assert!(
        workspace
            .package_slots_containing_path(&generated_api)
            .is_empty(),
        "paths outside every package root should not force a package rebuild"
    );
}

#[test]
fn parses_rustc_host_target_from_verbose_version_output() {
    let output = r#"
rustc 1.94.1
binary: rustc
host: aarch64-apple-darwin
release: 1.94.1
"#;

    let target = RustcTarget::parse_host_from_verbose_output(output)
        .expect("verbose rustc output should contain a host triple");

    assert_eq!(target.as_str(), "aarch64-apple-darwin");
}

#[test]
fn normalizes_explicit_cargo_metadata_target() {
    let config = CargoMetadataConfig::default().target_triple("  x86_64-unknown-linux-gnu  ");

    let CargoMetadataTarget::Triple(target) = config.target() else {
        panic!("non-empty explicit target should configure a target triple");
    };
    assert_eq!(target.as_str(), "x86_64-unknown-linux-gnu");
}

fn cargo_feature_fixture() -> CrateFixture {
    fixture_crate(
        r#"
//- /Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[features]
default = ["default_on"]
default_on = []
extra = []

//- /src/lib.rs
pub struct App;
"#,
    )
}

fn package_cfg_options_for_config(
    fixture: &CrateFixture,
    config: CargoMetadataConfig,
    package_name: &str,
) -> rg_cfg_eval::CfgOptions {
    let loaded = config
        .load_metadata_with_target_cfg(fixture.path("Cargo.toml"))
        .expect("fixture cargo metadata should load");
    let workspace = WorkspaceMetadata::lower(
        loaded.metadata,
        loaded.target_cfg,
        WorkspaceLoweringConfig::default(),
    )
    .expect("fixture workspace metadata should build");

    workspace
        .packages()
        .iter()
        .find(|package| package.name == package_name)
        .unwrap_or_else(|| panic!("fixture package `{package_name}` should exist"))
        .cfg_options
        .clone()
}
