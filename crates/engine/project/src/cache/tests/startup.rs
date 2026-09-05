use std::{fmt::Write as _, fs};

use expect_test::expect;
use rg_body_ir::BodyIrBuildPolicy;

use super::utils::{package_cache_header_for, write_cached_package_artifact};
use crate::cache::WorkspaceCachePlan;
use crate::profile::metric;
use crate::{
    PackageResidencyPolicy, Project,
    testonly::{ProjectFixture, ProjectSourceFixture},
};

#[test]
fn residency_policy_changes_rebuild_from_source() {
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
        profile_counter(&transition_profile, metric::CACHE_PROBE_MISSING_ARTIFACTS),
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
        profile_counter(&switch_back_profile, metric::CACHE_PROBE_MISSING_ARTIFACTS),
    )
    .expect("string writes should not fail");

    expect![[r#"
        residency policy cache invalidation
        first generation existed true
        generation changed true
        transition hits 0
        transition missing artifacts 1
        old generation after transition false
        transition artifact true
        switch-back hits 0
        switch-back missing artifacts 2
    "#]]
    .assert_eq(&format!("{}\n", dump.trim_end()));
}

#[test]
fn lazy_loads_offloaded_packages_for_queries() {
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

[[bin]]
name = "dep-tool"
path = "src/bin/dep_tool.rs"

//- /dep/src/lib.rs
pub struct DepType;

unsafe extern "C" {
    pub fn dep_foreign(value: u32) -> u32;
    pub static DEP_STATIC: u32;
    pub type DepOpaque;
}

//- /dep/src/bin/dep_tool.rs
fn main() {}
"#,
    );
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

    expect![[r#"
        offloaded dependency query
        dep resident false
        symbols
        - fn dep_foreign @ dep[lib]
        - static DEP_STATIC @ dep[lib]
        - struct DepType @ dep[lib]
        - type_alias DepOpaque @ dep[lib]
    "#]]
    .assert_eq(&format!("{}\n", dump.trim_end()));
}

#[test]
fn startup_indexing_rejects_payload_with_forged_source_fingerprint() {
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

    expect![[r#"
        startup artifact-backed indexing
        dep resident false
        old symbols 0
        new symbols 1
    "#]]
    .assert_eq(&format!("{}\n", dump.trim_end()));
}

#[test]
fn startup_cache_profile_reports_probe_hits() {
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

    expect![[r#"
        startup cache probe profile
        packages 2
        resident 1
        offloadable 1
        hits 1
        misses 0
    "#]]
    .assert_eq(&format!("{}\n", dump.trim_end()));
}

#[test]
fn startup_cache_misses_rebuild_reverse_dependents() {
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

    expect![[r#"
        dependency cache miss closure
        hits 1
        direct restore misses 1
        reverse-dependent misses 2
        definitions
        - dep struct Kept
    "#]]
    .assert_eq(&format!("{}\n", dump.trim_end()));
}

#[test]
fn missing_dependency_artifact_rebuilds_reverse_dependents() {
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

    expect![[r#"
        missing dependency artifact closure
        missing artifacts 1
        reverse-dependent misses 1
        hits 0
        dependency symbols 1
    "#]]
    .assert_eq(&format!("{}\n", dump.trim_end()));
}

#[test]
fn startup_discards_incomplete_cache_updates() {
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

    expect![[r#"
        incomplete cache update recovery
        recovery hits 0
        recovery missing artifacts 2
        next startup hits 2
    "#]]
    .assert_eq(&format!("{}\n", dump.trim_end()));
}

#[test]
fn startup_indexing_rejects_artifacts_when_body_ir_policy_needs_more_bodies() {
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
    writeln!(&mut dump, "body IR crate statuses").expect("string writes should not fail");
    for (crate_idx, &coverage) in reader.probe().body_ir_coverage.iter().enumerate() {
        writeln!(
            &mut dump,
            "- crate {crate_idx} {} {}",
            coverage.status(),
            coverage,
        )
        .expect("string writes should not fail");
    }

    expect![[r#"
        startup body IR policy mismatch
        hits 0
        misses 1
        body policy mismatches 1
        body IR crate statuses
        - crate 0 built complete
    "#]]
    .assert_eq(&format!("{}\n", dump.trim_end()));
}

#[test]
fn startup_indexing_rejects_artifacts_when_out_of_line_modules_change() {
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

    expect![[r#"
        startup stale out-of-line module
        dep resident false
        old symbols 0
        new symbols 1
    "#]]
    .assert_eq(&format!("{}\n", dump.trim_end()));
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
