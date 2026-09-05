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
fn normalizes_missing_target_sources_by_workspace_membership() {
    let fixture = fixture_crate(
        r#"
//- /Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "dep" }

[[example]]
name = "app-demo"
path = "examples/app-demo.rs"

//- /src/lib.rs
pub struct App;

//- /examples/app-demo.rs
fn main() {}

//- /dep/Cargo.toml
[package]
name = "dep"
version = "0.1.0"
edition = "2024"

[[example]]
name = "dep-demo"
path = "examples/dep-demo.rs"

//- /dep/src/lib.rs
pub struct Dep;

//- /dep/examples/dep-demo.rs
fn main() {}
"#,
    );
    let metadata = fixture.metadata();
    for path in ["examples/app-demo.rs", "dep/examples/dep-demo.rs"] {
        fs::remove_file(fixture.path(path))
            .expect("fixture example file should be removable after metadata is loaded");
    }

    let workspace = WorkspaceMetadata::for_tests(metadata, WorkspaceLoweringConfig::default())
        .expect("missing target sources should normalize by package membership");
    let app = workspace
        .workspace_packages()
        .find(|package| package.name == "app")
        .expect("fixture app package should be present");
    let app_root = fixture
        .path("Cargo.toml")
        .canonicalize()
        .expect("fixture manifest should canonicalize")
        .parent()
        .expect("fixture manifest should have a parent")
        .to_path_buf();

    assert!(
        app.targets.iter().any(|target| {
            target.kind == TargetKind::Example
                && target.src_path == app_root.join("examples/app-demo.rs")
        }),
        "workspace example paths should remain rooted at the canonical package directory"
    );
    let dep = workspace
        .packages()
        .iter()
        .find(|package| package.name == "dep")
        .expect("dependency package should be present");

    assert!(
        dep.targets
            .iter()
            .any(|target| target.kind == TargetKind::Lib),
        "dependency library target should remain available"
    );
    assert!(
        !dep.targets
            .iter()
            .any(|target| target.kind == TargetKind::Example),
        "missing dependency example target should be omitted"
    );
}

#[test]
fn classifies_supported_and_rejects_unknown_cargo_package_sources() {
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
        ("local path", None, Ok(PackageSource::Path)),
        (
            "explicit path",
            Some("path+file:///tmp/source_fixture"),
            Ok(PackageSource::Path),
        ),
        (
            "registry",
            Some("registry+https://github.com/rust-lang/crates.io-index"),
            Ok(PackageSource::Registry),
        ),
        (
            "sparse registry",
            Some("sparse+https://index.crates.io/"),
            Ok(PackageSource::SparseRegistry),
        ),
        (
            "git",
            Some("git+https://example.com/repo.git"),
            Ok(PackageSource::Git),
        ),
        (
            "local registry",
            Some("local-registry+file:///tmp/registry"),
            Ok(PackageSource::LocalRegistry),
        ),
        (
            "directory",
            Some("directory+file:///tmp/vendor"),
            Ok(PackageSource::Directory),
        ),
        ("unsupported", Some("mystery+https://example.com"), Err(())),
    ];

    for (label, source, expected) in cases {
        let mut metadata = fixture.metadata();
        metadata.workspace_members.clear();
        metadata.packages[0].source = source.map(|source| cargo_metadata::Source {
            repr: source.to_string(),
        });

        let actual = WorkspaceMetadata::for_tests(metadata, WorkspaceLoweringConfig::default());
        match expected {
            Ok(expected_source) => assert_eq!(
                actual
                    .expect("supported package source should normalize")
                    .packages()[0]
                    .source,
                expected_source,
                "{label}"
            ),
            Err(()) => assert!(
                matches!(
                    actual,
                    Err(WorkspaceMetadataError::UnsupportedPackageSource { .. })
                ),
                "{label}: unexpected result {actual:?}"
            ),
        }
    }
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

//- /sysroot/library/proc_macro/src/lib.rs
pub struct TokenStream;
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

            package proc_macro [sysroot]
            manifest sysroot/library/proc_macro/Cargo.toml
            source sysroot
            edition 2024
            targets
            - proc_macro [lib] sysroot/library/proc_macro/src/lib.rs
            dependencies
            - alloc -> alloc
            - core -> core
            - std -> std

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
fn explicit_sysroot_root_requires_every_modeled_crate() {
    let incomplete = fixture_crate(
        r#"
//- /Cargo.toml
[package]
name = "app"
version = "0.1.0"

//- /sysroot/library/core/src/lib.rs
pub struct Core;
//- /sysroot/library/alloc/src/lib.rs
pub struct Alloc;
//- /sysroot/library/std/src/lib.rs
pub struct Std;
"#,
    );
    assert!(
        SysrootSources::from_library_root(incomplete.path("sysroot/library")).is_none(),
        "a source tree without proc_macro should be rejected",
    );

    let complete = fixture_crate(
        r#"
//- /Cargo.toml
[package]
name = "app"
version = "0.1.0"

//- /sysroot/library/core/src/lib.rs
pub struct Core;
//- /sysroot/library/alloc/src/lib.rs
pub struct Alloc;
//- /sysroot/library/std/src/lib.rs
pub struct Std;
//- /sysroot/library/proc_macro/src/lib.rs
pub struct TokenStream;
"#,
    );
    assert!(
        SysrootSources::from_library_root(complete.path("sysroot/library")).is_some(),
        "all modeled crate roots should form a usable sysroot",
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

//- /sysroot/library/proc_macro/src/lib.rs
pub struct TokenStream;
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

    for name in ["core", "alloc", "std", "proc_macro"] {
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
fn lowering_cfg_options_distinguishes_workspace_only_and_global_atoms() {
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
        WorkspaceLoweringConfig::default()
            .cfg_test(true)
            .custom_cfg_atoms(["tokio_unstable"]),
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

    for (label, package, expected_test, expected_custom) in [
        ("workspace package", app, true, true),
        ("dependency package", dep, false, true),
    ] {
        assert_eq!(
            package.cfg_options.contains_atom("test"),
            expected_test,
            "{label}: cfg(test)"
        );
        assert_eq!(
            package.cfg_options.contains_atom("tokio_unstable"),
            expected_custom,
            "{label}: custom cfg"
        );
    }
}

#[test]
fn cargo_feature_modes_reach_lowered_package_cfg_options() {
    let fixture = fixture_crate(
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
    );
    let cases = [
        (
            "custom features are additive",
            CargoMetadataConfig::default().custom_features(["extra"]),
            true,
            true,
        ),
        (
            "no default features keeps explicit features",
            CargoMetadataConfig::default()
                .no_default_features(true)
                .custom_features(["extra"]),
            false,
            true,
        ),
        (
            "all features combines with other options",
            CargoMetadataConfig::default()
                .all_features(true)
                .no_default_features(true)
                .custom_features(["extra"]),
            true,
            true,
        ),
    ];

    for (label, config, default_on, extra) in cases {
        let cfg_options = package_cfg_options_for_config(&fixture, config, "app");
        assert_eq!(
            cfg_options.contains_key_value("feature", "default_on"),
            default_on,
            "{label}: default_on"
        );
        assert_eq!(
            cfg_options.contains_key_value("feature", "extra"),
            extra,
            "{label}: extra"
        );
    }
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
