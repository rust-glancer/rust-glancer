use std::{fs, path::Path, process::Command};

use cargo_metadata::MetadataCommand;
use rg_workspace::{SysrootSources, WorkspaceLoweringConfig, WorkspaceMetadata};
use test_fixture::CrateFixture;

use crate::{Project, SavedFileChange, testonly::ProjectSourceFixture};

#[test]
fn indexes_generated_sources_from_a_cargo_check_with_an_external_build_directory() {
    let fixture = ProjectSourceFixture::build_with_fake_sysroot(
        r#"
//- /Cargo.toml
[workspace]
members = ["crates/demo"]
resolver = "3"

//- /crates/demo/Cargo.toml
[package]
name = "generated-demo"
version = "0.1.0"
edition = "2024"

[lib]
name = "demo_api"

//- /crates/demo/build.rs
use std::{env, fs, path::PathBuf};

fn main() {
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo sets OUT_DIR"));
    fs::write(out_dir.join("generated.rs"), "pub struct CargoGenerated;")
        .expect("Cargo creates OUT_DIR");
    println!("cargo::rustc-check-cfg=cfg(has_generated)");
    println!("cargo::rustc-cfg=has_generated");
    println!("cargo::rustc-env=GENERATED_FILE=generated.rs");
}

//- /crates/demo/src/lib.rs
#[cfg(has_generated)]
include!(concat!(env!("OUT_DIR"), "/", env!("GENERATED_FILE")));
"#,
    );
    let root = fixture.path("");
    let external_build = CrateFixture::from_files([]);
    let build_dir = external_build.path("intermediates");
    let target_dir = fixture.path("artifacts");
    let cargo = env!("CARGO");
    let checked = Command::new(cargo)
        .current_dir(&root)
        .env("CARGO_BUILD_BUILD_DIR", &build_dir)
        .env("CARGO_TARGET_DIR", &target_dir)
        .env_remove("__CARGO_TEMPORARY_BUILD_DIR_NEW_LAYOUT_OPT_OUT")
        .args(["check", "--offline"])
        .output()
        .expect("fixture cargo check should start");
    assert!(
        checked.status.success(),
        "fixture cargo check failed: {}",
        String::from_utf8_lossy(&checked.stderr),
    );

    let metadata = MetadataCommand::new()
        .cargo_path(cargo)
        .current_dir(root)
        .env("CARGO_BUILD_BUILD_DIR", build_dir.as_os_str())
        .env("CARGO_TARGET_DIR", target_dir.as_os_str())
        .exec()
        .expect("fixture Cargo metadata should load");
    let sysroot = SysrootSources::from_library_root(fixture.path("sysroot/library"))
        .expect("fixture sysroot should be complete");

    // Older Cargo metadata reports the directory with intermediate artifacts as target_directory.
    let mut metadata_without_build_dir = metadata.clone();
    metadata_without_build_dir.target_directory = metadata_without_build_dir
        .build_directory
        .take()
        .expect("Cargo should report the external build directory");

    for metadata in [metadata, metadata_without_build_dir] {
        let workspace = WorkspaceMetadata::for_tests(metadata, WorkspaceLoweringConfig::default())
            .expect("fixture metadata should lower")
            .with_sysroot_sources(Some(sysroot.clone()));
        let project = Project::builder(workspace)
            .build()
            .expect("fixture project should build");
        assert_symbol_count(&project, "CargoGenerated", 1);
    }
}

#[test]
fn passively_recovered_out_dir_sources_enter_semantic_analysis() {
    let fixture = ProjectSourceFixture::build_with_fake_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "passive_generated_fixture"
version = "0.1.0"
edition = "2024"
build = "build.rs"

//- /build.rs
fn main() {}

//- /src/lib.rs
#[cfg(recovered_cfg)]
include!(concat!(env!("OUT_DIR"), concat!("/", env!("GENERATED_FILE"))));

macro_rules! include_macro_generated {
    () => {
        include!(concat!(env!("OUT_DIR"), "/macro_generated.rs"));
    };
}

include_macro_generated!();

pub fn edited_later() {}

//- /src/generated_child.rs
pub struct GeneratedChild;
"#,
    );
    let generated =
        fixture.path("target/debug/build/passive_generated_fixture-unit/out/generated.rs");
    let nested =
        fixture.path("target/debug/build/passive_generated_fixture-unit/out/nested_generated.rs");
    let macro_generated =
        fixture.path("target/debug/build/passive_generated_fixture-unit/out/macro_generated.rs");
    write_cargo_build_output_fixture(
        &fixture,
        &generated,
        &[&generated, &nested, &macro_generated],
        "cargo::rustc-cfg=recovered_cfg\n\
         cargo:rustc-env=GENERATED_FILE=generated.rs\n",
    );
    fs::write(
        &generated,
        "pub struct RecoveredGenerated;\n\
         include!(\"nested_generated.rs\");\n\
         pub mod generated_child;\n",
    )
    .expect("generated fixture source should be written");
    fs::write(&nested, "pub struct NestedRecovered;")
        .expect("nested generated fixture source should be written");
    fs::write(&macro_generated, "pub struct MacroRecovered;")
        .expect("macro-generated include fixture source should be written");

    let workspace = fixture.workspace_metadata();
    let workspace_package = workspace
        .packages()
        .iter()
        .find(|package| package.name == "passive_generated_fixture")
        .expect("passive generated fixture package should exist");
    assert!(workspace_package.cfg_options.contains_atom("recovered_cfg"));
    assert!(workspace_package.cargo_generated_sources.is_some());

    let mut project = Project::builder(workspace)
        .build()
        .expect("passive generated-source fixture should build");
    project.inspect(|snapshot| {
        assert!(
            snapshot
                .file_context_for_source_path(
                    &fs::canonicalize(&generated).expect("generated source path")
                )
                .expect("generated source routing")
                .is_some()
        )
    });
    assert_symbol_count(&project, "RecoveredGenerated", 1);
    assert_symbol_count(&project, "NestedRecovered", 1);
    assert_symbol_count(&project, "MacroRecovered", 1);
    assert_symbol_count(&project, "GeneratedChild", 1);
    project.inspect(|snapshot| {
        let generated_symbol = snapshot
            .analysis()
            .workspace_symbols("RecoveredGenerated")
            .expect("generated fixture symbol query should resolve")
            .into_iter()
            .next()
            .expect("recovered generated symbol should exist");
        assert_eq!(
            snapshot
                .sources()
                .path(generated_symbol.file_id)
                .expect("source path"),
            fs::canonicalize(&generated)
                .expect("generated source should canonicalize")
                .as_path()
        );
    });

    // An ordinary source save rebuilds from retained Cargo-generated sources. It must not require a
    // Cargo rescan to rediscover the same output directory.
    fs::write(
        fixture.path("src/lib.rs"),
        "#[cfg(recovered_cfg)]\n\
         include!(concat!(env!(\"OUT_DIR\"), concat!(\"/\", env!(\"GENERATED_FILE\"))));\n\
         macro_rules! include_macro_generated {\n\
             () => { include!(concat!(env!(\"OUT_DIR\"), \"/macro_generated.rs\")); };\n\
         }\n\
         include_macro_generated!();\n\
         pub fn edited_later(_: RecoveredGenerated) {}\n",
    )
    .expect("ordinary fixture source should be updated");
    project
        .apply_changes([SavedFileChange::fs_path(fixture.path("src/lib.rs"))])
        .expect("ordinary save should preserve passive generated sources");
    assert_symbol_count(&project, "RecoveredGenerated", 1);
    assert_symbol_count(&project, "MacroRecovered", 1);

    // Generated paths live outside the package root but become ordinary explicit package sources.
    // A direct save notification must therefore rebuild the same owner and replace its symbols.
    fs::write(&generated, "pub struct RefreshedGenerated;")
        .expect("generated fixture source should be refreshed");
    project
        .apply_changes([SavedFileChange::fs_path(&generated)])
        .expect("generated source save should rebuild its explicit owner");
    assert_symbol_count(&project, "RecoveredGenerated", 0);
    assert_symbol_count(&project, "RefreshedGenerated", 1);

    drop(project);
    let warm = Project::builder(fixture.workspace_metadata())
        .build()
        .expect("passive generated-source cache should restore");
    assert_symbol_count(&warm, "RefreshedGenerated", 1);
    assert_symbol_count(&warm, "MacroRecovered", 1);

    // Cache identity permits historical build snapshots, but source validation still requires
    // every parsed generated file to exist with the exact cached contents.
    drop(warm);
    for path in [&generated, &nested, &macro_generated] {
        fs::remove_file(path).expect("simulated cargo clean should remove generated source");
    }
    fs::remove_file(fixture.path("target/debug/deps/passive_generated_fixture-deadbeef.d"))
        .expect("simulated cargo clean should remove dep-info evidence");
    let source_only_workspace = fixture.workspace_metadata();
    assert!(
        source_only_workspace
            .packages()
            .iter()
            .all(|package| package.cargo_generated_sources.is_none())
    );
    let source_only = Project::builder(source_only_workspace)
        .build()
        .expect("missing passive artifacts should fall back to source-only analysis");
    assert_symbol_count(&source_only, "RefreshedGenerated", 0);
    assert_symbol_count(&source_only, "MacroRecovered", 0);
}

#[test]
fn warm_start_uses_newly_selected_build_output() {
    let fixture = ProjectSourceFixture::build_with_fake_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "passive_generated_fixture"
version = "0.1.0"
edition = "2024"
build = "build.rs"

//- /build.rs
fn main() {}

//- /src/lib.rs
include!(concat!(env!("OUT_DIR"), "/generated.rs"));
"#,
    );
    let historical =
        fixture.path("target/debug/build/passive_generated_fixture-historical/out/generated.rs");
    write_cargo_build_output_fixture(&fixture, &historical, &[&historical], "");
    fs::write(&historical, "pub struct HistoricalGenerated;")
        .expect("historical generated source should be written");
    let historical_project = Project::builder(fixture.workspace_metadata())
        .build()
        .expect("historical generated-source project should build");
    assert_symbol_count(&historical_project, "HistoricalGenerated", 1);
    drop(historical_project);

    // Cargo has since selected another unit, but the old generated file still describes the exact
    // source snapshot stored in the package cache.
    let selected =
        fixture.path("target/debug/build/passive_generated_fixture-selected/out/generated.rs");
    write_cargo_build_output_fixture(&fixture, &selected, &[&selected], "");
    fs::write(&selected, "pub struct NewlySelectedGenerated;")
        .expect("newly selected generated source should be written");
    let current_workspace = fixture.workspace_metadata();
    let current_sources = current_workspace
        .packages()
        .iter()
        .find(|package| package.name == "passive_generated_fixture")
        .and_then(|package| package.cargo_generated_sources.as_ref())
        .expect("current Cargo-generated sources should be discovered");
    assert_eq!(
        current_sources.out_dir(),
        fs::canonicalize(
            selected
                .parent()
                .expect("selected generated source should have an output directory"),
        )
        .expect("selected output directory should canonicalize")
    );

    let restored = Project::builder(current_workspace)
        .build()
        .expect("warm startup should index the current build output");
    assert_symbol_count(&restored, "HistoricalGenerated", 0);
    assert_symbol_count(&restored, "NewlySelectedGenerated", 1);
}

#[test]
fn manual_workspace_reindex_rescans_passive_cargo_build_outputs() {
    let fixture = ProjectSourceFixture::build_with_fake_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "passive_generated_fixture"
version = "0.1.0"
edition = "2024"
build = "build.rs"

//- /build.rs
fn main() {}

//- /src/lib.rs
include!(concat!(env!("OUT_DIR"), "/generated.rs"));
"#,
    );
    let generated =
        fixture.path("target/debug/build/passive_generated_fixture-unit/out/generated.rs");
    let mut project = fixture.build_project();
    assert!(
        project
            .workspace()
            .packages()
            .iter()
            .all(|package| package.cargo_generated_sources.is_none())
    );

    write_cargo_build_output_fixture(&fixture, &generated, &[&generated], "");
    fs::write(&generated, "pub struct DiscoveredAfterStartup;")
        .expect("post-startup generated source should be written");
    project
        .reindex_workspace()
        .expect("manual reindex should rescan passive Cargo build outputs");

    let generated =
        fs::canonicalize(generated).expect("post-startup generated source should canonicalize");
    assert!(
        project
            .workspace()
            .packages()
            .iter()
            .any(|package| package.cargo_generated_sources.is_some())
    );
    assert!(project.inspect(|snapshot| {
        snapshot
            .file_context_for_source_path(&generated)
            .expect("source routing")
            .is_some()
    }));
    assert_symbol_count(&project, "DiscoveredAfterStartup", 1);
}

#[test]
fn macro_generated_include_reuses_one_file_under_distinct_module_contexts() {
    let fixture = ProjectSourceFixture::build_with_fake_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "passive_generated_fixture"
version = "0.1.0"
edition = "2024"
build = "build.rs"

//- /build.rs
fn main() {}

//- /src/lib.rs
macro_rules! include_context {
    () => { include!(concat!(env!("OUT_DIR"), "/context.rs")); };
}

pub mod left {
    include_context!();
}

pub mod right {
    include_context!();
}

//- /src/left/child.rs
pub struct LeftGeneratedChild;

//- /src/right/child.rs
pub struct RightGeneratedChild;
"#,
    );
    let generated =
        fixture.path("target/debug/build/passive_generated_fixture-unit/out/context.rs");
    write_cargo_build_output_fixture(&fixture, &generated, &[&generated], "");
    fs::write(&generated, "pub mod child;")
        .expect("shared generated include source should be written");

    let project = fixture.build_project();
    assert_symbol_count(&project, "LeftGeneratedChild", 1);
    assert_symbol_count(&project, "RightGeneratedChild", 1);
}

fn write_cargo_build_output_fixture(
    fixture: &ProjectSourceFixture,
    generated: &Path,
    loaded_sources: &[&Path],
    build_output: &str,
) {
    let out_dir = generated
        .parent()
        .expect("generated fixture source should have an output directory");
    let unit_dir = out_dir
        .parent()
        .expect("generated fixture output should have a build unit directory");
    let dep_info = fixture.path("target/debug/deps/passive_generated_fixture-deadbeef.d");
    fs::create_dir_all(out_dir).expect("generated fixture output directory should be created");
    fs::create_dir_all(
        dep_info
            .parent()
            .expect("generated fixture dep-info should have a parent"),
    )
    .expect("generated fixture dep-info directory should be created");
    fs::write(unit_dir.join("output"), build_output)
        .expect("generated fixture build output should be written");

    let crate_root = fs::canonicalize(fixture.path("src/lib.rs"))
        .expect("generated fixture crate root should canonicalize");
    let mut rule = format!("{}: {}", dep_info.display(), crate_root.display());
    for source in loaded_sources {
        rule.push(' ');
        rule.push_str(&source.display().to_string());
    }
    rule.push('\n');
    fs::write(dep_info, rule).expect("generated fixture dep-info should be written");
}

fn assert_symbol_count(project: &Project, name: &str, expected: usize) {
    let symbols = project.inspect(|snapshot| {
        snapshot
            .analysis()
            .workspace_symbols(name)
            .expect("generated fixture symbol query should resolve")
    });
    assert_eq!(
        symbols.len(),
        expected,
        "workspace symbol count for {name} should match",
    );
}
