use rg_def_map::PackageSlot;
use rg_parse::ParseDb;

use crate::{
    cache::{PackageCacheInstance, PackageCacheStore, WorkspaceCachePlan},
    testonly::ProjectSourceFixture,
};

#[test]
fn cache_instances_claim_distinct_live_slots() {
    let fixture = instance_fixture();
    let workspace = fixture.workspace_metadata();

    let first = PackageCacheInstance::for_workspace(&workspace)
        .expect("first cache instance should claim a slot");
    let second = PackageCacheInstance::for_workspace(&workspace)
        .expect("second cache instance should claim a slot");

    assert_eq!(first.slot_for_tests(), 1);
    assert_eq!(second.slot_for_tests(), 2);
    assert_ne!(first.root(), second.root());
    assert!(first.root().join("instance.lock").exists());
    assert!(second.root().join("instance.lock").exists());
}

#[test]
fn cache_instances_reuse_unlocked_slots() {
    let fixture = instance_fixture();
    let workspace = fixture.workspace_metadata();

    let first = PackageCacheInstance::for_workspace(&workspace)
        .expect("first cache instance should claim a slot");
    let first_root = first.root().to_path_buf();
    assert_eq!(first.slot_for_tests(), 1);

    drop(first);
    let reused = PackageCacheInstance::for_workspace(&workspace)
        .expect("cache instance should reuse the unlocked first slot");

    assert_eq!(reused.slot_for_tests(), 1);
    assert_eq!(reused.root(), first_root.as_path());
}

#[test]
fn cache_stores_under_distinct_instances_use_distinct_artifact_paths() {
    let fixture = instance_fixture();
    let workspace = fixture.workspace_metadata();
    let cache_plan = WorkspaceCachePlan::build(&workspace);
    let first = PackageCacheInstance::for_workspace(&workspace)
        .expect("first cache instance should claim a slot");
    let second = PackageCacheInstance::for_workspace(&workspace)
        .expect("second cache instance should claim a slot");
    let first_store = PackageCacheStore::for_instance(&workspace, &cache_plan, &first);
    let second_store = PackageCacheStore::for_instance(&workspace, &cache_plan, &second);
    let header = package_header(&workspace, &cache_plan);

    assert_ne!(
        first_store.package_artifact_path(&header.package),
        second_store.package_artifact_path(&header.package),
    );
}

fn instance_fixture() -> ProjectSourceFixture {
    ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct App;
"#,
    )
}

fn package_header(
    workspace: &rg_workspace::WorkspaceMetadata,
    cache_plan: &WorkspaceCachePlan,
) -> crate::cache::PackageCacheHeader {
    let parse = ParseDb::build(workspace).expect("fixture parse db should build");
    let fingerprints = cache_plan
        .source_fingerprints(workspace.workspace_root(), &parse)
        .expect("fixture source fingerprints should compute");

    cache_plan
        .artifact_header(PackageSlot(0), &fingerprints)
        .expect("fixture package should have an artifact header")
}
