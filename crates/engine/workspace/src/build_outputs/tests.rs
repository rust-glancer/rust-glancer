use std::{
    any::type_name,
    fs::{self, File, FileTimes},
    mem,
    path::Path,
    sync::Arc,
    time::{Duration, UNIX_EPOCH},
};

use test_fixture::CrateFixture;

use rg_std::{MemoryRecordKind, MemoryRecorder, MemorySize};

use super::{CargoCompileEnvVar, CargoGeneratedSources, CargoGeneratedSourcesData};
use crate::{CargoMetadataConfig, WorkspaceLoweringConfig, WorkspaceMetadata};

#[test]
fn cloned_generated_sources_count_their_shared_payload_once() {
    let generated_sources = CargoGeneratedSources::new(
        "/target/build/unit/out".into(),
        vec![CargoCompileEnvVar {
            name: "OUT_DIR".to_string(),
            value: "/target/build/unit/out".to_string(),
        }],
        vec!["/target/build/unit/out/generated.rs".into()],
    );
    let cloned = generated_sources.clone();

    let mut one_handle = MemoryRecorder::new("one");
    generated_sources.record_memory_children(&mut one_handle);
    let mut both_handles = MemoryRecorder::total_only("both");
    generated_sources.record_memory_children(&mut both_handles);
    cloned.record_memory_children(&mut both_handles);

    assert_eq!(both_handles.total_bytes(), one_handle.total_bytes());
    assert!(one_handle.records().iter().any(|record| {
        record.kind == MemoryRecordKind::Heap
            && record.type_name == type_name::<CargoGeneratedSourcesData>()
            && record.bytes == mem::size_of::<CargoGeneratedSourcesData>()
    }));
    assert!(one_handle.records().iter().any(|record| {
        record.kind == MemoryRecordKind::Approximate
            && record.type_name == type_name::<Arc<CargoGeneratedSourcesData>>()
            && record.bytes > 0
    }));
}

#[test]
fn prefers_the_reported_build_directory_then_the_newest_unit() {
    let fixture = CrateFixture::from_fixture_spec(
        r#"
//- /Cargo.toml
[package]
name = "passive-artifact-fixture"
version = "0.1.0"
edition = "2024"
build = "build.rs"

//- /build.rs
fn main() {}

//- /src/lib.rs
include!(concat!(env!("OUT_DIR"), "/generated.rs"));
"#,
    );
    let mut metadata = fixture.metadata();
    let cargo_build_dir = metadata.target_directory.join("cargo-build");
    let target_dir = metadata.target_directory.as_std_path().to_path_buf();
    metadata.build_directory = Some(cargo_build_dir.clone());
    let cargo_build_dir = cargo_build_dir.as_std_path();
    let crate_root = fs::canonicalize(fixture.path("src/lib.rs"))
        .expect("fixture crate root should canonicalize");

    let older_out = cargo_build_dir.join("debug/build/passive-artifact-fixture-old/out");
    let selected_out = cargo_build_dir.join("debug/build/passive-artifact-fixture-selected/out");
    let fallback_out = target_dir.join("debug/build/passive-artifact-fixture-fallback/out");
    let deps_dir = cargo_build_dir.join("debug/deps");
    let fallback_deps_dir = target_dir.join("debug/deps");
    fs::create_dir_all(&older_out).expect("older output directory should be created");
    fs::create_dir_all(&selected_out).expect("selected output directory should be created");
    fs::create_dir_all(&fallback_out).expect("fallback output directory should be created");
    fs::create_dir_all(&deps_dir).expect("dep-info directory should be created");
    fs::create_dir_all(&fallback_deps_dir).expect("fallback dep-info directory should be created");

    let older_generated = older_out.join("generated.rs");
    let selected_generated = selected_out.join("generated.rs");
    let fallback_generated = fallback_out.join("generated.rs");
    let older_build_output = older_out
        .parent()
        .expect("older output directory should have a unit")
        .join("output");
    let selected_build_output = selected_out
        .parent()
        .expect("selected output directory should have a unit")
        .join("output");
    let fallback_build_output = fallback_out
        .parent()
        .expect("fallback output directory should have a unit")
        .join("output");
    fs::write(&older_generated, "pub struct Older;")
        .expect("older generated source should be written");
    fs::write(&selected_generated, "pub struct Selected;")
        .expect("selected generated source should be written");
    fs::write(&fallback_generated, "pub struct Fallback;")
        .expect("fallback generated source should be written");
    fs::write(
        &older_build_output,
        "cargo:rustc-env=GENERATED_NAME=older.rs\n",
    )
    .expect("older build output should be written");
    fs::write(
        &selected_build_output,
        "cargo::rustc-cfg=recovered_cfg\n\
         cargo:rustc-env=GENERATED_NAME=generated.rs\n",
    )
    .expect("selected build output should be written");
    fs::write(
        &fallback_build_output,
        "cargo:rustc-env=GENERATED_NAME=fallback.rs\n",
    )
    .expect("fallback build output should be written");

    let older_dep_info = deps_dir.join("passive_artifact_fixture-1111111111111111.d");
    let selected_dep_info = deps_dir.join("passive_artifact_fixture-2222222222222222.d");
    let fallback_dep_info = fallback_deps_dir.join("passive_artifact_fixture-3333333333333333.d");
    write_dep_info(&older_dep_info, &crate_root, &older_generated);
    write_dep_info(&selected_dep_info, &crate_root, &selected_generated);
    write_dep_info(&fallback_dep_info, &crate_root, &fallback_generated);

    // The build output itself is deliberately newer for the losing candidate. Selection follows
    // the crate compilation record, because Cargo can reuse old build-script output in a new unit.
    set_file_modified(&older_build_output, Duration::from_secs(30));
    set_file_modified(&selected_build_output, Duration::from_secs(10));
    set_file_modified(&older_dep_info, Duration::from_secs(20));
    set_file_modified(&selected_dep_info, Duration::from_secs(40));
    // A newer historical unit in the target-directory fallback must not override a usable unit
    // from Cargo's reported build directory.
    set_file_modified(&fallback_dep_info, Duration::from_secs(80));

    let workspace = WorkspaceMetadata::for_tests(metadata, WorkspaceLoweringConfig::default())
        .expect("workspace metadata should lower");
    let package = workspace
        .packages()
        .iter()
        .find(|package| package.name == "passive-artifact-fixture")
        .expect("fixture package should exist");
    let stats = workspace.cargo_build_output_stats();
    assert_eq!(stats.build_script_packages(), 1);
    assert_eq!(stats.selected_packages(), 1);
    assert_eq!(stats.generated_files(), 1);
    assert!(stats.generated_bytes() > 0);
    assert_eq!(stats.target_directories(), 2);
    assert_eq!(stats.matched_rustc_units(), 3);
    assert_eq!(stats.build_output_candidates(), 3);
    let generated_sources = package
        .cargo_generated_sources
        .as_ref()
        .expect("Cargo-generated sources should be selected");

    assert_eq!(
        generated_sources.out_dir(),
        fs::canonicalize(&selected_out).expect("selected output directory should canonicalize")
    );
    assert_eq!(
        generated_sources.compile_env_value("GENERATED_NAME"),
        Some("generated.rs")
    );
    assert_eq!(
        generated_sources.compile_env_value("OUT_DIR"),
        Some(generated_sources.out_dir().to_string_lossy().as_ref())
    );
    assert!(package.cfg_options.contains_atom("recovered_cfg"));
    assert_eq!(
        generated_sources.generated_files(),
        &[fs::canonicalize(selected_generated)
            .expect("selected generated source should canonicalize")]
    );
}

#[test]
fn combines_generated_files_from_package_targets_sharing_an_out_dir() {
    let fixture = CrateFixture::from_fixture_spec(
        r#"
//- /Cargo.toml
[package]
name = "multi-target-fixture"
version = "0.1.0"
edition = "2024"
build = "build.rs"

[[bin]]
name = "helper"
path = "src/main.rs"

//- /build.rs
fn main() {}

//- /src/lib.rs
include!(concat!(env!("OUT_DIR"), "/library.rs"));

//- /src/main.rs
include!(concat!(env!("OUT_DIR"), "/binary.rs"));
"#,
    );
    let metadata = fixture.metadata();
    let target_dir = metadata.target_directory.as_std_path();
    let out_dir = target_dir.join("debug/build/multi-target-fixture-unit/out");
    let deps_dir = target_dir.join("debug/deps");
    let library_generated = out_dir.join("library.rs");
    let binary_generated = out_dir.join("binary.rs");
    let library_dep_info = deps_dir.join("multi_target_fixture-1111111111111111.d");
    let binary_dep_info = deps_dir.join("helper-2222222222222222.d");
    fs::create_dir_all(&out_dir).expect("shared output directory should be created");
    fs::create_dir_all(&deps_dir).expect("dep-info directory should be created");
    fs::write(&library_generated, "pub struct FromLibrary;")
        .expect("library-generated source should be written");
    fs::write(&binary_generated, "pub struct FromBinary;")
        .expect("binary-generated source should be written");
    write_dep_info(
        &library_dep_info,
        &fs::canonicalize(fixture.path("src/lib.rs"))
            .expect("library target root should canonicalize"),
        &library_generated,
    );
    write_dep_info(
        &binary_dep_info,
        &fs::canonicalize(fixture.path("src/main.rs"))
            .expect("binary target root should canonicalize"),
        &binary_generated,
    );
    set_file_modified(&library_dep_info, Duration::from_secs(20));
    set_file_modified(&binary_dep_info, Duration::from_secs(40));

    let workspace = WorkspaceMetadata::for_tests(metadata, WorkspaceLoweringConfig::default())
        .expect("multi-target workspace metadata should lower");
    let package = workspace
        .packages()
        .iter()
        .find(|package| package.name == "multi-target-fixture")
        .expect("fixture package should exist");
    let generated_sources = package
        .cargo_generated_sources
        .as_ref()
        .expect("shared generated sources should be selected");
    let library_generated =
        fs::canonicalize(library_generated).expect("library-generated source should canonicalize");
    let binary_generated =
        fs::canonicalize(binary_generated).expect("binary-generated source should canonicalize");

    assert_eq!(workspace.cargo_build_output_stats().generated_files(), 2);
    assert_eq!(generated_sources.generated_files().len(), 2);
    assert!(
        generated_sources
            .generated_files()
            .contains(&library_generated)
    );
    assert!(
        generated_sources
            .generated_files()
            .contains(&binary_generated)
    );
}

#[test]
fn keeps_all_feature_cfgs_when_reusing_default_feature_artifacts() {
    let fixture = CrateFixture::from_fixture_spec(
        r#"
//- /Cargo.toml
[package]
name = "feature-mismatch-fixture"
version = "0.1.0"
edition = "2024"
build = "build.rs"

[features]
default = ["ordinary"]
ordinary = []
extra = []

//- /build.rs
fn main() {}

//- /src/lib.rs
include!(concat!(env!("OUT_DIR"), "/generated.rs"));
"#,
    );
    let loaded = CargoMetadataConfig::default()
        .all_features(true)
        .load_metadata_with_target_cfg(fixture.path("Cargo.toml"))
        .expect("all-features fixture metadata should load");
    let metadata = loaded.metadata;
    let build_directory = metadata
        .build_directory
        .as_ref()
        .unwrap_or(&metadata.target_directory)
        .as_std_path();
    let crate_root = fs::canonicalize(fixture.path("src/lib.rs"))
        .expect("fixture crate root should canonicalize");
    let out_dir = build_directory.join("debug/build/feature-mismatch-fixture-default/out");
    let generated = out_dir.join("generated.rs");
    let dep_info = build_directory.join("debug/deps/feature_mismatch_fixture-defa017.d");
    fs::create_dir_all(&out_dir).expect("generated output directory should be created");
    fs::create_dir_all(dep_info.parent().expect("dep-info should have a parent"))
        .expect("dep-info directory should be created");
    fs::write(&generated, "pub struct GeneratedForDefaultFeatures;")
        .expect("generated source should be written");
    fs::write(
        out_dir
            .parent()
            .expect("output directory should have a build unit")
            .join("output"),
        "cargo::rustc-cfg=artifact_from_default_features\n\
         cargo::rustc-env=ARTIFACT_FEATURE_SET=default\n",
    )
    .expect("default-feature build output should be written");
    write_dep_info(&dep_info, &crate_root, &generated);

    // This retained unit deliberately represents a default-feature build. Its dep-info proves
    // which package and generated source it belongs to, but Cargo metadata remains authoritative
    // for the feature set used by this analysis snapshot.
    let workspace = WorkspaceMetadata::lower(
        metadata,
        loaded.target_cfg,
        WorkspaceLoweringConfig::default(),
    )
    .expect("all-features workspace metadata should lower");
    let package = workspace
        .packages()
        .iter()
        .find(|package| package.name == "feature-mismatch-fixture")
        .expect("fixture package should exist");
    let generated_sources = package
        .cargo_generated_sources
        .as_ref()
        .expect("provenance-valid generated sources should be selected");

    assert!(
        package
            .cfg_options
            .contains_key_value("feature", "ordinary")
    );
    assert!(package.cfg_options.contains_key_value("feature", "extra"));
    assert!(
        package
            .cfg_options
            .contains_atom("artifact_from_default_features")
    );
    assert_eq!(
        generated_sources.compile_env_value("ARTIFACT_FEATURE_SET"),
        Some("default")
    );
    assert_eq!(
        generated_sources.generated_files(),
        &[fs::canonicalize(generated).expect("generated source should canonicalize")]
    );
}

#[test]
fn does_not_attribute_a_package_name_collision_without_the_target_root() {
    let fixture = CrateFixture::from_fixture_spec(
        r#"
//- /Cargo.toml
[package]
name = "same-name"
version = "0.1.0"
edition = "2024"
build = "build.rs"

//- /build.rs
fn main() {}

//- /src/lib.rs
pub struct RealPackage;
"#,
    );
    let metadata = fixture.metadata();
    let target_dir = metadata.target_directory.as_std_path();
    let generated = target_dir.join("debug/build/same-name-unit/out/generated.rs");
    let dep_info = target_dir.join("debug/deps/same_name-aaaaaaaaaaaaaaaa.d");
    fs::create_dir_all(
        generated
            .parent()
            .expect("generated file should have a parent"),
    )
    .expect("generated output directory should be created");
    fs::create_dir_all(dep_info.parent().expect("dep-info should have a parent"))
        .expect("dep-info directory should be created");
    fs::write(&generated, "pub struct WrongPackage;").expect("generated source should be written");
    write_dep_info(&dep_info, Path::new("/unrelated/src/lib.rs"), &generated);

    let workspace = WorkspaceMetadata::for_tests(metadata, WorkspaceLoweringConfig::default())
        .expect("workspace metadata should lower");
    let package = workspace
        .packages()
        .iter()
        .find(|package| package.name == "same-name")
        .expect("fixture package should exist");

    assert!(package.cargo_generated_sources.is_none());
}

#[test]
fn ignores_missing_generated_paths_and_oversized_dep_info() {
    let fixture = CrateFixture::from_fixture_spec(
        r#"
//- /Cargo.toml
[package]
name = "bounded-artifact-fixture"
version = "0.1.0"
edition = "2024"
build = "build.rs"

//- /build.rs
fn main() {}

//- /src/lib.rs
include!(concat!(env!("OUT_DIR"), "/generated.rs"));
"#,
    );
    let metadata = fixture.metadata();
    let target_dir = metadata.target_directory.as_std_path();
    let crate_root = fs::canonicalize(fixture.path("src/lib.rs"))
        .expect("bounded fixture crate root should canonicalize");
    let generated = target_dir.join("debug/build/bounded-artifact-fixture-unit/out/generated.rs");
    let dep_info = target_dir.join("debug/deps/bounded_artifact_fixture-deadbeef.d");
    fs::create_dir_all(dep_info.parent().expect("dep-info should have a parent"))
        .expect("dep-info directory should be created");
    write_dep_info(&dep_info, &crate_root, &generated);

    let workspace =
        WorkspaceMetadata::for_tests(metadata.clone(), WorkspaceLoweringConfig::default())
            .expect("workspace metadata with a missing generated path should lower");
    assert!(
        workspace
            .packages()
            .iter()
            .all(|package| package.cargo_generated_sources.is_none())
    );

    fs::create_dir_all(
        generated
            .parent()
            .expect("generated path should have a parent"),
    )
    .expect("generated output directory should be created");
    fs::write(&generated, "pub struct Generated;").expect("generated source should be written");
    File::options()
        .write(true)
        .truncate(true)
        .open(&dep_info)
        .expect("dep-info should open for truncation")
        .set_len(8 * 1024 * 1024 + 1)
        .expect("oversized dep-info should be created sparsely");

    let workspace = WorkspaceMetadata::for_tests(metadata, WorkspaceLoweringConfig::default())
        .expect("workspace metadata with oversized dep-info should lower");
    assert!(
        workspace
            .packages()
            .iter()
            .all(|package| package.cargo_generated_sources.is_none())
    );
}

fn write_dep_info(path: &Path, crate_root: &Path, generated: &Path) {
    fs::write(
        path,
        format!(
            "{}: {} {}\n",
            path.display(),
            crate_root.display(),
            generated.display()
        ),
    )
    .expect("dep-info should be written");
}

fn set_file_modified(path: &Path, since_epoch: Duration) {
    // Windows requires write access on the file handle before it will update timestamps.
    File::options()
        .write(true)
        .open(path)
        .expect("timestamp target should open for writing")
        .set_times(FileTimes::new().set_modified(UNIX_EPOCH + since_epoch))
        .expect("timestamp should be set");
}
