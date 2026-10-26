use std::fs;

use rg_ir_model::{CrateId, PackageSlot};

use super::utils::write_resident_package_artifact;
use crate::{
    PackageResidencyPolicy,
    profile::metric,
    storage::cache::codec::{PACKAGE_CACHE_CONTAINER_PREFIX_BYTES, PackageCacheLayout},
    testonly::{ProjectFixture, ProjectSourceFixture},
};

#[test]
fn probe_and_def_map_reads_do_not_decode_a_corrupt_body_section() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /src/lib.rs crate:app
pub fn answer() -> usize { 42 }
"#,
    );
    let project = fixture.build_project();
    let store = project.state.cache_store.clone();
    let header = write_resident_package_artifact(&project, PackageSlot(0));
    let path = store.package_artifact_path(&header.package);

    // Break only Body IR. The fixed prefix and probe remain readable, and DefMap has its own
    // independently encoded range before the corrupt bytes.
    let mut bytes = fs::read(&path).expect("written package cache artifact should be readable");
    let prefix = bytes
        .get(..PACKAGE_CACHE_CONTAINER_PREFIX_BYTES)
        .expect("written artifact should contain its fixed prefix");
    let layout = PackageCacheLayout::decode_prefix(prefix, bytes.len() as u64)
        .expect("written artifact should have a valid section layout");
    let body_start =
        usize::try_from(layout.body_ir.offset).expect("test Body IR offset should fit into usize");
    bytes[body_start] ^= 0xff;
    fs::write(&path, bytes).expect("test should overwrite the Body IR section");

    let probe = store
        .read_probe_for_package(&header.package)
        .expect("probe should not decode the corrupt Body IR section")
        .expect("written probe should exist");
    assert_eq!(probe.header, header);

    let reader = store
        .open_artifact(&header)
        .expect("opening the artifact should only decode its probe")
        .expect("written artifact should exist");
    assert_eq!(
        reader
            .read_def_map()
            .expect("DefMap should decode independently"),
        *project
            .state
            .def_map
            .resident_package(PackageSlot(0))
            .expect("fixture package should have resident DefMap"),
    );
    let body_error = reader
        .read_body_crate(CrateId(0))
        .expect_err("corrupt Body IR should fail when that section is requested");
    assert!(
        format!("{body_error:#}").contains("Body IR"),
        "Body IR decode failure should retain section context: {body_error:#}",
    );
}

#[test]
fn file_local_query_reads_one_body_file_shard() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /src/lib.rs crate:app
mod a;
mod b;

//- /src/a.rs
pub fn selected() {
    let value = 42u32;
    let _copy = val$0ue;
}

//- /src/b.rs
pub fn unrelated() {
    let text = "not selected";
    let _copy = text;
}
"#,
    );
    let project =
        fixture.build_project_with_package_residency_policy(PackageResidencyPolicy::AllOffloadable);
    assert!(
        project
            .state
            .body_ir
            .resident_package(PackageSlot(0))
            .is_none(),
        "all-offloadable fixture should exercise cache-backed Body IR",
    );
    let marker = fixture.markers().position("0");
    let snapshot = project.snapshot();
    let file =
        ProjectFixture::file_id_for_path_in(snapshot.parse_db(), &fixture.path(&marker.path));
    let target = snapshot
        .crates_for_file(PackageSlot(0), file)
        .expect("fixture target lookup should start")
        .into_iter()
        .next()
        .expect("fixture file should belong to a target");

    let run = rg_profile::test_support::ProfileTest::start(
        crate::profile_descriptors(),
        "project.cache.sections",
    );
    let analysis = snapshot
        .analysis_for_crates(&[target], rg_std::CancellationToken::new())
        .expect("fixture analysis should construct");
    assert!(
        analysis
            .type_at(target, file, marker.offset)
            .expect("fixture type query should resolve")
            .is_some(),
        "fixture marker should resolve through Body IR",
    );
    let profile = run.finish();

    profile.assert_keyed_duration_count(metric::CACHE_SECTION_READ, "body_ir.file", 1);
    profile.assert_keyed_duration_count(metric::CACHE_SECTION_DECODE, "body_ir.file", 1);
    assert!(
        profile
            .inner()
            .keyed_counter(metric::CACHE_SECTION_BYTES.path(), "body_ir.file")
            .is_some_and(|bytes| bytes > 0),
        "file-local query should read a non-empty Body IR file shard",
    );
    assert_eq!(
        profile
            .inner()
            .keyed_counter(metric::CACHE_SECTION_BYTES.path(), "body_ir"),
        None,
        "file-local query must not read the package-wide Body IR section",
    );
}
