use std::{fmt::Write as _, fs};

use expect_test::expect;
use rg_def_map::PackageSlot;

use super::utils::{
    assert_reader_matches_resident_package, package_cache_header, package_cache_header_for,
    write_resident_package_artifact,
};
use crate::{PackageResidencyPolicy, Project, testonly::ProjectSourceFixture};

#[test]
fn stores_package_cache_artifacts_on_disk() {
    let fixture = ProjectSourceFixture::build(
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
    let project = fixture.build_project();
    let store = project.state.cache_store.clone();
    let header = package_cache_header(&project, PackageSlot(0));
    let path = store.package_artifact_path(&header.package);

    store
        .clear_package_artifacts()
        .expect("fixture cache namespace should start empty for direct store I/O");
    assert!(
        store
            .open_artifact(&header)
            .expect("missing package cache artifact should not fail")
            .is_none(),
        "cache namespace should start empty",
    );

    let written_header = write_resident_package_artifact(&project, PackageSlot(0));
    assert_eq!(written_header, header);
    let loaded = store
        .open_artifact(&header)
        .expect("written package cache artifact should read from disk")
        .expect("written package cache artifact should exist");
    assert_reader_matches_resident_package(&loaded, &project, PackageSlot(0));
    assert_eq!(loaded.probe().header.package.package.0, 0);
    assert_eq!(loaded.probe().header.package.name, "app");
    assert!(
        fs::metadata(&path)
            .expect("written package cache artifact should have file metadata")
            .len()
            > 0,
        "written artifact should contain bytes",
    );

    // Corruption is surfaced as a cache problem, not silently treated as a miss. The higher-level
    // invalidation layer will decide whether to wipe and rebuild.
    fs::write(&path, b"not a package cache artifact")
        .expect("test should overwrite package cache artifact with invalid bytes");
    let corrupt_error = store
        .open_artifact(&header)
        .expect_err("corrupted package cache artifact should fail to decode");
    assert!(
        format!("{corrupt_error:#}").contains("failed to decode artifact"),
        "corrupt reads should retain typed decode context: {corrupt_error:#}",
    );

    store
        .clear_package_artifacts()
        .expect("package cache artifacts should be removable");
    assert!(
        store
            .open_artifact(&header)
            .expect("missing package cache artifact should not fail after invalidation")
            .is_none(),
        "artifact should be absent after invalidation",
    );
}

#[test]
fn removes_stale_package_cache_generations() {
    let fixture = ProjectSourceFixture::build(
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
    let project = fixture.build_project();
    let store = project.state.cache_store.clone();
    let header = write_resident_package_artifact(&project, PackageSlot(0));
    let current_artifact = store.package_artifact_path(&header.package);
    assert!(
        current_artifact.exists(),
        "current artifact should exist before stale-generation cleanup",
    );

    let packages_dir = store.root().join("packages");
    let stale_generation = packages_dir.join("graph-stale");
    fs::create_dir_all(&stale_generation).expect("stale generation dir should be creatable");
    fs::write(stale_generation.join("old.rgpkg"), b"old artifact")
        .expect("stale generation artifact should be writable");

    store
        .cleanup_stale_generations()
        .expect("stale generation cleanup should succeed");

    assert!(
        !stale_generation.exists(),
        "stale cache generation should be removed",
    );
    assert!(
        current_artifact.exists(),
        "current cache artifact should survive stale-generation cleanup",
    );
}

#[test]
fn residency_policy_controls_package_artifact_writes() {
    let fixture = ProjectSourceFixture::build(
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
    );
    let workspace = fixture.workspace_metadata();
    let mut dump = String::new();
    writeln!(&mut dump, "artifact writes by residency policy")
        .expect("string writes should not fail");

    for (index, (label, policy)) in [
        ("all-resident", PackageResidencyPolicy::AllResident),
        (
            "workspace-resident",
            PackageResidencyPolicy::WorkspaceResident,
        ),
        ("all-offloadable", PackageResidencyPolicy::AllOffloadable),
    ]
    .into_iter()
    .enumerate()
    {
        if index > 0 {
            writeln!(&mut dump).expect("string writes should not fail");
        }

        let project = Project::builder(workspace.clone())
            .package_residency_policy(policy)
            .build()
            .unwrap_or_else(|error| panic!("{label} fixture project should build: {error:#}"));

        writeln!(&mut dump, "{label}").expect("string writes should not fail");
        for package in project.snapshot().parse_db().packages() {
            let header = package_cache_header_for(&project, package.package_name());
            let artifact_exists = project
                .state
                .cache_store
                .package_artifact_path(&header.package)
                .exists();
            writeln!(
                &mut dump,
                "- {} artifact {artifact_exists}",
                package.package_name(),
            )
            .expect("string writes should not fail");
        }

        project
            .state
            .cache_store
            .clear_package_artifacts()
            .unwrap_or_else(|error| {
                panic!("{label} fixture cache artifacts should clean up: {error}")
            });
    }

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
    "#]]
    .assert_eq(&format!("{}\n", dump.trim_end()));
}
