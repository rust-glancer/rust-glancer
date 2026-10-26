use std::{
    fmt::{self, Write as _},
    fs,
};

use expect_test::{Expect, expect};
use rg_body_ir::BodyIrBuildPolicy;

use super::utils::{package_cache_header_for, write_cached_package_artifact};
use crate::{
    PackageResidencyPolicy, Project,
    profile::metric,
    testonly::{ProjectFixture, ProjectSourceFixture},
};

#[test]
fn resident_diagnostics_preserve_warm_package_artifacts() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /src/lib.rs crate:app deps:dep
pub struct App(dep::Dep);

//- /dep/src/lib.rs crate:dep root:dep origin:dependency
pub struct Dep;
"#,
    );
    let workspace = fixture.workspace_metadata();
    // Resident diagnostics need no package artifacts when starting with an empty cache.
    let resident = Project::builder(workspace.clone())
        .package_residency_policy(PackageResidencyPolicy::AllResident)
        .build()
        .expect("resident diagnostic build should succeed without a cache");
    for package in resident.state.cache_plan.packages() {
        assert!(
            !resident
                .state
                .cache_store
                .package_artifact_path(package)
                .exists(),
            "resident diagnostics should not write an artifact for {}",
            package.name,
        );
    }
    drop(resident);

    let cold = Project::builder(workspace.clone())
        .package_residency_policy(PackageResidencyPolicy::AllOffloadable)
        .build()
        .expect("cold build should persist package artifacts");
    drop(cold);

    // A diagnostic build must retain decoded packages even when matching artifacts already exist.
    // It should also leave that backing available for the next ordinary offloading build.
    let run = rg_profile::test_support::ProfileTest::start(
        crate::profile_descriptors(),
        "project.build.cache_probe",
    );
    let resident = Project::builder(workspace.clone())
        .package_residency_policy(PackageResidencyPolicy::AllResident)
        .build()
        .expect("resident diagnostic build should succeed");
    let counters = CacheProbeCounters::capture(run.finish());
    assert_eq!(resident.stats().def_map.resident_package_count, 2);
    assert_eq!(counters.hits, 0);
    assert_eq!(counters.misses, 0);
    drop(resident);

    let run = rg_profile::test_support::ProfileTest::start(
        crate::profile_descriptors(),
        "project.build.cache_probe",
    );
    let warm = Project::builder(workspace)
        .package_residency_policy(PackageResidencyPolicy::AllOffloadable)
        .build()
        .expect("offloading startup should reuse the preserved cache");
    let counters = CacheProbeCounters::capture(run.finish());
    assert_eq!(counters.hits, 2);
    assert_eq!(counters.misses, 0);
    assert_eq!(warm.stats().def_map.resident_package_count, 0);
}

#[test]
fn lazy_loads_offloaded_packages_for_queries() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /src/lib.rs crate:app deps:dep
pub struct App;

//- /dep/src/lib.rs crate:dep root:dep origin:dependency
pub struct DepType;

unsafe extern "C" {
    pub fn dep_foreign(value: u32) -> u32;
    pub static DEP_STATIC: u32;
    pub type DepOpaque;
}

//- /dep/src/bin/dep_tool.rs crate:dep root:dep origin:dependency kind:bin name:dep-tool
fn main() {}
"#,
    );
    let project =
        fixture.build_project_with_package_residency_policy(PackageResidencyPolicy::AllOffloadable);
    let dep = ProjectFixture::package_slot_by_name_in(project.snapshot().parse_db(), "dep");
    let analysis = project
        .snapshot()
        .full_analysis(rg_std::CancellationToken::new())
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

    let mut dump = StartupSnapshot::new("offloaded dependency query");
    dump.value(
        "dep resident",
        project.state.def_map.resident_package(dep).is_some(),
    );
    dump.section(
        "symbols",
        symbols.into_iter().map(|symbol| {
            let package = project
                .snapshot()
                .parse_db()
                .package(symbol.crate_ref.package.0)
                .expect("workspace symbol package should be parsed");
            // Crate ids are allocated in parsed target order. The DefMap payload is deliberately
            // offloaded in this fixture, so render through the stable parsed package shape instead.
            let target = package
                .targets()
                .get(symbol.crate_ref.crate_id.0)
                .expect("workspace symbol target should be parsed");
            format!(
                "{} {} @ {}[{}]",
                symbol.kind,
                symbol.name,
                package.package_name(),
                target.kind
            )
        }),
    );

    dump.check(expect![[r#"
        offloaded dependency query
        dep resident false
        symbols
        - fn dep_foreign @ dep[lib]
        - static DEP_STATIC @ dep[lib]
        - struct DepType @ dep[lib]
        - type_alias DepOpaque @ dep[lib]
    "#]]);
}

#[test]
fn startup_indexing_rejects_payload_with_forged_source_fingerprint() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /src/lib.rs crate:app deps:dep
pub struct App;

//- /dep/src/lib.rs crate:dep root:dep origin:dependency
pub struct DepOld;
"#,
    );
    let workspace = fixture.workspace_metadata();
    let project = Project::builder(workspace.clone())
        .package_residency_policy(PackageResidencyPolicy::AllOffloadable)
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
    let parse_after_edit = rg_parse::ParseDb::build(&workspace)
        .expect("fixture parse db should build after source edit");
    let source_fingerprints = project
        .state
        .cache_plan
        .source_fingerprints(workspace.workspace_root(), &parse_after_edit)
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
    let cached_project = Project::builder(workspace)
        .package_residency_policy(PackageResidencyPolicy::AllOffloadable)
        .build()
        .expect("fixture project should rebuild from matching artifact");
    let analysis = cached_project
        .snapshot()
        .full_analysis(rg_std::CancellationToken::new())
        .expect("cached project analysis should construct");
    let old_symbols = analysis
        .workspace_symbols("DepOld")
        .expect("old dependency symbol query should resolve");
    let new_symbols = analysis
        .workspace_symbols("DepNew")
        .expect("new dependency symbol query should resolve");

    let mut dump = StartupSnapshot::new("startup artifact-backed indexing");
    dump.value(
        "dep resident",
        cached_project.state.def_map.resident_package(dep).is_some(),
    );
    dump.value("old symbols", old_symbols.len());
    dump.value("new symbols", new_symbols.len());

    dump.check(expect![[r#"
        startup artifact-backed indexing
        dep resident false
        old symbols 0
        new symbols 1
    "#]]);
}

#[test]
fn startup_cache_misses_rebuild_reverse_dependents() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /dep/src/lib.rs crate:dep root:dep
pub struct Before;
pub struct Kept;

//- /mid/src/lib.rs crate:mid root:mid deps:dep
pub use dep::Kept;

//- /app/src/lib.rs crate:app root:app deps:mid
pub fn keep(value: mid::Ke$usage$pt) -> mid::Kept { value }

//- /independent/src/lib.rs crate:independent root:independent
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
    let run = rg_profile::test_support::ProfileTest::start(
        crate::profile_descriptors(),
        "project.build.cache_probe",
    );
    let project = Project::builder(workspace)
        .package_residency_policy(PackageResidencyPolicy::AllOffloadable)
        .build()
        .expect("dependency cache miss should rebuild its reverse dependents");
    let profile = CacheProbeCounters::capture(run.finish());

    let marker = fixture.markers().position("usage");
    let snapshot = project.snapshot();
    let context = snapshot
        .file_context_for_path(fixture.path(&marker.path))
        .expect("app source should resolve to a file context")
        .expect("app source should have one file context");
    let target = context.crate_ref;
    let analysis = snapshot
        .full_analysis(rg_std::CancellationToken::new())
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

    let mut dump = StartupSnapshot::new("dependency cache miss closure");
    dump.values(&[
        ("hits", profile.hits),
        ("direct restore misses", profile.parse_restore_errors),
        ("reverse-dependent misses", profile.propagated_misses),
    ]);
    dump.section(
        "definitions",
        definitions.into_iter().map(|definition| {
            let package = snapshot
                .parse_db()
                .package(definition.crate_ref.package.0)
                .expect("definition package should exist");
            format!(
                "{} {} {}",
                package.package_name(),
                definition.kind,
                definition.name
            )
        }),
    );

    dump.check(expect![[r#"
        dependency cache miss closure
        hits 1
        direct restore misses 1
        reverse-dependent misses 2
        definitions
        - dep struct Kept
    "#]]);
}

#[test]
fn missing_dependency_artifact_rebuilds_reverse_dependents() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /src/lib.rs crate:app deps:dep
pub struct App(dep::Dep);

//- /dep/src/lib.rs crate:dep root:dep origin:dependency
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
    let profile = CacheProbeCounters::capture(run.finish());
    let symbols = rebuilt
        .snapshot()
        .full_analysis(rg_std::CancellationToken::new())
        .expect("rebuilt project analysis should materialize")
        .workspace_symbols("Dep")
        .expect("dependency symbol query should resolve");

    let mut dump = StartupSnapshot::new("missing dependency artifact closure");
    dump.values(&[
        ("missing artifacts", profile.missing_artifacts),
        ("reverse-dependent misses", profile.propagated_misses),
        ("hits", profile.hits),
    ]);
    dump.value("dependency symbols", symbols.len());

    dump.check(expect![[r#"
        missing dependency artifact closure
        missing artifacts 1
        reverse-dependent misses 1
        hits 0
        dependency symbols 1
    "#]]);
}

#[test]
fn startup_discards_incomplete_cache_updates() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /src/lib.rs crate:app deps:dep
pub struct App(dep::Dep);

//- /dep/src/lib.rs crate:dep root:dep origin:dependency
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
    let recovery_profile = CacheProbeCounters::capture(recovery_run.finish());
    drop(recovered);

    let clean_run = rg_profile::test_support::ProfileTest::start(
        crate::profile_descriptors(),
        "project.build.cache_probe",
    );
    let _clean = Project::builder(workspace)
        .package_residency_policy(PackageResidencyPolicy::AllOffloadable)
        .build()
        .expect("cold recovery should publish a reusable cache again");
    let clean_profile = CacheProbeCounters::capture(clean_run.finish());

    let mut dump = StartupSnapshot::new("incomplete cache update recovery");
    dump.values(&[
        ("recovery hits", recovery_profile.hits),
        (
            "recovery missing artifacts",
            recovery_profile.missing_artifacts,
        ),
    ]);
    dump.values(&[("next startup hits", clean_profile.hits)]);

    dump.check(expect![[r#"
        incomplete cache update recovery
        recovery hits 0
        recovery missing artifacts 2
        next startup hits 2
    "#]]);
}

#[test]
fn startup_indexing_rejects_artifacts_when_body_ir_policy_needs_more_bodies() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /src/lib.rs crate:app deps:dep
pub fn app_value() -> usize { 1 }

//- /dep/src/lib.rs crate:dep root:dep origin:dependency
pub fn dep_value() -> usize { 2 }
"#,
    );
    let workspace = fixture.workspace_metadata();
    Project::builder(workspace.clone())
        .package_residency_policy(PackageResidencyPolicy::AllOffloadable)
        .build()
        .expect("fixture project should write dependency cache artifact");

    let run = rg_profile::test_support::ProfileTest::start(
        crate::profile_descriptors(),
        "project.build.cache_probe",
    );
    let project = Project::builder(workspace)
        .body_ir_policy(BodyIrBuildPolicy::all_packages())
        .package_residency_policy(PackageResidencyPolicy::AllOffloadable)
        .build()
        .expect("fixture project should reject body-policy-mismatched artifact");
    let snapshot = CacheProbeCounters::capture(run.finish());
    let header = package_cache_header_for(&project, "dep");
    let reader = project
        .state
        .cache_store
        .open_artifact(&header)
        .expect("dependency package cache artifact should open")
        .expect("dependency package cache artifact should exist");

    let mut dump = StartupSnapshot::new("startup body IR policy mismatch");
    dump.values(&[
        ("hits", snapshot.hits),
        ("misses", snapshot.misses),
        ("body policy mismatches", snapshot.body_policy_mismatches),
        ("propagated misses", snapshot.propagated_misses),
    ]);
    dump.section(
        "body IR crate statuses",
        reader
            .probe()
            .body_ir_coverage
            .iter()
            .enumerate()
            .map(|(crate_idx, coverage)| {
                format!("crate {crate_idx} {} {coverage}", coverage.status())
            }),
    );

    dump.check(expect![[r#"
        startup body IR policy mismatch
        hits 0
        misses 2
        body policy mismatches 1
        propagated misses 1
        body IR crate statuses
        - crate 0 built complete
    "#]]);
}

/// Record values immediately: a later startup can replace or remove the artifact being observed.
/// This helper only formats observations; cache operations and project lifetimes stay in the tests.
struct StartupSnapshot {
    dump: String,
}

impl StartupSnapshot {
    fn new(title: &str) -> Self {
        Self {
            dump: format!("{title}\n"),
        }
    }

    fn value(&mut self, label: &str, value: impl fmt::Display) {
        writeln!(self.dump, "{label} {value}").expect("string writes should not fail");
    }

    fn values(&mut self, rows: &[(&str, impl fmt::Display)]) {
        for (label, value) in rows {
            self.value(label, value);
        }
    }

    fn section(&mut self, title: &str, rows: impl IntoIterator<Item = impl fmt::Display>) {
        writeln!(self.dump, "{title}").expect("string writes should not fail");
        for row in rows {
            writeln!(self.dump, "- {row}").expect("string writes should not fail");
        }
    }

    fn check(&self, expected: Expect) {
        expected.assert_eq(&self.dump);
    }
}

/// A completed startup's counter observations. Tests choose which fields belong in their snapshot.
struct CacheProbeCounters {
    hits: u64,
    missing_artifacts: u64,
    parse_restore_errors: u64,
    propagated_misses: u64,
    body_policy_mismatches: u64,
    misses: u64,
}

impl CacheProbeCounters {
    fn capture(profile: rg_profile::test_support::TestSnapshot) -> Self {
        // Unrecorded counters mean that no events occurred in this profiling run.
        let counter =
            |metric: rg_profile::CounterMetric| profile.inner().counter(metric.path()).unwrap_or(0);
        let misses = [
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
        .map(counter)
        .sum();
        Self {
            hits: counter(metric::CACHE_PROBE_HITS),
            missing_artifacts: counter(metric::CACHE_PROBE_MISSING_ARTIFACTS),
            parse_restore_errors: counter(metric::CACHE_PROBE_PARSE_RESTORE_ERRORS),
            propagated_misses: counter(metric::CACHE_PROBE_PROPAGATED_MISSES),
            body_policy_mismatches: counter(metric::CACHE_PROBE_BODY_IR_POLICY_MISMATCHES),
            misses,
        }
    }
}
