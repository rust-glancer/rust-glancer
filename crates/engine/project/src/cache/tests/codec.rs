use std::{fs, path::Path};

use rg_body_ir::PackageBodies;
use rg_def_map::{PackageDefMaps, PackageSlot};
use rg_ir_model::CrateId;
use rg_parse::PackageParseSnapshot;
use rg_semantic_ir::PackageIr;

use super::utils::{assert_reader_matches_resident_package, write_resident_package_artifact};
use crate::cache::codec::{
    BODY_CACHE_CONTAINER_PREFIX_BYTES, PACKAGE_CACHE_CONTAINER_PREFIX_BYTES, PackageCacheLayout,
    PackageCacheSectionRange,
};
use crate::cache::{
    CachedCfgOptions, CachedPackage, CachedPackageSlot, CachedPackageSource, CachedPath,
    CachedRustEdition, Fingerprint, PackageCacheCodec, PackageCacheHeader, PackageCacheWriteInput,
    WorkspaceCachePlan,
};
use crate::profile::metric;
use crate::{
    PackageResidencyPolicy,
    testonly::{ProjectFixture, ProjectSourceFixture},
};

#[test]
fn roundtrips_minimal_package_cache_artifact_codec() {
    let header = PackageCacheHeader::new(
        CachedPackage {
            package: CachedPackageSlot(7),
            name: String::new(),
            source: CachedPackageSource::Workspace,
            edition: CachedRustEdition::Edition2024,
            manifest_path: CachedPath::from_workspace_path(
                Path::new("/workspace"),
                Path::new("/workspace/Cargo.toml"),
            ),
            cfg_options: CachedCfgOptions::default(),
            targets: Vec::new(),
            dependencies: Vec::new(),
        },
        Fingerprint::from_stable_bytes([7; 32]),
    );
    let parse = PackageParseSnapshot::empty();
    let def_map = PackageDefMaps::default();
    let semantic_ir = PackageIr::default();
    let body_ir = PackageBodies::default();

    // Exercise the production borrowed input and fragment writer before decoding every container
    // section independently, just like the filesystem store does.
    let encoded = PackageCacheCodec::encode_write_input(PackageCacheWriteInput::new(
        &header,
        &parse,
        &def_map,
        &semantic_ir,
        &body_ir,
    ))
    .expect("minimal package cache input should encode");
    let mut bytes = Vec::new();
    encoded
        .write_to(&mut bytes)
        .expect("encoded package cache fragments should write");
    let section_bytes = |range: PackageCacheSectionRange| {
        let start =
            usize::try_from(range.offset).expect("test cache section offset should fit usize");
        let len = usize::try_from(range.len).expect("test cache section length should fit usize");
        let end = start
            .checked_add(len)
            .expect("test cache section range should not overflow");
        bytes
            .get(start..end)
            .expect("test cache section should be inside encoded bytes")
    };

    let layout = PackageCacheLayout::decode_prefix(
        &bytes[..PACKAGE_CACHE_CONTAINER_PREFIX_BYTES],
        bytes.len() as u64,
    )
    .expect("minimal package cache layout should decode");
    let probe = PackageCacheCodec::decode_probe(section_bytes(layout.probe))
        .expect("minimal package cache probe should decode");
    let decoded_def_map = PackageCacheCodec::decode_def_map(section_bytes(layout.def_map), &probe)
        .expect("minimal package cache DefMap should decode");
    let decoded_semantic_ir =
        PackageCacheCodec::decode_semantic_ir(section_bytes(layout.semantic_ir), &probe)
            .expect("minimal package cache Semantic IR should decode");
    let body_bytes = section_bytes(layout.body_ir);
    let body_prefix = &body_bytes[..BODY_CACHE_CONTAINER_PREFIX_BYTES];
    let manifest_len = PackageCacheCodec::decode_body_prefix(body_prefix)
        .expect("minimal Body IR prefix should decode");
    let manifest_end = BODY_CACHE_CONTAINER_PREFIX_BYTES + manifest_len;
    let body_index = PackageCacheCodec::decode_body_index(
        &body_bytes[BODY_CACHE_CONTAINER_PREFIX_BYTES..manifest_end],
        body_bytes.len() as u64,
        &probe,
    )
    .expect("minimal Body IR manifest should decode");

    assert_eq!(probe.header, header);
    assert_eq!(probe.parse, parse);
    assert_eq!(decoded_def_map, def_map);
    assert_eq!(decoded_semantic_ir, semantic_ir);
    assert!(body_index.manifest().crates().is_empty());
    assert!(!bytes.is_empty(), "encoded artifact should contain bytes");
}

#[test]
fn roundtrips_fixture_package_cache_artifact_codec() {
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
    let header = write_resident_package_artifact(&project, PackageSlot(0));
    let path = project
        .state
        .cache_store
        .package_artifact_path(&header.package);
    let reader = project
        .state
        .cache_store
        .open_artifact(&header)
        .expect("fixture package cache artifact should open")
        .expect("fixture package cache artifact should exist");
    assert_reader_matches_resident_package(&reader, &project, PackageSlot(0));
    assert!(
        fs::metadata(path)
            .expect("fixture package cache artifact should have metadata")
            .len()
            > 0,
        "encoded fixture artifact should contain bytes",
    );
}

#[test]
fn probe_and_def_map_reads_do_not_decode_a_corrupt_body_section() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
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
//- /Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
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
        .analysis_for_crates(&[target])
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

#[test]
fn artifact_snapshot_source_fingerprint_matches_discovered_package_sources() {
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
pub struct App;

//- /dep/Cargo.toml
[package]
name = "dep"
version = "0.1.0"
edition = "2024"

//- /dep/src/lib.rs
mod child;

//- /dep/src/child.rs
pub struct DepChild;
"#,
    );
    let project = fixture
        .build_project_with_package_residency_policy(PackageResidencyPolicy::WorkspaceResident);
    let dep = ProjectFixture::package_slot_by_name_in(project.snapshot().parse_db(), "dep");
    let header = project
        .state
        .cache_plan
        .artifact_header(dep, &project.state.package_source_fingerprints)
        .expect("dependency should have a cache artifact header");
    let reader = project
        .state
        .cache_store
        .open_artifact(&header)
        .expect("written dependency artifact should be readable")
        .expect("written dependency artifact should exist");
    let snapshot_fingerprint = WorkspaceCachePlan::snapshot_source_fingerprint(
        project.workspace().workspace_root(),
        &reader.probe().header.package,
        &reader.probe().parse,
    )
    .expect("artifact parse snapshot source fingerprint should compute");
    let source_fingerprint = project.state.package_source_fingerprints[dep.0]
        .expect("dependency source fingerprint should be recorded");
    let parse_package = project
        .snapshot()
        .parse_db()
        .package(dep.0)
        .expect("dependency should be parsed");

    assert_eq!(parse_package.package_name(), "dep");
    assert_eq!(
        reader.probe().parse.files().len(),
        2,
        "dependency artifact should capture both discovered source files",
    );
    assert_eq!(
        snapshot_fingerprint, source_fingerprint,
        "artifact parse snapshot should reproduce the discovered package source fingerprint",
    );
}
