use std::{
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

use expect_test::Expect;
use rg_body_ir::{BodyIrPackageBundle, PackageBodies};
use rg_def_map::{DefMapPackageBundle, Package, PackageSlot};
use rg_semantic_ir::{PackageIr, SemanticIrPackageBundle};
use rg_workspace::WorkspaceMetadata;
use test_fixture::fixture_crate;

use crate::cache::{
    CachedDependency, CachedPackage, CachedPackageId, CachedPackageSlot, CachedPackageSource,
    CachedPath, CachedRustEdition, CachedTarget, CachedTargetKind, PackageCacheArtifact,
    PackageCacheBodyIrState, PackageCacheCodec, PackageCacheHeader, PackageCachePayload,
    PackageCacheStore, WorkspaceCachePlan,
};
use crate::{PackageResidencyPolicy, Project, project::state::ProjectState};

pub(super) fn check_cache_plan(fixture: &str, expect: Expect) {
    let fixture = fixture_crate(fixture);
    let workspace = WorkspaceMetadata::from_cargo(fixture.metadata())
        .expect("fixture workspace metadata should normalize");
    let cache_plan = WorkspaceCachePlan::build(&workspace);
    let actual = render_cache_plan(&workspace, &cache_plan);

    expect.assert_eq(&format!("{}\n", actual.trim_end()));
}

pub(super) fn check_cache_store_paths(fixture: &str, expect: Expect) {
    let fixture = fixture_crate(fixture);
    let workspace = WorkspaceMetadata::from_cargo(fixture.metadata())
        .expect("fixture workspace metadata should normalize");
    let cache_plan = WorkspaceCachePlan::build(&workspace);

    let mut dump = String::new();
    render_cache_store(
        "workspace target",
        &workspace,
        &cache_plan,
        &PackageCacheStore::for_workspace_with_target_dir(
            &workspace,
            &cache_plan,
            workspace.workspace_root().join("target"),
        ),
        &mut dump,
    );
    writeln!(&mut dump).expect("string writes should not fail");
    render_cache_store(
        "custom target",
        &workspace,
        &cache_plan,
        &PackageCacheStore::for_workspace_with_target_dir(
            &workspace,
            &cache_plan,
            PathBuf::from("custom-target"),
        ),
        &mut dump,
    );

    expect.assert_eq(&format!("{}\n", dump.trim_end()));
}

pub(super) fn check_cache_header_codec(expect: Expect) {
    let header = PackageCacheHeader::new(CachedPackage {
        package: CachedPackageSlot(7),
        package_id: CachedPackageId::from_stable_text("path+file:///workspace#app@0.1.0"),
        name: "app".to_string(),
        source: CachedPackageSource::Workspace,
        edition: CachedRustEdition::Edition2024,
        manifest_path: CachedPath::from_stable_text("/workspace/Cargo.toml"),
        targets: vec![
            CachedTarget {
                name: "app".to_string(),
                kind: CachedTargetKind::Lib,
                src_path: CachedPath::from_stable_text("/workspace/src/lib.rs"),
            },
            CachedTarget {
                name: "app-cli".to_string(),
                kind: CachedTargetKind::Bin,
                src_path: CachedPath::from_stable_text("/workspace/src/main.rs"),
            },
        ],
        dependencies: vec![CachedDependency {
            package_id: CachedPackageId::from_stable_text("path+file:///workspace/dep#dep@0.1.0"),
            name: "dep".to_string(),
            is_normal: true,
            is_build: false,
            is_dev: false,
        }],
    });

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
        PackageCacheHeader::new(CachedPackage {
            package: CachedPackageSlot(7),
            package_id: CachedPackageId::from_stable_text("path+file:///workspace#empty@0.1.0"),
            name: String::new(),
            source: CachedPackageSource::Workspace,
            edition: CachedRustEdition::Edition2024,
            manifest_path: CachedPath::from_stable_text("/workspace/Cargo.toml"),
            targets: Vec::new(),
            dependencies: Vec::new(),
        }),
        PackageCachePayload::new(
            DefMapPackageBundle::new(Package::default()),
            SemanticIrPackageBundle::new(PackageIr::default()),
            PackageCacheBodyIrState::Built(Box::new(BodyIrPackageBundle::new(
                PackageBodies::default(),
            ))),
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
    let fixture = fixture_crate(fixture);
    let workspace = WorkspaceMetadata::from_cargo(fixture.metadata())
        .expect("fixture workspace metadata should normalize");
    let project = Project::builder(workspace)
        .build()
        .expect("fixture project should build")
        .into_project();
    let artifact =
        package_artifact_from_project(&project.state.cache_plan, &project.state, PackageSlot(0));

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
    let fixture = fixture_crate(fixture);
    let workspace = WorkspaceMetadata::from_cargo(fixture.metadata())
        .expect("fixture workspace metadata should normalize");
    let project = Project::builder(workspace)
        .build()
        .expect("fixture project should build")
        .into_project();
    let artifact =
        package_artifact_from_project(&project.state.cache_plan, &project.state, PackageSlot(0));
    let store = PackageCacheStore::for_workspace_with_target_dir(
        project.workspace(),
        &project.state.cache_plan,
        project.workspace().workspace_root().join("target"),
    );
    let path = store.package_artifact_path(&artifact.header.package);

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
        .invalidate_workspace_cache()
        .expect("workspace cache namespace should be removable");
    let missing_after_invalidation = store
        .read_artifact(&artifact.header)
        .expect("missing package cache artifact should not fail after invalidation")
        .is_none();

    let mut dump = String::new();
    writeln!(&mut dump, "cache store artifact I/O").expect("string writes should not fail");
    writeln!(&mut dump, "missing before write {missing_before_write}")
        .expect("string writes should not fail");
    writeln!(
        &mut dump,
        "artifact path {}",
        cache_path(project.workspace(), path),
    )
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

pub(super) fn check_cache_store_generation_cleanup(fixture: &str, expect: Expect) {
    let fixture = fixture_crate(fixture);
    let workspace = WorkspaceMetadata::from_cargo(fixture.metadata())
        .expect("fixture workspace metadata should normalize");
    let project = Project::builder(workspace)
        .build()
        .expect("fixture project should build")
        .into_project();
    let artifact =
        package_artifact_from_project(&project.state.cache_plan, &project.state, PackageSlot(0));
    let store = PackageCacheStore::for_workspace_with_target_dir(
        project.workspace(),
        &project.state.cache_plan,
        project.workspace().workspace_root().join("target"),
    );
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

pub(super) fn check_offloaded_dependency_query(fixture: &str, expect: Expect) {
    let fixture = fixture_crate(fixture);
    let workspace = WorkspaceMetadata::from_cargo(fixture.metadata())
        .expect("fixture workspace metadata should normalize");
    let project = Project::builder(workspace)
        .package_residency_policy(PackageResidencyPolicy::WorkspaceResident)
        .build()
        .expect("fixture project should build")
        .into_project();
    let dep = package_slot_by_name(project.snapshot().parse_db(), "dep");
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

fn package_artifact_from_project(
    cache_plan: &WorkspaceCachePlan,
    project: &ProjectState,
    package: PackageSlot,
) -> PackageCacheArtifact {
    let header = cache_plan
        .artifact_header(package)
        .expect("cache-planned fixture package should have an artifact header");
    let def_map = project
        .def_map
        .resident_package(package)
        .expect("fixture package should have def-map data")
        .clone();
    let semantic_ir = project
        .semantic_ir
        .resident_package(package)
        .expect("fixture package should have semantic IR data")
        .clone();
    let body_ir = project
        .body_ir
        .resident_package(package)
        .expect("fixture package should have body IR data")
        .clone();

    PackageCacheArtifact::new(
        header,
        PackageCachePayload::new(
            DefMapPackageBundle::new(def_map),
            SemanticIrPackageBundle::new(semantic_ir),
            PackageCacheBodyIrState::Built(Box::new(BodyIrPackageBundle::new(body_ir))),
        ),
    )
}

fn package_slot_by_name(parse: &rg_parse::ParseDb, package_name: &str) -> PackageSlot {
    parse
        .packages()
        .iter()
        .enumerate()
        .find_map(|(idx, package)| {
            (package.package_name() == package_name).then_some(PackageSlot(idx))
        })
        .unwrap_or_else(|| panic!("fixture package {package_name} should be parsed"))
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

fn render_cache_store(
    label: &str,
    workspace: &WorkspaceMetadata,
    cache_plan: &WorkspaceCachePlan,
    store: &PackageCacheStore,
    dump: &mut String,
) {
    writeln!(dump, "cache store `{label}`").expect("string writes should not fail");
    writeln!(
        dump,
        "root {}",
        cache_path(workspace, store.root().to_path_buf()),
    )
    .expect("string writes should not fail");
    writeln!(dump, "artifacts").expect("string writes should not fail");

    for package in cache_plan.packages() {
        writeln!(
            dump,
            "- #{} {} {}",
            package.package.0,
            package.name,
            store.package_fingerprint(package),
        )
        .expect("string writes should not fail");
        writeln!(
            dump,
            "  {}",
            cache_path(workspace, store.package_artifact_path(package)),
        )
        .expect("string writes should not fail");
    }
}

fn render_package(
    workspace: &WorkspaceMetadata,
    cache_plan: &WorkspaceCachePlan,
    package: &CachedPackage,
    dump: &mut String,
) {
    // The header is rendered together with the cached package because artifact metadata is the
    // unit future cache readers will validate before loading any package payload.
    let header = cache_plan
        .artifact_header(
            package
                .package
                .workspace_slot()
                .expect("cached package slots should fit into workspace slots"),
        )
        .expect("cached package should have an artifact header");

    writeln!(dump, "package #{} {}", package.package.0, package.name)
        .expect("string writes should not fail");
    writeln!(dump, "schema {}", header.schema_version.0).expect("string writes should not fail");
    writeln!(
        dump,
        "id {}",
        normalize_package_id(workspace.workspace_root(), package.package_id.as_str()),
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
    writeln!(
        dump,
        "def-map package {} targets {}",
        artifact.payload.def_map.package().package_name(),
        artifact.payload.def_map.package().targets().len(),
    )
    .expect("string writes should not fail");
    writeln!(
        dump,
        "semantic IR targets {}",
        artifact.payload.semantic_ir.package().targets().len(),
    )
    .expect("string writes should not fail");

    match &artifact.payload.body_ir {
        PackageCacheBodyIrState::Built(bundle) => {
            writeln!(
                dump,
                "body IR built targets {}",
                bundle.package().targets().len()
            )
            .expect("string writes should not fail");
        }
        PackageCacheBodyIrState::SkippedByPolicy => {
            writeln!(dump, "body IR skipped by policy").expect("string writes should not fail");
        }
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
        .unwrap_or_else(|| normalize_package_id(workspace.workspace_root(), package_id.as_str()))
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

fn cache_path(workspace: &WorkspaceMetadata, path: PathBuf) -> String {
    let path = relative_path(workspace.workspace_root(), &path);
    let workspace_name = workspace
        .workspace_root()
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_else(|| "workspace".into());

    path.replace(workspace_name.as_ref(), "<workspace>")
}

fn render_hex(bytes: &[u8], dump: &mut String) {
    for chunk in bytes.chunks(32) {
        for byte in chunk {
            write!(dump, "{byte:02x}").expect("string writes should not fail");
        }
        writeln!(dump).expect("string writes should not fail");
    }
}
