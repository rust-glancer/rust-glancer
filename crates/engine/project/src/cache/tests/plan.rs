use std::fmt::Write as _;

use expect_test::expect;
use rg_workspace::{PackageSlot, WorkspaceLoweringConfig, WorkspaceMetadata};
use test_fixture::fixture_crate;

use crate::cache::{
    CURRENT_PACKAGE_CACHE_SCHEMA_VERSION, CachedDependency, CachedPath, CachedTarget,
    WorkspaceCachePlan,
};
use crate::{PackageResidencyPolicy, testonly::ProjectSourceFixture};

#[test]
fn cache_identity_uses_structure_instead_of_checkout_or_cargo_id_text() {
    let fixture = r#"
//- /Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct App;
"#;
    let left = ProjectSourceFixture::build(fixture);
    let right = ProjectSourceFixture::build(fixture);
    let left_workspace = left.workspace_metadata();
    let right_workspace = right.workspace_metadata();

    assert_ne!(
        left_workspace.packages()[0].id,
        right_workspace.packages()[0].id,
        "Cargo should report distinct opaque IDs for distinct fixture checkouts",
    );

    let left_plan = WorkspaceCachePlan::build(&left_workspace);
    let right_plan = WorkspaceCachePlan::build(&right_workspace);
    assert_eq!(left_plan, right_plan);
    assert_eq!(
        left_plan.generation_fingerprint(PackageResidencyPolicy::default()),
        right_plan.generation_fingerprint(PackageResidencyPolicy::default()),
    );
}

#[test]
fn plans_cache_artifacts_from_analyzed_targets() {
    let fixture = ProjectSourceFixture::build(
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
    );
    let workspace = fixture.workspace_metadata();
    let cache_plan = WorkspaceCachePlan::build(&workspace);
    let actual = render_cache_plan(&workspace, &cache_plan);

    expect![[r#"
        workspace cache plan

        package #0 app
        schema 7
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
        schema 7
        source path
        edition 2021
        manifest build-helper/Cargo.toml
        targets
        - build_helper [lib] build-helper/src/lib.rs
        dependencies
        - <none>

        package #2 dep-pkg
        schema 7
        source path
        edition 2021
        manifest dep/Cargo.toml
        targets
        - dep_pkg [lib] dep/src/lib.rs
        dependencies
        - <none>

        package #3 dev-helper
        schema 7
        source path
        edition 2018
        manifest dev-helper/Cargo.toml
        targets
        - dev_helper [lib] dev-helper/src/lib.rs
        dependencies
        - <none>
    "#]]
    .assert_eq(&format!("{}\n", actual.trim_end()));
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

    let package_slot = |package_name: &str| {
        normal_workspace
            .packages()
            .iter()
            .position(|package| package.name == package_name)
            .map(PackageSlot)
            .unwrap_or_else(|| panic!("fixture package `{package_name}` should exist"))
    };
    let app = package_slot("app");
    let dep = package_slot("dep");
    assert_ne!(
        normal_plan
            .package(app)
            .expect("normal app package should exist")
            .fingerprint(),
        test_plan
            .package(app)
            .expect("test app package should exist")
            .fingerprint(),
        "cfg(test) should select a distinct workspace package cache identity",
    );
    assert_eq!(
        normal_plan
            .package(dep)
            .expect("normal dep package should exist")
            .fingerprint(),
        test_plan
            .package(dep)
            .expect("test dep package should exist")
            .fingerprint(),
        "dependency package cache identity should not change for workspace cfg(test)",
    );
}

fn render_cache_plan(workspace: &WorkspaceMetadata, cache_plan: &WorkspaceCachePlan) -> String {
    // Use checkout-relative paths and uniform separators so temporary roots stay out of the dump.
    let render_path = |path: &CachedPath| {
        let root = workspace.workspace_root();
        let path = path
            .to_path_buf(root)
            .expect("production cache path should decode on its producing host");
        let relative_path = path.strip_prefix(root).unwrap_or(&path);
        if relative_path.as_os_str().is_empty() {
            ".".to_string()
        } else {
            relative_path
                .components()
                .map(|component| component.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/")
        }
    };

    let mut dump = String::new();
    writeln!(&mut dump, "workspace cache plan").expect("string writes should not fail");

    for package in cache_plan.packages() {
        // Start with the package identity before listing what its artifact contains.
        writeln!(&mut dump).expect("string writes should not fail");
        writeln!(&mut dump, "package #{} {}", package.package.0, package.name,)
            .expect("string writes should not fail");
        writeln!(
            &mut dump,
            "schema {}",
            CURRENT_PACKAGE_CACHE_SCHEMA_VERSION.0,
        )
        .expect("string writes should not fail");
        writeln!(&mut dump, "source {}", package.source).expect("string writes should not fail");
        writeln!(&mut dump, "edition {}", package.edition).expect("string writes should not fail");
        writeln!(
            &mut dump,
            "manifest {}",
            render_path(&package.manifest_path),
        )
        .expect("string writes should not fail");

        // Show the analyzed targets in a stable order, including their source paths.
        writeln!(&mut dump, "targets").expect("string writes should not fail");
        let targets = CachedTarget::sorted(&package.targets);
        if targets.is_empty() {
            writeln!(&mut dump, "- <none>").expect("string writes should not fail");
        }
        for target in targets {
            writeln!(
                &mut dump,
                "- {} [{}] {}",
                target.name,
                target.kind,
                render_path(&target.src_path),
            )
            .expect("string writes should not fail");
        }

        // Pair each dependency alias with its destination package and enabled dependency kinds.
        writeln!(&mut dump, "dependencies").expect("string writes should not fail");
        if package.dependencies.is_empty() {
            writeln!(&mut dump, "- <none>").expect("string writes should not fail");
        }
        for dependency in CachedDependency::sorted(&package.dependencies) {
            let destination = cache_plan
                .packages()
                .iter()
                .find(|package| package.package == dependency.package)
                .map(|package| format!("{} (#{})", package.name, package.package.0))
                .unwrap_or_else(|| format!("unknown package (#{})", dependency.package.0));
            let mut kinds = Vec::new();
            if dependency.is_normal {
                kinds.push("normal");
            }
            if dependency.is_build {
                kinds.push("build");
            }
            if dependency.is_dev {
                kinds.push("dev");
            }
            writeln!(
                &mut dump,
                "- {} -> {destination} [{}]",
                dependency.name,
                kinds.join(", "),
            )
            .expect("string writes should not fail");
        }
    }

    dump
}
