//! Split-indexing behavior for packages with multiple Cargo targets.

use std::fmt::Write as _;

use crate::{
    AnalysisSurface, PackageResidencyPolicy, Project, SavedFileChange, SplitIndexingMode,
    profile::metric,
    testonly::{ProjectFixture, ProjectSourceFixture},
};

const TARGET_FANOUT_FIXTURE: &str = r#"
//- /Cargo.toml
[package]
name = "target_fanout_fixture"
version = "0.1.0"
edition = "2024"

[lib]
path = "src/lib.rs"

[[test]]
name = "first-test"
path = "tests/first.rs"

[[test]]
name = "second-test"
path = "tests/second.rs"

//- /src/lib.rs
pub fn library_value() -> usize { 1 }

//- /tests/first.rs
#[test]
fn first() { assert_eq!(target_fanout_fixture::library_value(), 1); }

//- /tests/second.rs
#[test]
fn second() { assert_eq!(target_fanout_fixture::library_value(), 1); }
"#;

#[test]
fn secondary_targets_stay_deferred_and_materialize_one_exact_crate() {
    let fixture = ProjectSourceFixture::build(TARGET_FANOUT_FIXTURE);
    let mut project = Project::builder(fixture.workspace_metadata())
        .split_indexing_mode(SplitIndexingMode::EarlyStart)
        .build()
        .expect("multi-target early-start project should build");

    let target_for_path = |project: &Project, path: &str| {
        project
            .snapshot()
            .file_contexts_for_path(fixture.path(path))
            .expect("target source path should resolve")
            .into_iter()
            .flat_map(|context| context.crates)
            .find(|crate_ref| {
                let package = project
                    .state
                    .def_map
                    .resident_package(crate_ref.package)
                    .expect("workspace def map should stay resident");
                let crate_data = package
                    .crate_data(crate_ref.crate_id)
                    .expect("target crate should have definition data");
                project.state.parse_db().packages()[crate_ref.package.0]
                    .target(crate_data.cargo_target())
                    .is_some_and(|target| target.src_path.ends_with(path))
            })
            .expect("source path should identify one semantic target")
    };
    let library = target_for_path(&project, "src/lib.rs");
    let first_test = target_for_path(&project, "tests/first.rs");
    let second_test = target_for_path(&project, "tests/second.rs");
    project
        .split_indexing()
        .finish()
        .expect("configured eager targets should finish");

    let coverage = |project: &Project, crate_ref: rg_ir_model::CrateRef| {
        project
            .state
            .body_ir
            .resident_package(crate_ref.package)
            .expect("workspace Body IR should stay resident")
            .crate_bodies(crate_ref.crate_id)
            .expect("semantic target should have a Body IR slot")
            .coverage()
    };
    assert_eq!(
        coverage(&project, library),
        rg_body_ir::CrateBodiesCoverage::Complete,
    );
    assert_eq!(
        coverage(&project, first_test),
        rg_body_ir::CrateBodiesCoverage::SkippedByPolicy,
    );
    assert_eq!(
        coverage(&project, second_test),
        rg_body_ir::CrateBodiesCoverage::SkippedByPolicy,
    );

    project
        .split_indexing()
        .materialize(AnalysisSurface::Crates(&[first_test]))
        .expect("one deferred test target should materialize");

    assert_eq!(
        coverage(&project, first_test),
        rg_body_ir::CrateBodiesCoverage::Complete,
    );
    assert_eq!(
        coverage(&project, second_test),
        rg_body_ir::CrateBodiesCoverage::SkippedByPolicy,
        "materializing one test must not build its sibling targets",
    );
}

#[test]
fn many_example_targets_stay_deferred_during_background_finishing() {
    // Use enough sibling targets that accidentally finishing every secondary target would retain a
    // meaningful fanout, without turning this policy regression into a stress test.
    const EXAMPLE_COUNT: usize = 24;

    let mut manifest = r#"
[package]
name = "example_fanout_fixture"
version = "0.1.0"
edition = "2024"

[lib]
path = "src/lib.rs"
"#
    .to_owned();
    let mut example_sources = String::new();
    for example_idx in 0..EXAMPLE_COUNT {
        writeln!(
            manifest,
            r#"
[[example]]
name = "example-{example_idx}"
path = "examples/example_{example_idx}.rs"
"#,
        )
        .expect("example manifest writes should succeed");
        writeln!(
            example_sources,
            r#"
//- /examples/example_{example_idx}.rs
fn main() {{
    let _ = example_fanout_fixture::library_value();
}}
"#,
        )
        .expect("example source writes should succeed");
    }

    let fixture = ProjectSourceFixture::build(&format!(
        r#"
//- /Cargo.toml
{manifest}
//- /src/lib.rs
pub fn library_value() -> usize {{ 1 }}
{example_sources}
"#,
    ));
    let mut project = Project::builder(fixture.workspace_metadata())
        .split_indexing_mode(SplitIndexingMode::EarlyStart)
        .build()
        .expect("many-example early-start project should build");
    project
        .split_indexing()
        .finish()
        .expect("configured eager targets should finish");

    let package =
        ProjectFixture::package_slot_by_name_in(project.state.parse_db(), "example_fanout_fixture");
    let parse_package = project
        .state
        .parse_db()
        .package(package.0)
        .expect("example fixture parse package should exist");
    let bodies = project
        .state
        .body_ir
        .resident_package(package)
        .expect("workspace example package Body IR should stay resident");
    let mut example_count = 0;
    for target in parse_package.targets() {
        let coverage = bodies
            .crate_bodies(rg_ir_model::CrateId(target.id.0))
            .expect("every example fixture target should have a Body IR slot")
            .coverage();
        match &target.kind {
            rg_workspace::TargetKind::Lib => {
                assert_eq!(coverage, rg_body_ir::CrateBodiesCoverage::Complete);
            }
            rg_workspace::TargetKind::Example => {
                example_count += 1;
                assert_eq!(
                    coverage,
                    rg_body_ir::CrateBodiesCoverage::SkippedByPolicy,
                    "background finishing should leave example target {:?} deferred",
                    target.id,
                );
            }
            kind => panic!("unexpected example fixture target kind {kind:?}"),
        }
    }
    assert_eq!(example_count, EXAMPLE_COUNT);
}

#[test]
fn shared_source_materialization_preserves_each_target_interpretation() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "shared_source_fixture"
version = "0.1.0"
edition = "2024"

[lib]
path = "src/shared.rs"

[[test]]
name = "first-shared"
path = "src/shared.rs"

[[test]]
name = "second-shared"
path = "src/shared.rs"

//- /src/shared.rs
pub fn shared_value() -> usize { 1 }
"#,
    );
    let mut project = Project::builder(fixture.workspace_metadata())
        .split_indexing_mode(SplitIndexingMode::EarlyStart)
        .build()
        .expect("shared-source early-start project should build");
    let context = project
        .snapshot()
        .file_contexts_for_path(fixture.path("src/shared.rs"))
        .expect("shared source contexts should resolve")
        .pop()
        .expect("shared source should have one package context");
    assert_eq!(context.crates.len(), 3);

    let target_named = |name: &str| {
        context
            .crates
            .iter()
            .copied()
            .find(|crate_ref| {
                let def_map = project
                    .state
                    .def_map
                    .resident_package(crate_ref.package)
                    .expect("workspace DefMap should stay resident");
                let crate_data = def_map
                    .crate_data(crate_ref.crate_id)
                    .expect("shared target should have definition data");
                project.state.parse_db().packages()[crate_ref.package.0]
                    .target(crate_data.cargo_target())
                    .is_some_and(|target| target.name == name)
            })
            .expect("named shared-source target should exist")
    };
    let first_test = target_named("first-shared");
    let second_test = target_named("second-shared");
    let library = context
        .crates
        .iter()
        .copied()
        .find(|crate_ref| *crate_ref != first_test && *crate_ref != second_test)
        .expect("shared source should also belong to its library target");

    project
        .split_indexing()
        .finish()
        .expect("configured eager targets should finish");
    project
        .split_indexing()
        .materialize(AnalysisSurface::Files(&[(first_test, context.file)]))
        .expect("one shared-source interpretation should materialize");

    let bodies = project
        .state
        .body_ir
        .resident_package(context.package)
        .expect("workspace Body IR should stay resident");
    assert_eq!(
        bodies
            .crate_bodies(first_test.crate_id)
            .expect("first shared test should have a body slot")
            .coverage(),
        rg_body_ir::CrateBodiesCoverage::Complete,
    );
    assert_eq!(
        bodies
            .crate_bodies(second_test.crate_id)
            .expect("second shared test should have a body slot")
            .coverage(),
        rg_body_ir::CrateBodiesCoverage::SkippedByPolicy,
        "the same FileId must not imply readiness for an unrequested target interpretation",
    );

    // A save invalidates every interpretation of this shared file. The rebuilt early-start state
    // should request the primary library again, restore policy-deferred coverage for both tests,
    // and still allow one exact test interpretation to be materialized afterward.
    let shared_path = fixture.path("src/shared.rs");
    std::fs::write(&shared_path, "pub fn shared_value() -> usize { 2 }\n")
        .expect("shared fixture source should be replaceable");
    project
        .apply_change(SavedFileChange::fs_path(&shared_path))
        .expect("shared source update should rebuild its target interpretations");

    let bodies = project
        .state
        .body_ir
        .resident_package(context.package)
        .expect("updated shared-source Body IR should stay resident");
    assert_eq!(
        bodies
            .crate_bodies(library.crate_id)
            .expect("updated shared library should have a body slot")
            .coverage(),
        rg_body_ir::CrateBodiesCoverage::Missing,
    );
    assert_eq!(
        bodies
            .crate_bodies(first_test.crate_id)
            .expect("updated first shared test should have a body slot")
            .coverage(),
        rg_body_ir::CrateBodiesCoverage::SkippedByPolicy,
    );
    assert_eq!(
        bodies
            .crate_bodies(second_test.crate_id)
            .expect("updated second shared test should have a body slot")
            .coverage(),
        rg_body_ir::CrateBodiesCoverage::SkippedByPolicy,
    );

    project
        .split_indexing()
        .finish()
        .expect("updated primary shared target should finish");
    project
        .split_indexing()
        .materialize(AnalysisSurface::Files(&[(first_test, context.file)]))
        .expect("updated first shared test should materialize exactly");
    let bodies = project
        .state
        .body_ir
        .resident_package(context.package)
        .expect("rematerialized shared-source Body IR should stay resident");
    assert_eq!(
        bodies
            .crate_bodies(library.crate_id)
            .expect("finished shared library should have a body slot")
            .coverage(),
        rg_body_ir::CrateBodiesCoverage::Complete,
    );
    assert_eq!(
        bodies
            .crate_bodies(first_test.crate_id)
            .expect("rematerialized first shared test should have a body slot")
            .coverage(),
        rg_body_ir::CrateBodiesCoverage::Complete,
    );
    assert_eq!(
        bodies
            .crate_bodies(second_test.crate_id)
            .expect("unrequested second shared test should have a body slot")
            .coverage(),
        rg_body_ir::CrateBodiesCoverage::SkippedByPolicy,
    );
}

#[test]
fn offloaded_secondary_target_materialization_rewrites_exact_cached_coverage() {
    let fixture = ProjectSourceFixture::build(TARGET_FANOUT_FIXTURE);
    let mut project = Project::builder(fixture.workspace_metadata())
        .split_indexing_mode(SplitIndexingMode::EarlyStart)
        .package_residency_policy(PackageResidencyPolicy::AllOffloadable)
        .build()
        .expect("offloadable multi-target project should build");

    let target_for_path = |project: &Project, path: &str| {
        project
            .snapshot()
            .file_contexts_for_path(fixture.path(path))
            .expect("target source path should resolve")
            .into_iter()
            .flat_map(|context| context.crates)
            .find(|crate_ref| {
                let package = project
                    .state
                    .def_map
                    .resident_package(crate_ref.package)
                    .expect("target discovery should run before package offload");
                let crate_data = package
                    .crate_data(crate_ref.crate_id)
                    .expect("target crate should have definition data");
                project.state.parse_db().packages()[crate_ref.package.0]
                    .target(crate_data.cargo_target())
                    .is_some_and(|target| target.src_path.ends_with(path))
            })
            .expect("source path should identify one semantic target")
    };
    let library = target_for_path(&project, "src/lib.rs");
    let first_test = target_for_path(&project, "tests/first.rs");
    let second_test = target_for_path(&project, "tests/second.rs");

    let first_test_file = ProjectFixture::file_id_for_path_in(
        project.state.parse_db(),
        &fixture.path("tests/first.rs"),
    );
    project
        .split_indexing()
        .finish()
        .expect("configured eager targets should finish and offload");
    assert!(
        project
            .state
            .body_ir
            .package_is_offloaded(first_test.package),
        "finished package should use its durable cache artifact",
    );

    let run = rg_profile::test_support::ProfileTest::start(
        crate::profile_descriptors(),
        "project.cache.sections",
    );
    project
        .split_indexing()
        .materialize(AnalysisSurface::Files(&[(first_test, first_test_file)]))
        .expect("one cached deferred test file should materialize its exact target");
    let profile = run.finish();
    assert!(
        profile
            .inner()
            .keyed_duration(metric::CACHE_SECTION_DECODE.path(), "body_ir.file")
            .is_none(),
        "exact target materialization should copy sibling shards without decoding them",
    );
    assert!(
        project
            .state
            .body_ir
            .package_is_offloaded(first_test.package),
        "improved package should return to lazy residency after rewriting its artifact",
    );
    assert!(
        !project
            .split_indexing()
            .needs_materialization(AnalysisSurface::Crates(&[first_test])),
        "materialized target coverage should remain available after offloading",
    );
    assert!(
        project
            .split_indexing()
            .needs_materialization(AnalysisSurface::Crates(&[second_test])),
        "an untouched sibling target should remain deferred",
    );

    let cached_package = project
        .state
        .cache_plan
        .package(first_test.package)
        .expect("fixture package should have a cache plan entry");
    let probe = project
        .state
        .cache_store
        .read_probe_for_package(cached_package)
        .expect("rewritten package cache probe should be readable")
        .expect("rewritten package cache probe should exist");
    assert_eq!(
        probe.body_ir_coverage[first_test.crate_id.0],
        rg_body_ir::CrateBodiesCoverage::Complete,
    );
    assert_eq!(
        probe.body_ir_coverage[second_test.crate_id.0],
        rg_body_ir::CrateBodiesCoverage::SkippedByPolicy,
        "rewriting one cached test must leave its sibling deferred",
    );

    // Inspect the rewritten revision without keeping its reader pinned across the restart below.
    {
        let header = project
            .state
            .cache_plan
            .artifact_header(
                first_test.package,
                &project.state.package_source_fingerprints,
            )
            .expect("fixture package should still have an artifact header");
        let reader = project
            .state
            .cache_store
            .open_artifact(&header)
            .expect("rewritten package artifact should open")
            .expect("rewritten package artifact should exist");
        assert!(
            !reader
                .read_body_crate(library.crate_id)
                .expect("untouched library Body IR should survive the artifact rewrite")
                .bodies()
                .is_empty(),
            "the copied sibling shard should retain its bodies",
        );
    }

    // A new project generation should seed the same decisions from the validated startup probe.
    // In particular, policy alone cannot tell that one secondary target was explicitly completed.
    drop(project);
    let mut restarted = Project::builder(fixture.workspace_metadata())
        .split_indexing_mode(SplitIndexingMode::EarlyStart)
        .package_residency_policy(PackageResidencyPolicy::AllOffloadable)
        .build()
        .expect("rewritten target coverage should survive a startup cache hit");
    assert!(
        restarted
            .state
            .body_ir
            .package_is_offloaded(first_test.package),
        "startup cache hit should keep the package payload lazy",
    );
    assert!(
        !restarted
            .split_indexing()
            .needs_materialization(AnalysisSurface::Crates(&[first_test])),
        "startup coverage should remember the completed secondary target",
    );
    assert!(
        restarted
            .split_indexing()
            .needs_materialization(AnalysisSurface::Crates(&[second_test])),
        "startup coverage should preserve the untouched deferred sibling",
    );
}
