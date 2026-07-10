use std::{fmt::Write as _, fs, path::Path};

use expect_test::Expect;
use rg_body_ir::{BodyIrBuildPolicy, PackageBodies};
use rg_def_map::PackageSlot;
use rg_ir_storage::PackageDefMaps;
use rg_parse::PackageParseSnapshot;
use rg_semantic_ir::PackageIr;
use rg_workspace::WorkspaceMetadata;

use crate::cache::codec::{PACKAGE_CACHE_CONTAINER_PREFIX_BYTES, PackageCacheLayout};
use crate::cache::{
    CURRENT_PACKAGE_CACHE_SCHEMA_VERSION, CachedCfgOptions, CachedDependency, CachedPackage,
    CachedPackageId, CachedPackageSlot, CachedPackageSource, CachedPath, CachedRustEdition,
    CachedTarget, CachedTargetKind, Fingerprint, PackageCacheArtifact, PackageCacheCodec,
    PackageCacheHeader, PackageCachePayload, WorkspaceCachePlan,
};
use crate::profile::metric;
use crate::{
    PackageResidencyPolicy, Project,
    testonly::{ProjectFixture, ProjectSourceFixture},
};

pub(super) fn check_cache_plan(fixture: &str, expect: Expect) {
    let fixture = ProjectSourceFixture::build(fixture);
    let workspace = fixture.workspace_metadata();
    let cache_plan = WorkspaceCachePlan::build(&workspace);
    let actual = render_cache_plan(&workspace, &cache_plan);

    expect.assert_eq(&format!("{}\n", actual.trim_end()));
}

pub(super) fn check_cache_header_codec(expect: Expect) {
    let header = PackageCacheHeader::new(
        CachedPackage {
            package: CachedPackageSlot(7),
            package_id: CachedPackageId("path+file:///workspace#app@0.1.0".into()),
            name: "app".to_string(),
            source: CachedPackageSource::Workspace,
            edition: CachedRustEdition::Edition2024,
            manifest_path: CachedPath("/workspace/Cargo.toml".into()),
            cfg_options: CachedCfgOptions::default(),
            targets: vec![
                CachedTarget {
                    name: "app".to_string(),
                    kind: CachedTargetKind::Lib,
                    src_path: CachedPath("/workspace/src/lib.rs".into()),
                },
                CachedTarget {
                    name: "app-cli".to_string(),
                    kind: CachedTargetKind::Bin,
                    src_path: CachedPath("/workspace/src/main.rs".into()),
                },
            ],
            dependencies: vec![CachedDependency {
                package_id: CachedPackageId("path+file:///workspace/dep#dep@0.1.0".into()),
                name: "dep".to_string(),
                is_normal: true,
                is_build: false,
                is_dev: false,
            }],
        },
        Fingerprint::from_stable_bytes([7; 32]),
    );

    let bytes =
        PackageCacheCodec::encode_header(&header).expect("package cache header should serialize");
    let decoded =
        PackageCacheCodec::decode_header(&bytes).expect("package cache header should deserialize");
    assert_eq!(decoded, header);

    let mut dump = String::new();
    writeln!(&mut dump, "encoded header bytes {}", bytes.len())
        .expect("string writes should not fail");
    render_hex(&bytes, &mut dump);
    writeln!(&mut dump).expect("string writes should not fail");
    render_header("decoded header", &decoded, &mut dump);

    expect.assert_eq(&format!("{}\n", dump.trim_end()));
}

pub(super) fn check_minimal_cache_artifact_codec(expect: Expect) {
    let artifact = PackageCacheArtifact::new(
        PackageCacheHeader::new(
            CachedPackage {
                package: CachedPackageSlot(7),
                package_id: CachedPackageId("path+file:///workspace#empty@0.1.0".into()),
                name: String::new(),
                source: CachedPackageSource::Workspace,
                edition: CachedRustEdition::Edition2024,
                manifest_path: CachedPath("/workspace/Cargo.toml".into()),
                cfg_options: CachedCfgOptions::default(),
                targets: Vec::new(),
                dependencies: Vec::new(),
            },
            Fingerprint::from_stable_bytes([7; 32]),
        ),
        PackageCachePayload::new(
            PackageParseSnapshot::empty(),
            PackageDefMaps::default(),
            PackageIr::default(),
            PackageBodies::default(),
        ),
    );

    let bytes = PackageCacheCodec::encode_artifact(&artifact)
        .expect("package cache artifact should serialize");
    let decoded = PackageCacheCodec::decode_artifact(&bytes)
        .expect("package cache artifact should deserialize");
    assert_eq!(decoded, artifact);

    let mut dump = String::new();
    writeln!(
        &mut dump,
        "encoded artifact has bytes {}",
        !bytes.is_empty()
    )
    .expect("string writes should not fail");
    render_hex(&bytes, &mut dump);
    writeln!(&mut dump).expect("string writes should not fail");
    render_artifact("decoded artifact", &decoded, &mut dump);

    expect.assert_eq(&format!("{}\n", dump.trim_end()));
}

pub(super) fn check_fixture_cache_artifact_codec(fixture: &str, expect: Expect) {
    let fixture = ProjectSourceFixture::build(fixture);
    let project = fixture.build_project();
    let artifact = package_artifact_from_project(&project, PackageSlot(0));

    let bytes = PackageCacheCodec::encode_artifact(&artifact)
        .expect("package cache artifact should serialize");
    let decoded = PackageCacheCodec::decode_artifact(&bytes)
        .expect("package cache artifact should deserialize");
    assert_eq!(decoded, artifact);

    let mut dump = String::new();
    writeln!(
        &mut dump,
        "encoded artifact has bytes {}",
        !bytes.is_empty()
    )
    .expect("string writes should not fail");
    render_artifact("decoded artifact", &decoded, &mut dump);

    expect.assert_eq(&format!("{}\n", dump.trim_end()));
}

pub(super) fn check_cache_store_artifact_io(fixture: &str, expect: Expect) {
    let fixture = ProjectSourceFixture::build(fixture);
    let project = fixture.build_project();
    let artifact = package_artifact_from_project(&project, PackageSlot(0));
    let store = project.state.cache_store.clone();
    let path = store.package_artifact_path(&artifact.header.package);

    store
        .clear_package_artifacts()
        .expect("fixture cache namespace should start empty for direct store I/O");
    let missing_before_write = store
        .read_artifact(&artifact.header)
        .expect("missing package cache artifact should not fail")
        .is_none();

    store
        .write_artifact(&artifact)
        .expect("package cache artifact should write to disk");
    let loaded = store
        .read_artifact(&artifact.header)
        .expect("written package cache artifact should read from disk")
        .expect("written package cache artifact should exist");
    assert_eq!(loaded, artifact);
    let written_len = fs::metadata(&path)
        .expect("written package cache artifact should have file metadata")
        .len();

    // Corruption is surfaced as a cache problem, not silently treated as a miss. The higher-level
    // invalidation layer will decide whether to wipe and rebuild.
    fs::write(&path, b"not a package cache artifact")
        .expect("test should overwrite package cache artifact with invalid bytes");
    let corrupt_error = store
        .read_artifact(&artifact.header)
        .expect_err("corrupted package cache artifact should fail to decode");
    let corrupt_error_text = format!("{corrupt_error:#}");

    store
        .clear_package_artifacts()
        .expect("package cache artifacts should be removable");
    let missing_after_invalidation = store
        .read_artifact(&artifact.header)
        .expect("missing package cache artifact should not fail after invalidation")
        .is_none();

    let mut dump = String::new();
    writeln!(&mut dump, "cache store artifact I/O").expect("string writes should not fail");
    writeln!(&mut dump, "missing before write {missing_before_write}")
        .expect("string writes should not fail");
    writeln!(&mut dump, "written artifact has bytes {}", written_len > 0)
        .expect("string writes should not fail");
    writeln!(
        &mut dump,
        "loaded package #{} {}",
        loaded.header.package.package.0, loaded.header.package.name,
    )
    .expect("string writes should not fail");
    writeln!(
        &mut dump,
        "corrupt read has typed decode error {}",
        corrupt_error_text.contains("failed to decode artifact"),
    )
    .expect("string writes should not fail");
    writeln!(
        &mut dump,
        "missing after invalidation {missing_after_invalidation}",
    )
    .expect("string writes should not fail");

    expect.assert_eq(&format!("{}\n", dump.trim_end()));
}

pub(super) fn check_sectioned_cache_reads(fixture: &str) {
    let fixture = ProjectSourceFixture::build(fixture);
    let project = fixture.build_project();
    let artifact = package_artifact_from_project(&project, PackageSlot(0));
    let store = project.state.cache_store.clone();
    let path = store.package_artifact_path(&artifact.header.package);
    store
        .write_artifact(&artifact)
        .expect("package cache artifact should write to disk");

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
        .read_probe_for_package(&artifact.header.package)
        .expect("probe should not decode the corrupt Body IR section")
        .expect("written probe should exist");
    assert_eq!(probe.header, artifact.header);

    let reader = store
        .open_artifact(&artifact.header)
        .expect("opening the artifact should only decode its probe")
        .expect("written artifact should exist");
    assert_eq!(
        reader
            .read_def_map()
            .expect("DefMap should decode independently"),
        artifact.payload.def_map,
    );
    let body_error = reader
        .read_body_ir()
        .expect_err("corrupt Body IR should fail when that section is requested");
    assert!(
        format!("{body_error:#}").contains("Body IR"),
        "Body IR decode failure should retain section context: {body_error:#}",
    );
}

pub(super) fn check_file_local_query_reads_one_body_shard() {
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
        .targets_for_file(PackageSlot(0), file)
        .expect("fixture target lookup should start")
        .into_iter()
        .next()
        .expect("fixture file should belong to a target");

    let run = rg_profile::test_support::ProfileTest::start(
        crate::profile_descriptors(),
        "project.cache.sections",
    );
    let analysis = snapshot
        .analysis_for_targets(&[target])
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

pub(super) fn check_cache_store_generation_cleanup(fixture: &str, expect: Expect) {
    let fixture = ProjectSourceFixture::build(fixture);
    let project = fixture.build_project();
    let artifact = package_artifact_from_project(&project, PackageSlot(0));
    let store = project.state.cache_store.clone();
    let current_artifact = store.package_artifact_path(&artifact.header.package);

    store
        .write_artifact(&artifact)
        .expect("package cache artifact should write to disk");
    let packages_dir = store.root().join("packages");
    let stale_generation = packages_dir.join("graph-stale");
    fs::create_dir_all(&stale_generation).expect("stale generation dir should be creatable");
    fs::write(stale_generation.join("old.rgpkg"), b"old artifact")
        .expect("stale generation artifact should be writable");

    let current_artifact_before_cleanup = current_artifact.exists();
    store
        .cleanup_stale_generations()
        .expect("stale generation cleanup should succeed");

    let mut dump = String::new();
    writeln!(&mut dump, "cache store generation cleanup").expect("string writes should not fail");
    writeln!(
        &mut dump,
        "current artifact before cleanup {current_artifact_before_cleanup}",
    )
    .expect("string writes should not fail");
    writeln!(
        &mut dump,
        "stale generation after cleanup {}",
        stale_generation.exists(),
    )
    .expect("string writes should not fail");
    writeln!(
        &mut dump,
        "current artifact after cleanup {}",
        current_artifact.exists(),
    )
    .expect("string writes should not fail");

    expect.assert_eq(&format!("{}\n", dump.trim_end()));
}

pub(super) fn check_residency_policy_controls_artifact_writes(fixture: &str, expect: Expect) {
    let fixture = ProjectSourceFixture::build(fixture);
    let workspace = fixture.workspace_metadata();

    let mut dump = String::new();
    writeln!(&mut dump, "artifact writes by residency policy")
        .expect("string writes should not fail");
    render_artifact_existence_for_policy(
        "all-resident",
        &workspace,
        PackageResidencyPolicy::AllResident,
        &mut dump,
    );
    writeln!(&mut dump).expect("string writes should not fail");
    render_artifact_existence_for_policy(
        "workspace-resident",
        &workspace,
        PackageResidencyPolicy::WorkspaceResident,
        &mut dump,
    );
    writeln!(&mut dump).expect("string writes should not fail");
    render_artifact_existence_for_policy(
        "all-offloadable",
        &workspace,
        PackageResidencyPolicy::AllOffloadable,
        &mut dump,
    );

    expect.assert_eq(&format!("{}\n", dump.trim_end()));
}

pub(super) fn check_offloaded_dependency_query(fixture: &str, expect: Expect) {
    let fixture = ProjectSourceFixture::build(fixture);
    let project = fixture
        .build_project_with_package_residency_policy(PackageResidencyPolicy::WorkspaceResident);
    let dep = ProjectFixture::package_slot_by_name_in(project.snapshot().parse_db(), "dep");
    let analysis = project
        .snapshot()
        .full_analysis()
        .expect("offloaded package read transaction should load");
    let mut symbols = analysis
        .workspace_symbols("DepType")
        .expect("fixture workspace symbols should resolve");
    symbols.sort_by_key(|symbol| {
        (
            symbol.kind,
            symbol.name.clone(),
            symbol.target.package.0,
            symbol.target.target.0,
        )
    });

    let mut dump = String::new();
    writeln!(&mut dump, "offloaded dependency query").expect("string writes should not fail");
    writeln!(
        &mut dump,
        "dep resident {}",
        project.state.def_map.resident_package(dep).is_some(),
    )
    .expect("string writes should not fail");
    writeln!(&mut dump, "symbols").expect("string writes should not fail");

    for symbol in symbols {
        let package = project
            .snapshot()
            .parse_db()
            .package(symbol.target.package.0)
            .expect("workspace symbol package should be parsed");
        let target = package
            .target(symbol.target.target)
            .expect("workspace symbol target should be parsed");
        writeln!(
            &mut dump,
            "- {} {} @ {}[{}]",
            symbol.kind,
            symbol.name,
            package.package_name(),
            target.kind,
        )
        .expect("string writes should not fail");
    }

    expect.assert_eq(&format!("{}\n", dump.trim_end()));
}

pub(super) fn check_startup_cache_rejects_forged_source_fingerprint(expect: Expect) {
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
pub struct DepOld;
"#,
    );
    let workspace = fixture.workspace_metadata();
    let project = Project::builder(workspace.clone())
        .package_residency_policy(PackageResidencyPolicy::WorkspaceResident)
        .build()
        .expect("fixture project should build");
    let dep = ProjectFixture::package_slot_by_name_in(project.snapshot().parse_db(), "dep");
    let old_header = project
        .state
        .cache_plan
        .artifact_header(dep, &project.state.package_source_fingerprints)
        .expect("dependency should have a cache artifact header");
    let mut artifact = project
        .state
        .cache_store
        .read_artifact(&old_header)
        .expect("written dependency artifact should be readable")
        .expect("written dependency artifact should exist");

    fixture.write_fixture_files(
        r#"
//- /dep/src/lib.rs
pub struct DepNew;
"#,
    );
    let workspace_after_edit = fixture.workspace_metadata();
    let cache_plan_after_edit = WorkspaceCachePlan::build(&workspace_after_edit);
    let parse_after_edit = rg_parse::ParseDb::build(&workspace_after_edit)
        .expect("fixture parse db should build after source edit");
    let source_fingerprints = cache_plan_after_edit
        .source_fingerprints(workspace_after_edit.workspace_root(), &parse_after_edit)
        .expect("edited source fingerprints should compute");

    // Forge the old artifact header so its package fingerprint claims to describe the edited
    // source. The per-file source descriptors still prove that the payload belongs to `DepOld`, so
    // startup must reject it and rebuild `DepNew` rather than trusting the header alone.
    artifact.header.source_fingerprint =
        source_fingerprints[dep.0].expect("edited dependency source should have a fingerprint");
    project
        .state
        .cache_store
        .write_artifact(&artifact)
        .expect("test should rewrite dependency artifact header");

    drop(project);
    let cached_project = Project::builder(workspace_after_edit)
        .package_residency_policy(PackageResidencyPolicy::WorkspaceResident)
        .build()
        .expect("fixture project should rebuild from matching artifact");
    let analysis = cached_project
        .snapshot()
        .full_analysis()
        .expect("cached project analysis should construct");
    let old_symbols = analysis
        .workspace_symbols("DepOld")
        .expect("old dependency symbol query should resolve");
    let new_symbols = analysis
        .workspace_symbols("DepNew")
        .expect("new dependency symbol query should resolve");

    let mut dump = String::new();
    writeln!(&mut dump, "startup artifact-backed indexing").expect("string writes should not fail");
    writeln!(
        &mut dump,
        "dep resident {}",
        cached_project.state.def_map.resident_package(dep).is_some(),
    )
    .expect("string writes should not fail");
    writeln!(&mut dump, "old symbols {}", old_symbols.len())
        .expect("string writes should not fail");
    writeln!(&mut dump, "new symbols {}", new_symbols.len())
        .expect("string writes should not fail");

    expect.assert_eq(&format!("{}\n", dump.trim_end()));
}

pub(super) fn check_artifact_snapshot_source_fingerprint_matches_package_sources(expect: Expect) {
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
    let artifact = project
        .state
        .cache_store
        .read_artifact(&header)
        .expect("written dependency artifact should be readable")
        .expect("written dependency artifact should exist");
    let snapshot_fingerprint = WorkspaceCachePlan::snapshot_source_fingerprint(
        project.workspace().workspace_root(),
        &artifact.header.package,
        &artifact.payload.parse,
    )
    .expect("artifact parse snapshot source fingerprint should compute");
    let source_fingerprint = project.state.package_source_fingerprints[dep.0]
        .expect("dependency source fingerprint should be recorded");
    let parse_package = project
        .snapshot()
        .parse_db()
        .package(dep.0)
        .expect("dependency should be parsed");

    let mut dump = String::new();
    writeln!(&mut dump, "artifact snapshot source fingerprint")
        .expect("string writes should not fail");
    writeln!(&mut dump, "package {}", parse_package.package_name())
        .expect("string writes should not fail");
    writeln!(
        &mut dump,
        "parse files {}",
        artifact.payload.parse.files().len(),
    )
    .expect("string writes should not fail");
    writeln!(
        &mut dump,
        "matches {}",
        snapshot_fingerprint == source_fingerprint,
    )
    .expect("string writes should not fail");

    expect.assert_eq(&format!("{}\n", dump.trim_end()));
}

pub(super) fn check_startup_cache_probe_profile(expect: Expect) {
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
pub struct Dep;
"#,
    );
    let workspace = fixture.workspace_metadata();
    Project::builder(workspace.clone())
        .package_residency_policy(PackageResidencyPolicy::WorkspaceResident)
        .build()
        .expect("fixture project should write dependency cache artifact");

    let run = rg_profile::test_support::ProfileTest::start(
        crate::profile_descriptors(),
        "project.build.cache_probe",
    );
    let _project = Project::builder(workspace)
        .package_residency_policy(PackageResidencyPolicy::WorkspaceResident)
        .build()
        .expect("fixture project should build from dependency cache artifact");
    let snapshot = run.finish();

    let mut dump = String::new();
    writeln!(&mut dump, "startup cache probe profile").expect("string writes should not fail");
    writeln!(
        &mut dump,
        "packages {}",
        profile_counter(&snapshot, metric::CACHE_PROBE_PACKAGES)
    )
    .expect("string writes should not fail");
    writeln!(
        &mut dump,
        "resident {}",
        profile_counter(&snapshot, metric::CACHE_PROBE_RESIDENT_PACKAGES)
    )
    .expect("string writes should not fail");
    writeln!(
        &mut dump,
        "offloadable {}",
        profile_counter(&snapshot, metric::CACHE_PROBE_OFFLOADABLE_PACKAGES)
    )
    .expect("string writes should not fail");
    writeln!(
        &mut dump,
        "hits {}",
        profile_counter(&snapshot, metric::CACHE_PROBE_HITS)
    )
    .expect("string writes should not fail");
    writeln!(&mut dump, "misses {}", cache_probe_misses(&snapshot))
        .expect("string writes should not fail");

    expect.assert_eq(&format!("{}\n", dump.trim_end()));
}

pub(super) fn check_startup_cache_misses_rebuild_reverse_dependents(expect: Expect) {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[workspace]
members = ["dep", "mid", "app", "independent"]
resolver = "3"

//- /dep/Cargo.toml
[package]
name = "dep"
version = "0.1.0"
edition = "2024"

//- /dep/src/lib.rs
pub struct Before;
pub struct Kept;

//- /mid/Cargo.toml
[package]
name = "mid"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "../dep" }

//- /mid/src/lib.rs
pub use dep::Kept;

//- /app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
mid = { path = "../mid" }

//- /app/src/lib.rs
pub fn keep(value: mid::Ke$usage$pt) -> mid::Kept { value }

//- /independent/Cargo.toml
[package]
name = "independent"
version = "0.1.0"
edition = "2024"

//- /independent/src/lib.rs
pub struct Independent;
"#,
    );
    let workspace = fixture.workspace_metadata();
    let project = Project::builder(workspace.clone())
        .package_residency_policy(PackageResidencyPolicy::AllOffloadable)
        .build()
        .expect("fixture project should write the first cache generation");
    drop(project);

    // Only the dependency source changes between processes. Its own artifact must miss, and the
    // cache plan must reject both reverse dependents even though their source files still match.
    fixture.write_fixture_files(
        r#"
//- /dep/src/lib.rs
pub struct AfterOne;
pub struct AfterTwo;
pub struct Kept;
"#,
    );
    let workspace_after_edit = fixture.workspace_metadata();
    let run = rg_profile::test_support::ProfileTest::start(
        crate::profile_descriptors(),
        "project.build.cache_probe",
    );
    let project = Project::builder(workspace_after_edit)
        .package_residency_policy(PackageResidencyPolicy::AllOffloadable)
        .build()
        .expect("dependency cache miss should rebuild its reverse dependents");
    let profile = run.finish();

    let marker = fixture.markers().position("usage");
    let snapshot = project.snapshot();
    let context = snapshot
        .file_contexts_for_path(fixture.path(&marker.path))
        .expect("app source should resolve to a file context")
        .pop()
        .expect("app source should have one file context");
    let target = context
        .targets
        .first()
        .copied()
        .expect("app source should belong to one target");
    let analysis = snapshot
        .full_analysis()
        .expect("rebuilt project analysis should materialize");
    let mut definitions = analysis
        .goto_definition(target, context.file, marker.offset)
        .expect("reexported dependency type should resolve");
    definitions.sort_by_key(|definition| {
        (
            definition.target.package.0,
            definition.target.target.0,
            definition.name.clone(),
        )
    });

    let mut dump = String::new();
    writeln!(&mut dump, "dependency cache miss closure").expect("string writes should not fail");
    writeln!(
        &mut dump,
        "hits {}",
        profile_counter(&profile, metric::CACHE_PROBE_HITS),
    )
    .expect("string writes should not fail");
    writeln!(
        &mut dump,
        "direct restore misses {}",
        profile_counter(&profile, metric::CACHE_PROBE_PARSE_RESTORE_ERRORS),
    )
    .expect("string writes should not fail");
    writeln!(
        &mut dump,
        "reverse-dependent misses {}",
        profile_counter(&profile, metric::CACHE_PROBE_PROPAGATED_MISSES),
    )
    .expect("string writes should not fail");
    writeln!(&mut dump, "definitions").expect("string writes should not fail");
    for definition in definitions {
        let package = snapshot
            .parse_db()
            .package(definition.target.package.0)
            .expect("definition package should exist");
        writeln!(
            &mut dump,
            "- {} {} {}",
            package.package_name(),
            definition.kind,
            definition.name,
        )
        .expect("string writes should not fail");
    }

    expect.assert_eq(&format!("{}\n", dump.trim_end()));
}

pub(super) fn check_missing_dependency_artifact_rebuilds_reverse_dependents(expect: Expect) {
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
pub struct App(dep::Dep);

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
    let project = Project::builder(workspace.clone())
        .package_residency_policy(PackageResidencyPolicy::AllOffloadable)
        .build()
        .expect("fixture project should write both package artifacts");
    let dependency = package_cache_artifact_for(&project, "dep");
    let dependency_path = project
        .state
        .cache_store
        .package_artifact_path(&dependency.header.package);
    fs::remove_file(&dependency_path).expect("test should remove dependency cache artifact");
    drop(project);

    let run = rg_profile::test_support::ProfileTest::start(
        crate::profile_descriptors(),
        "project.build.cache_probe",
    );
    let rebuilt = Project::builder(workspace)
        .package_residency_policy(PackageResidencyPolicy::AllOffloadable)
        .build()
        .expect("missing dependency artifact should rebuild the dependency closure");
    let profile = run.finish();
    let symbols = rebuilt
        .snapshot()
        .full_analysis()
        .expect("rebuilt project analysis should materialize")
        .workspace_symbols("Dep")
        .expect("dependency symbol query should resolve");

    let mut dump = String::new();
    writeln!(&mut dump, "missing dependency artifact closure")
        .expect("string writes should not fail");
    writeln!(
        &mut dump,
        "missing artifacts {}",
        profile_counter(&profile, metric::CACHE_PROBE_MISSING_ARTIFACTS),
    )
    .expect("string writes should not fail");
    writeln!(
        &mut dump,
        "reverse-dependent misses {}",
        profile_counter(&profile, metric::CACHE_PROBE_PROPAGATED_MISSES),
    )
    .expect("string writes should not fail");
    writeln!(
        &mut dump,
        "hits {}",
        profile_counter(&profile, metric::CACHE_PROBE_HITS),
    )
    .expect("string writes should not fail");
    writeln!(&mut dump, "dependency symbols {}", symbols.len())
        .expect("string writes should not fail");

    expect.assert_eq(&format!("{}\n", dump.trim_end()));
}

pub(super) fn check_startup_discards_incomplete_cache_update(expect: Expect) {
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
pub struct App(dep::Dep);

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
    let project = Project::builder(workspace.clone())
        .package_residency_policy(PackageResidencyPolicy::AllOffloadable)
        .build()
        .expect("fixture project should write a complete cache generation");
    let artifact = package_cache_artifact_for(&project, "dep");

    // Start another package-set write and deliberately omit commit. This models a process stopping
    // after one package replacement while older dependent artifacts still exist.
    {
        let update = project
            .state
            .cache_store
            .begin_artifact_update()
            .expect("test should start an incomplete cache update");
        update
            .write_artifact(&artifact)
            .expect("test should replace one package artifact");
    }
    drop(project);

    let recovery_run = rg_profile::test_support::ProfileTest::start(
        crate::profile_descriptors(),
        "project.build.cache_probe",
    );
    let recovered = Project::builder(workspace.clone())
        .package_residency_policy(PackageResidencyPolicy::AllOffloadable)
        .build()
        .expect("incomplete cache update should fall back to a cold build");
    let recovery_profile = recovery_run.finish();
    drop(recovered);

    let clean_run = rg_profile::test_support::ProfileTest::start(
        crate::profile_descriptors(),
        "project.build.cache_probe",
    );
    let _clean = Project::builder(workspace)
        .package_residency_policy(PackageResidencyPolicy::AllOffloadable)
        .build()
        .expect("cold recovery should publish a reusable cache again");
    let clean_profile = clean_run.finish();

    let mut dump = String::new();
    writeln!(&mut dump, "incomplete cache update recovery").expect("string writes should not fail");
    writeln!(
        &mut dump,
        "recovery hits {}",
        profile_counter(&recovery_profile, metric::CACHE_PROBE_HITS),
    )
    .expect("string writes should not fail");
    writeln!(
        &mut dump,
        "recovery missing artifacts {}",
        profile_counter(&recovery_profile, metric::CACHE_PROBE_MISSING_ARTIFACTS),
    )
    .expect("string writes should not fail");
    writeln!(
        &mut dump,
        "next startup hits {}",
        profile_counter(&clean_profile, metric::CACHE_PROBE_HITS),
    )
    .expect("string writes should not fail");

    expect.assert_eq(&format!("{}\n", dump.trim_end()));
}

pub(super) fn check_startup_cache_rejects_body_ir_policy_mismatch(expect: Expect) {
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
pub fn app_value() -> usize { 1 }

//- /dep/Cargo.toml
[package]
name = "dep"
version = "0.1.0"
edition = "2024"

//- /dep/src/lib.rs
pub fn dep_value() -> usize { 2 }
"#,
    );
    let workspace = fixture.workspace_metadata();
    Project::builder(workspace.clone())
        .package_residency_policy(PackageResidencyPolicy::WorkspaceResident)
        .build()
        .expect("fixture project should write workspace-policy dependency cache artifact");

    let run = rg_profile::test_support::ProfileTest::start(
        crate::profile_descriptors(),
        "project.build.cache_probe",
    );
    let project = Project::builder(workspace)
        .body_ir_policy(BodyIrBuildPolicy::all_packages())
        .package_residency_policy(PackageResidencyPolicy::WorkspaceResident)
        .build()
        .expect("fixture project should reject body-policy-mismatched artifact");
    let snapshot = run.finish();
    let artifact = package_cache_artifact_for(&project, "dep");

    let mut dump = String::new();
    writeln!(&mut dump, "startup body IR policy mismatch").expect("string writes should not fail");
    writeln!(
        &mut dump,
        "hits {}",
        profile_counter(&snapshot, metric::CACHE_PROBE_HITS)
    )
    .expect("string writes should not fail");
    writeln!(&mut dump, "misses {}", cache_probe_misses(&snapshot))
        .expect("string writes should not fail");
    writeln!(
        &mut dump,
        "body policy mismatches {}",
        profile_counter(&snapshot, metric::CACHE_PROBE_BODY_IR_POLICY_MISMATCHES),
    )
    .expect("string writes should not fail");
    render_body_ir_target_statuses(&artifact, &mut dump);

    expect.assert_eq(&format!("{}\n", dump.trim_end()));
}

fn profile_counter(
    snapshot: &rg_profile::test_support::TestSnapshot,
    metric: rg_profile::CounterMetric,
) -> u64 {
    snapshot.inner().counter(metric.path()).unwrap_or(0)
}

fn cache_probe_misses(snapshot: &rg_profile::test_support::TestSnapshot) -> u64 {
    [
        metric::CACHE_PROBE_MISSING_ARTIFACTS,
        metric::CACHE_PROBE_ARTIFACT_READ_ERRORS,
        metric::CACHE_PROBE_SOURCE_MISMATCHES,
        metric::CACHE_PROBE_SOURCE_ERRORS,
        metric::CACHE_PROBE_BODY_IR_POLICY_MISMATCHES,
        metric::CACHE_PROBE_PARSE_RESTORE_ERRORS,
        metric::CACHE_PROBE_UNPLANNED_PACKAGES,
        metric::CACHE_PROBE_PROPAGATED_MISSES,
    ]
    .into_iter()
    .map(|metric| profile_counter(snapshot, metric))
    .sum()
}

pub(super) fn check_startup_cache_rejects_stale_out_of_line_module(expect: Expect) {
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
pub struct DepChildOld;
"#,
    );
    let workspace = fixture.workspace_metadata();
    let project = Project::builder(workspace.clone())
        .package_residency_policy(PackageResidencyPolicy::WorkspaceResident)
        .build()
        .expect("fixture project should build");
    let dep = ProjectFixture::package_slot_by_name_in(project.snapshot().parse_db(), "dep");

    fixture.write_fixture_files(
        r#"
//- /dep/src/child.rs
pub struct DepChildNew;
"#,
    );
    let workspace_after_edit = fixture.workspace_metadata();

    // The changed file is discovered only after item-tree lowering. Startup cache validation must
    // therefore trust the artifact's saved parse manifest, not the fresh target-root-only parse DB.
    let cached_project = Project::builder(workspace_after_edit)
        .package_residency_policy(PackageResidencyPolicy::WorkspaceResident)
        .build()
        .expect("fixture project should reject stale artifact and rebuild from source");
    let analysis = cached_project
        .snapshot()
        .full_analysis()
        .expect("cached project analysis should construct");
    let old_symbols = analysis
        .workspace_symbols("DepChildOld")
        .expect("old dependency symbol query should resolve");
    let new_symbols = analysis
        .workspace_symbols("DepChildNew")
        .expect("new dependency symbol query should resolve");

    let mut dump = String::new();
    writeln!(&mut dump, "startup stale out-of-line module").expect("string writes should not fail");
    writeln!(
        &mut dump,
        "dep resident {}",
        cached_project.state.def_map.resident_package(dep).is_some(),
    )
    .expect("string writes should not fail");
    writeln!(&mut dump, "old symbols {}", old_symbols.len())
        .expect("string writes should not fail");
    writeln!(&mut dump, "new symbols {}", new_symbols.len())
        .expect("string writes should not fail");

    expect.assert_eq(&format!("{}\n", dump.trim_end()));
}

fn render_artifact_existence_for_policy(
    label: &str,
    workspace: &WorkspaceMetadata,
    policy: PackageResidencyPolicy,
    dump: &mut String,
) {
    let project = Project::builder(workspace.clone())
        .package_residency_policy(policy)
        .build()
        .unwrap_or_else(|error| panic!("{label} fixture project should build: {error:#}"));

    writeln!(dump, "{label}").expect("string writes should not fail");
    for package in project.snapshot().parse_db().packages() {
        writeln!(
            dump,
            "- {} artifact {}",
            package.package_name(),
            package_cache_artifact_exists_for(&project, package.package_name()),
        )
        .expect("string writes should not fail");
    }

    project
        .state
        .cache_store
        .clear_package_artifacts()
        .unwrap_or_else(|error| panic!("{label} fixture cache artifacts should clean up: {error}"));
}

fn package_artifact_from_project(project: &Project, package: PackageSlot) -> PackageCacheArtifact {
    let state = &project.state;
    let header = state
        .cache_plan
        .artifact_header(package, &state.package_source_fingerprints)
        .expect("cache-planned fixture package should have an artifact header");
    let def_map = state
        .def_map
        .resident_package(package)
        .expect("fixture package should have def-map data")
        .clone();
    let semantic_ir = state
        .semantic_ir
        .resident_package(package)
        .expect("fixture package should have semantic IR data")
        .clone();
    let body_ir = state
        .body_ir
        .resident_package(package)
        .expect("fixture package should have body IR data")
        .clone();

    PackageCacheArtifact::new(
        header,
        PackageCachePayload::new(
            state
                .parse
                .package(package.0)
                .expect("fixture package should have parse data")
                .parse_snapshot()
                .expect("fixture parse metadata should snapshot"),
            def_map,
            semantic_ir,
            body_ir,
        ),
    )
}

fn package_cache_artifact_for(project: &Project, package_name: &str) -> PackageCacheArtifact {
    let package = ProjectFixture::package_slot_by_name_in(project.state.parse_db(), package_name);
    let header = project
        .state
        .cache_plan
        .artifact_header(package, &project.state.package_source_fingerprints)
        .expect("fixture package should have a cache artifact header");

    project
        .state
        .cache_store
        .read_artifact(&header)
        .expect("fixture package cache artifact should read")
        .expect("fixture package cache artifact should exist")
}

fn package_cache_artifact_exists_for(project: &Project, package_name: &str) -> bool {
    let package = ProjectFixture::package_slot_by_name_in(project.state.parse_db(), package_name);
    let header = project
        .state
        .cache_plan
        .artifact_header(package, &project.state.package_source_fingerprints)
        .expect("fixture package should have a cache artifact header");

    project
        .state
        .cache_store
        .package_artifact_path(&header.package)
        .exists()
}

fn render_cache_plan(workspace: &WorkspaceMetadata, cache_plan: &WorkspaceCachePlan) -> String {
    let mut dump = String::new();
    writeln!(&mut dump, "workspace cache plan").expect("string writes should not fail");

    for package in cache_plan.packages() {
        writeln!(&mut dump).expect("string writes should not fail");
        render_package(workspace, cache_plan, package, &mut dump);
    }

    dump
}

fn render_package(
    workspace: &WorkspaceMetadata,
    cache_plan: &WorkspaceCachePlan,
    package: &CachedPackage,
    dump: &mut String,
) {
    writeln!(dump, "package #{} {}", package.package.0, package.name)
        .expect("string writes should not fail");
    writeln!(dump, "schema {}", CURRENT_PACKAGE_CACHE_SCHEMA_VERSION.0)
        .expect("string writes should not fail");
    writeln!(
        dump,
        "id {}",
        normalize_package_id(workspace.workspace_root(), &package.package_id.0),
    )
    .expect("string writes should not fail");
    writeln!(dump, "source {}", package.source).expect("string writes should not fail");
    writeln!(dump, "edition {}", package.edition).expect("string writes should not fail");
    writeln!(
        dump,
        "manifest {}",
        relative_path(workspace.workspace_root(), package.manifest_path.as_path())
    )
    .expect("string writes should not fail");

    render_targets(workspace, package, dump);
    render_dependencies(workspace, cache_plan, package, dump);
}

fn render_header(label: &str, header: &PackageCacheHeader, dump: &mut String) {
    writeln!(dump, "{label}").expect("string writes should not fail");
    writeln!(dump, "schema {}", header.schema_version.0).expect("string writes should not fail");
    writeln!(dump, "source fingerprint {}", header.source_fingerprint)
        .expect("string writes should not fail");
    writeln!(
        dump,
        "package #{} {}",
        header.package.package.0, header.package.name,
    )
    .expect("string writes should not fail");
    writeln!(dump, "id {}", header.package.package_id).expect("string writes should not fail");
    writeln!(dump, "source {}", header.package.source).expect("string writes should not fail");
    writeln!(dump, "edition {}", header.package.edition).expect("string writes should not fail");
    writeln!(dump, "manifest {}", header.package.manifest_path)
        .expect("string writes should not fail");

    writeln!(dump, "targets").expect("string writes should not fail");
    for target in CachedTarget::sorted(&header.package.targets) {
        writeln!(
            dump,
            "- {} [{}] {}",
            target.name, target.kind, target.src_path,
        )
        .expect("string writes should not fail");
    }

    writeln!(dump, "dependencies").expect("string writes should not fail");
    for dependency in CachedDependency::sorted(&header.package.dependencies) {
        writeln!(
            dump,
            "- {} -> {} {}",
            dependency.name,
            dependency.package_id,
            render_dependency_kinds(dependency),
        )
        .expect("string writes should not fail");
    }
}

fn render_artifact(label: &str, artifact: &PackageCacheArtifact, dump: &mut String) {
    writeln!(dump, "{label}").expect("string writes should not fail");
    writeln!(dump, "schema {}", artifact.header.schema_version.0)
        .expect("string writes should not fail");
    writeln!(
        dump,
        "source fingerprint {}",
        artifact.header.source_fingerprint,
    )
    .expect("string writes should not fail");
    writeln!(
        dump,
        "package #{} {}",
        artifact.header.package.package.0, artifact.header.package.name,
    )
    .expect("string writes should not fail");
    writeln!(
        dump,
        "header targets {}",
        artifact.header.package.targets.len()
    )
    .expect("string writes should not fail");
    writeln!(dump, "parse files {}", artifact.payload.parse.files().len())
        .expect("string writes should not fail");
    writeln!(
        dump,
        "parse target roots {}",
        artifact.payload.parse.target_root_count()
    )
    .expect("string writes should not fail");
    writeln!(
        dump,
        "def-map package {} targets {}",
        artifact.payload.def_map.package_name(),
        artifact.payload.def_map.def_maps().len(),
    )
    .expect("string writes should not fail");
    writeln!(
        dump,
        "semantic IR targets {}",
        artifact.payload.semantic_ir.targets().len(),
    )
    .expect("string writes should not fail");

    writeln!(
        dump,
        "body IR built targets {}",
        artifact.payload.body_ir.targets().len()
    )
    .expect("string writes should not fail");
}

fn render_body_ir_target_statuses(artifact: &PackageCacheArtifact, dump: &mut String) {
    writeln!(dump, "body IR target statuses").expect("string writes should not fail");
    for (target_idx, target) in artifact.payload.body_ir.targets().iter().enumerate() {
        writeln!(
            dump,
            "- target {target_idx} {} {}",
            target.status(),
            target.coverage()
        )
        .expect("string writes should not fail");
    }
}

fn render_targets(workspace: &WorkspaceMetadata, package: &CachedPackage, dump: &mut String) {
    writeln!(dump, "targets").expect("string writes should not fail");

    let targets = CachedTarget::sorted(&package.targets);

    if targets.is_empty() {
        writeln!(dump, "- <none>").expect("string writes should not fail");
        return;
    }

    for target in targets {
        writeln!(
            dump,
            "- {} [{}] {}",
            target.name,
            target.kind,
            relative_path(workspace.workspace_root(), target.src_path.as_path()),
        )
        .expect("string writes should not fail");
    }
}

fn render_dependencies(
    workspace: &WorkspaceMetadata,
    cache_plan: &WorkspaceCachePlan,
    package: &CachedPackage,
    dump: &mut String,
) {
    writeln!(dump, "dependencies").expect("string writes should not fail");

    if package.dependencies.is_empty() {
        writeln!(dump, "- <none>").expect("string writes should not fail");
        return;
    }

    let dependencies = CachedDependency::sorted(&package.dependencies);

    for dependency in dependencies {
        writeln!(
            dump,
            "- {} -> {} {}",
            dependency.name,
            render_dependency_package(workspace, cache_plan, &dependency.package_id),
            render_dependency_kinds(dependency),
        )
        .expect("string writes should not fail");
    }
}

fn render_dependency_package(
    workspace: &WorkspaceMetadata,
    cache_plan: &WorkspaceCachePlan,
    package_id: &CachedPackageId,
) -> String {
    cache_plan
        .packages()
        .iter()
        .find(|package| &package.package_id == package_id)
        .map(|package| format!("{} (#{})", package.name, package.package.0))
        .unwrap_or_else(|| normalize_package_id(workspace.workspace_root(), &package_id.0))
}

fn render_dependency_kinds(dependency: &CachedDependency) -> String {
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

    format!("[{}]", kinds.join(", "))
}

fn normalize_package_id(root: &Path, package_id: &str) -> String {
    let root_path = root.display().to_string();
    let mut root_paths = vec![root_path];

    // Cargo package IDs may preserve the non-canonical `/var` spelling on macOS while normalized
    // workspace paths point at `/private/var`. Treat both as the same fixture root in snapshots.
    let public_tmp_path = root_paths[0]
        .strip_prefix("/private/")
        .map(|path| format!("/{path}"));
    if let Some(public_tmp_path) = public_tmp_path {
        root_paths.push(public_tmp_path);
    }

    let mut package_id = package_id.to_string();
    for root_path in &root_paths {
        package_id = package_id.replace(&format!("file://{root_path}"), "file://./");
    }
    for root_path in root_paths {
        package_id = package_id.replace(&root_path, ".");
    }

    package_id.replace("file://.//", "file://./")
}

fn relative_path(root: &Path, path: &Path) -> String {
    let relative_path = path.strip_prefix(root).unwrap_or(path);

    if relative_path.as_os_str().is_empty() {
        ".".to_string()
    } else {
        relative_path.display().to_string()
    }
}

fn render_hex(bytes: &[u8], dump: &mut String) {
    for chunk in bytes.chunks(32) {
        for byte in chunk {
            write!(dump, "{byte:02x}").expect("string writes should not fail");
        }
        writeln!(dump).expect("string writes should not fail");
    }
}
