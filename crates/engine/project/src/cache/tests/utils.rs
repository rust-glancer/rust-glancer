use std::{fmt::Write as _, fs, path::Path};

use expect_test::Expect;
use rg_body_ir::{BodyIrBuildPolicy, PackageBodies};
use rg_def_map::PackageDefMaps;
use rg_def_map::PackageSlot;
use rg_ir_model::CrateId;
use rg_parse::PackageParseSnapshot;
use rg_semantic_ir::PackageIr;
use rg_workspace::WorkspaceMetadata;

use crate::cache::codec::{
    BODY_CACHE_CONTAINER_PREFIX_BYTES, PACKAGE_CACHE_CONTAINER_PREFIX_BYTES, PackageCacheLayout,
    PackageCacheSectionRange,
};
use crate::cache::{
    CURRENT_PACKAGE_CACHE_SCHEMA_VERSION, CachedCfgOptions, CachedDependency, CachedPackage,
    CachedPackageId, CachedPackageSlot, CachedPackageSource, CachedPath, CachedRustEdition,
    CachedTarget, Fingerprint, PackageArtifactReader, PackageCacheCodec, PackageCacheHeader,
    PackageCacheUpdate, PackageCacheWriteInput, WorkspaceCachePlan,
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

pub(super) fn check_minimal_cache_artifact_codec(expect: Expect) {
    let header = PackageCacheHeader::new(
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
    );
    let parse = PackageParseSnapshot::empty();
    let def_map = PackageDefMaps::default();
    let semantic_ir = PackageIr::default();
    let body_ir = PackageBodies::default();

    // Even the byte-level format snapshot starts from the production borrowed input and uses the
    // same fragment writer as the filesystem store.
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

    let layout = PackageCacheLayout::decode_prefix(
        &bytes[..PACKAGE_CACHE_CONTAINER_PREFIX_BYTES],
        bytes.len() as u64,
    )
    .expect("minimal package cache layout should decode");
    let probe = PackageCacheCodec::decode_probe(cache_section_bytes(&bytes, layout.probe))
        .expect("minimal package cache probe should decode");
    let decoded_def_map =
        PackageCacheCodec::decode_def_map(cache_section_bytes(&bytes, layout.def_map), &probe)
            .expect("minimal package cache DefMap should decode");
    let decoded_semantic_ir = PackageCacheCodec::decode_semantic_ir(
        cache_section_bytes(&bytes, layout.semantic_ir),
        &probe,
    )
    .expect("minimal package cache Semantic IR should decode");
    let body_bytes = cache_section_bytes(&bytes, layout.body_ir);
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

    let mut dump = String::new();
    writeln!(
        &mut dump,
        "encoded artifact has bytes {}",
        !bytes.is_empty()
    )
    .expect("string writes should not fail");
    render_hex(&bytes, &mut dump);
    writeln!(&mut dump).expect("string writes should not fail");
    render_decoded_artifact(
        "decoded artifact",
        &probe,
        &decoded_def_map,
        &decoded_semantic_ir,
        body_index.manifest().crates().len(),
        &mut dump,
    );

    expect.assert_eq(&format!("{}\n", dump.trim_end()));
}

pub(super) fn check_fixture_cache_artifact_codec(fixture: &str, expect: Expect) {
    let fixture = ProjectSourceFixture::build(fixture);
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

    let mut dump = String::new();
    writeln!(
        &mut dump,
        "encoded artifact has bytes {}",
        fs::metadata(path)
            .expect("fixture package cache artifact should have metadata")
            .len()
            > 0,
    )
    .expect("string writes should not fail");
    render_cached_artifact("decoded artifact", &reader, &mut dump);

    expect.assert_eq(&format!("{}\n", dump.trim_end()));
}

pub(super) fn check_cache_store_artifact_io(fixture: &str, expect: Expect) {
    let fixture = ProjectSourceFixture::build(fixture);
    let project = fixture.build_project();
    let store = project.state.cache_store.clone();
    let header = package_cache_header(&project, PackageSlot(0));
    let path = store.package_artifact_path(&header.package);

    store
        .clear_package_artifacts()
        .expect("fixture cache namespace should start empty for direct store I/O");
    let missing_before_write = store
        .open_artifact(&header)
        .expect("missing package cache artifact should not fail")
        .is_none();

    let written_header = write_resident_package_artifact(&project, PackageSlot(0));
    assert_eq!(written_header, header);
    let loaded = store
        .open_artifact(&header)
        .expect("written package cache artifact should read from disk")
        .expect("written package cache artifact should exist");
    assert_reader_matches_resident_package(&loaded, &project, PackageSlot(0));
    let written_len = fs::metadata(&path)
        .expect("written package cache artifact should have file metadata")
        .len();

    // Corruption is surfaced as a cache problem, not silently treated as a miss. The higher-level
    // invalidation layer will decide whether to wipe and rebuild.
    fs::write(&path, b"not a package cache artifact")
        .expect("test should overwrite package cache artifact with invalid bytes");
    let corrupt_error = store
        .open_artifact(&header)
        .expect_err("corrupted package cache artifact should fail to decode");
    let corrupt_error_text = format!("{corrupt_error:#}");

    store
        .clear_package_artifacts()
        .expect("package cache artifacts should be removable");
    let missing_after_invalidation = store
        .open_artifact(&header)
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
        loaded.probe().header.package.package.0,
        loaded.probe().header.package.name,
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

pub(super) fn check_cache_store_generation_cleanup(fixture: &str, expect: Expect) {
    let fixture = ProjectSourceFixture::build(fixture);
    let project = fixture.build_project();
    let store = project.state.cache_store.clone();
    let header = write_resident_package_artifact(&project, PackageSlot(0));
    let current_artifact = store.package_artifact_path(&header.package);

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

pub(super) fn check_residency_policy_change_rebuilds_from_source(expect: Expect) {
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
    let first = Project::builder(workspace.clone())
        .package_residency_policy(PackageResidencyPolicy::AllOffloadable)
        .build()
        .expect("fixture project should write the first residency generation");
    let first_artifact = first
        .state
        .cache_store
        .package_artifact_path(&package_cache_header_for(&first, "dep").package);
    let first_generation = first_artifact
        .parent()
        .expect("package artifact should belong to a generation directory")
        .to_path_buf();
    let first_generation_existed = first_generation.exists();
    drop(first);

    let transition_run = rg_profile::test_support::ProfileTest::start(
        crate::profile_descriptors(),
        "project.build.cache_probe",
    );
    let transitioned = Project::builder(workspace.clone())
        .package_residency_policy(PackageResidencyPolicy::WorkspaceResident)
        .build()
        .expect("residency change should rebuild into a fresh cache generation");
    let transition_profile = transition_run.finish();
    let transition_artifact = transitioned
        .state
        .cache_store
        .package_artifact_path(&package_cache_header_for(&transitioned, "dep").package);
    let transition_generation = transition_artifact
        .parent()
        .expect("package artifact should belong to a generation directory")
        .to_path_buf();

    let mut dump = String::new();
    writeln!(&mut dump, "residency policy cache invalidation")
        .expect("string writes should not fail");
    writeln!(
        &mut dump,
        "first generation existed {first_generation_existed}",
    )
    .expect("string writes should not fail");
    writeln!(
        &mut dump,
        "generation changed {}",
        first_generation != transition_generation,
    )
    .expect("string writes should not fail");
    writeln!(
        &mut dump,
        "transition hits {}",
        profile_counter(&transition_profile, metric::CACHE_PROBE_HITS),
    )
    .expect("string writes should not fail");
    writeln!(
        &mut dump,
        "transition missing artifacts {}",
        profile_counter(&transition_profile, metric::CACHE_PROBE_MISSING_ARTIFACTS,),
    )
    .expect("string writes should not fail");
    writeln!(
        &mut dump,
        "old generation after transition {}",
        first_generation.exists(),
    )
    .expect("string writes should not fail");
    writeln!(
        &mut dump,
        "transition artifact {}",
        transition_artifact.exists(),
    )
    .expect("string writes should not fail");
    drop(transitioned);

    // Switching back must not resurrect the generation used before the first policy change.
    let switch_back_run = rg_profile::test_support::ProfileTest::start(
        crate::profile_descriptors(),
        "project.build.cache_probe",
    );
    let _switched_back = Project::builder(workspace)
        .package_residency_policy(PackageResidencyPolicy::AllOffloadable)
        .build()
        .expect("switching residency back should rebuild from source again");
    let switch_back_profile = switch_back_run.finish();
    writeln!(
        &mut dump,
        "switch-back hits {}",
        profile_counter(&switch_back_profile, metric::CACHE_PROBE_HITS),
    )
    .expect("string writes should not fail");
    writeln!(
        &mut dump,
        "switch-back missing artifacts {}",
        profile_counter(&switch_back_profile, metric::CACHE_PROBE_MISSING_ARTIFACTS,),
    )
    .expect("string writes should not fail");

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
    let mut symbols = Vec::new();
    for query in ["DepType", "dep_foreign", "DEP_STATIC", "DepOpaque"] {
        symbols.extend(
            analysis
                .workspace_symbols(query)
                .expect("fixture workspace symbols should resolve"),
        );
    }
    symbols.sort_by_key(|symbol| {
        (
            symbol.kind,
            symbol.name.clone(),
            symbol.crate_ref.package.0,
            symbol.crate_ref.crate_id.0,
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
            .package(symbol.crate_ref.package.0)
            .expect("workspace symbol package should be parsed");
        // Crate ids are allocated in parsed Cargo-target order. The DefMap payload is deliberately
        // offloaded in this fixture, so render through the stable parsed package shape instead.
        let target = package
            .targets()
            .get(symbol.crate_ref.crate_id.0)
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
    let reader = project
        .state
        .cache_store
        .open_artifact(&old_header)
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
    let mut forged_header = old_header.clone();
    forged_header.source_fingerprint =
        source_fingerprints[dep.0].expect("edited dependency source should have a fingerprint");
    let update = project
        .state
        .cache_store
        .begin_artifact_update()
        .expect("test should start forged artifact update");
    write_cached_package_artifact(&update, &reader, &forged_header);
    update
        .commit()
        .expect("test should commit forged dependency artifact");

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

    let mut dump = String::new();
    writeln!(&mut dump, "artifact snapshot source fingerprint")
        .expect("string writes should not fail");
    writeln!(&mut dump, "package {}", parse_package.package_name())
        .expect("string writes should not fail");
    writeln!(
        &mut dump,
        "parse files {}",
        reader.probe().parse.files().len(),
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
        .crates
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
            definition.crate_ref.package.0,
            definition.crate_ref.crate_id.0,
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
            .package(definition.crate_ref.package.0)
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
    let dependency = package_cache_header_for(&project, "dep");
    let dependency_path = project
        .state
        .cache_store
        .package_artifact_path(&dependency.package);
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
    let header = package_cache_header_for(&project, "dep");
    let reader = project
        .state
        .cache_store
        .open_artifact(&header)
        .expect("dependency package cache artifact should open")
        .expect("dependency package cache artifact should exist");

    // Start another package-set write and deliberately omit commit. This models a process stopping
    // after one package replacement while older dependent artifacts still exist.
    {
        let update = project
            .state
            .cache_store
            .begin_artifact_update()
            .expect("test should start an incomplete cache update");
        write_cached_package_artifact(&update, &reader, &header);
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
    let header = package_cache_header_for(&project, "dep");
    let reader = project
        .state
        .cache_store
        .open_artifact(&header)
        .expect("dependency package cache artifact should open")
        .expect("dependency package cache artifact should exist");

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
    render_body_ir_crate_statuses(&reader, &mut dump);

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

fn package_cache_header(project: &Project, package: PackageSlot) -> PackageCacheHeader {
    let state = &project.state;
    state
        .cache_plan
        .artifact_header(package, &state.package_source_fingerprints)
        .expect("cache-planned fixture package should have an artifact header")
}

/// Write one resident fixture package through the same borrowed transaction path as production.
fn write_resident_package_artifact(project: &Project, package: PackageSlot) -> PackageCacheHeader {
    let state = &project.state;
    let header = package_cache_header(project, package);
    let parse = state
        .parse
        .package(package.0)
        .expect("fixture package should have parse data")
        .parse_snapshot()
        .expect("fixture parse metadata should snapshot");
    let def_map = state
        .def_map
        .resident_package(package)
        .expect("fixture package should have def-map data");
    let semantic_ir = state
        .semantic_ir
        .resident_package(package)
        .expect("fixture package should have semantic IR data");
    let body_ir = state
        .body_ir
        .resident_package(package)
        .expect("fixture package should have body IR data");

    let update = state
        .cache_store
        .begin_artifact_update()
        .expect("fixture package cache update should start");
    update
        .write_input(PackageCacheWriteInput::new(
            &header,
            &parse,
            def_map,
            semantic_ir,
            body_ir,
        ))
        .expect("fixture resident package should write to cache");
    update
        .commit()
        .expect("fixture package cache update should commit");
    header
}

fn package_cache_header_for(project: &Project, package_name: &str) -> PackageCacheHeader {
    let package = ProjectFixture::package_slot_by_name_in(project.state.parse_db(), package_name);
    package_cache_header(project, package)
}

/// Re-emit one cached package through the production lazy reader and borrowed writer.
fn write_cached_package_artifact(
    update: &PackageCacheUpdate<'_>,
    reader: &PackageArtifactReader,
    header: &PackageCacheHeader,
) {
    let def_map = reader
        .read_def_map()
        .expect("cached fixture DefMap should read");
    let semantic_ir = reader
        .read_semantic_ir()
        .expect("cached fixture Semantic IR should read");
    let manifest = reader
        .read_body_ir_manifest()
        .expect("cached fixture Body IR manifest should read");
    let body_ir = PackageBodies::new(
        (0..manifest.crates().len())
            .map(|target| {
                reader
                    .read_body_crate(CrateId(target))
                    .expect("cached fixture Body IR target should read")
            })
            .collect(),
    );

    update
        .write_input(PackageCacheWriteInput::new(
            header,
            &reader.probe().parse,
            &def_map,
            &semantic_ir,
            &body_ir,
        ))
        .expect("cached fixture package should write")
}

fn assert_reader_matches_resident_package(
    reader: &PackageArtifactReader,
    project: &Project,
    package: PackageSlot,
) {
    let state = &project.state;
    let expected_header = package_cache_header(project, package);
    let expected_parse = state
        .parse
        .package(package.0)
        .expect("fixture package should have parse data")
        .parse_snapshot()
        .expect("fixture parse metadata should snapshot");
    assert_eq!(reader.probe().header, expected_header);
    assert_eq!(reader.probe().parse, expected_parse);
    assert_eq!(
        reader
            .read_def_map()
            .expect("fixture cached DefMap should read"),
        *state
            .def_map
            .resident_package(package)
            .expect("fixture package should have resident DefMap"),
    );
    assert_eq!(
        reader
            .read_semantic_ir()
            .expect("fixture cached Semantic IR should read"),
        *state
            .semantic_ir
            .resident_package(package)
            .expect("fixture package should have resident Semantic IR"),
    );

    let expected_body_ir = state
        .body_ir
        .resident_package(package)
        .expect("fixture package should have resident Body IR");
    assert_eq!(
        reader
            .read_body_ir_manifest()
            .expect("fixture cached Body IR manifest should read"),
        expected_body_ir.manifest(),
    );
    for (target, expected) in expected_body_ir.crates().iter().enumerate() {
        assert_eq!(
            reader
                .read_body_crate(CrateId(target))
                .expect("fixture cached Body IR target should read"),
            *expected,
        );
    }
}

fn cache_section_bytes(bytes: &[u8], range: PackageCacheSectionRange) -> &[u8] {
    let start = usize::try_from(range.offset).expect("test cache section offset should fit usize");
    let len = usize::try_from(range.len).expect("test cache section length should fit usize");
    let end = start
        .checked_add(len)
        .expect("test cache section range should not overflow");
    bytes
        .get(start..end)
        .expect("test cache section should be inside encoded bytes")
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

fn render_cached_artifact(label: &str, reader: &PackageArtifactReader, dump: &mut String) {
    let def_map = reader
        .read_def_map()
        .expect("cached fixture DefMap should render");
    let semantic_ir = reader
        .read_semantic_ir()
        .expect("cached fixture Semantic IR should render");
    let body_manifest = reader
        .read_body_ir_manifest()
        .expect("cached fixture Body IR manifest should render");
    render_decoded_artifact(
        label,
        reader.probe(),
        &def_map,
        &semantic_ir,
        body_manifest.crates().len(),
        dump,
    );
}

fn render_decoded_artifact(
    label: &str,
    probe: &crate::cache::PackageCacheProbe,
    def_map: &PackageDefMaps,
    semantic_ir: &PackageIr,
    body_crate_count: usize,
    dump: &mut String,
) {
    let header = &probe.header;
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
    writeln!(dump, "header targets {}", header.package.targets.len())
        .expect("string writes should not fail");
    writeln!(dump, "parse files {}", probe.parse.files().len())
        .expect("string writes should not fail");
    writeln!(
        dump,
        "parse target roots {}",
        probe.parse.target_root_count()
    )
    .expect("string writes should not fail");
    writeln!(
        dump,
        "def-map package {} crates {}",
        def_map.package_name(),
        def_map.crates().len(),
    )
    .expect("string writes should not fail");
    writeln!(dump, "semantic IR crates {}", semantic_ir.crates().len(),)
        .expect("string writes should not fail");

    writeln!(dump, "body IR built crates {body_crate_count}")
        .expect("string writes should not fail");
}

fn render_body_ir_crate_statuses(reader: &PackageArtifactReader, dump: &mut String) {
    writeln!(dump, "body IR crate statuses").expect("string writes should not fail");
    for (crate_idx, &coverage) in reader.probe().body_ir_coverage.iter().enumerate() {
        writeln!(
            dump,
            "- crate {crate_idx} {} {}",
            coverage.status(),
            coverage,
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
