mod current_body;
mod utils;

use std::{
    collections::BTreeSet,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use expect_test::expect;
use rg_analysis::ReferenceQuery;
use rg_source::CapturedSource;
use rg_std::MemorySize as _;

use self::utils::{HostFixture, HostObservation};
use crate::{
    AnalysisChangeSummary, AnalysisSurface, BuildProcessMemory, PackageResidencyPolicy, Project,
    ProjectMemoryHooks, ProjectMemoryPurgePoint, SavedFileChange, SplitIndexingMode,
    testonly::{ProjectFixture, ProjectSourceFixture},
};

#[derive(Debug)]
struct RecordingMemoryHooks {
    points: Arc<Mutex<Vec<ProjectMemoryPurgePoint>>>,
}

impl ProjectMemoryHooks for RecordingMemoryHooks {
    fn purge(&self, point: ProjectMemoryPurgePoint) {
        self.points
            .lock()
            .expect("recorded memory hook points should not be poisoned")
            .push(point);
    }
}

#[derive(Debug)]
struct SourceMutationMemoryHooks {
    armed: AtomicBool,
    path: PathBuf,
    replacement: &'static str,
}

impl SourceMutationMemoryHooks {
    fn new(path: PathBuf, replacement: &'static str, armed: bool) -> Self {
        Self {
            armed: AtomicBool::new(armed),
            path,
            replacement,
        }
    }

    fn arm(&self) {
        self.armed.store(true, Ordering::Release);
    }
}

impl ProjectMemoryHooks for SourceMutationMemoryHooks {
    fn purge(&self, point: ProjectMemoryPurgePoint) {
        if point == ProjectMemoryPurgePoint::AfterItemTreeSyntaxEviction
            && self.armed.swap(false, Ordering::AcqRel)
        {
            std::fs::write(&self.path, self.replacement)
                .expect("source mutation hook should replace fixture source");
        }
    }
}

#[test]
fn fresh_build_rejects_source_changed_between_item_tree_and_body_ir() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "source_generation_build_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Before;
"#,
    );
    let hooks: Arc<dyn ProjectMemoryHooks> = Arc::new(SourceMutationMemoryHooks::new(
        fixture.path("src/lib.rs"),
        "pub struct After;\n",
        true,
    ));

    let error = Project::builder(fixture.workspace_metadata())
        .memory_hooks(hooks)
        .build()
        .expect_err("a source change during construction should invalidate the candidate");

    assert!(
        error.chain().any(|cause| cause
            .downcast_ref::<rg_source::SourceError>()
            .is_some_and(|error| { matches!(error, rg_source::SourceError::Stale { .. }) })),
        "build failure should retain the typed stale-source cause: {error:#}",
    );
}

#[test]
fn fresh_build_rejects_module_existence_changed_after_discovery() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "source_generation_existence_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod later;
"#,
    );
    let hooks: Arc<dyn ProjectMemoryHooks> = Arc::new(SourceMutationMemoryHooks::new(
        fixture.path("src/later.rs"),
        "pub struct Appeared;\n",
        true,
    ));

    let error = Project::builder(fixture.workspace_metadata())
        .memory_hooks(hooks)
        .build()
        .expect_err("a module appearing during construction should invalidate the candidate");
    assert!(
        error.chain().any(|cause| matches!(
            cause.downcast_ref::<rg_source::SourceError>(),
            Some(rg_source::SourceError::ExistenceChanged { .. })
        )),
        "build failure should retain the source-existence cause: {error:#}",
    );
}

#[test]
fn failed_saved_candidate_preserves_published_generation() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "source_generation_update_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Published;
"#,
    );
    let path = fixture.path("src/lib.rs");
    let hooks = Arc::new(SourceMutationMemoryHooks::new(
        path.clone(),
        "pub struct Concurrent;\n",
        false,
    ));
    let mut project = Project::builder(fixture.workspace_metadata())
        .memory_hooks(hooks.clone())
        .build()
        .expect("initial project generation should build");
    let published_generation = project.generation_id();
    let file_id = ProjectFixture::file_id_for_path_in(project.state.parse_db(), &path);

    std::fs::write(&path, "pub struct Candidate;\n")
        .expect("candidate fixture source should be written");
    hooks.arm();
    let error = project
        .apply_change(SavedFileChange::fs_path(&path))
        .expect_err("concurrently changed candidate should not publish");
    assert!(
        error.chain().any(|cause| matches!(
            cause.downcast_ref::<rg_source::SourceError>(),
            Some(rg_source::SourceError::Stale { .. })
        )),
        "saved update failure should retain the typed stale-source cause: {error:#}",
    );
    assert_eq!(
        project.generation_id(),
        published_generation,
        "a rejected candidate must not advance project generation identity",
    );

    // Restoring the old disk bytes proves the published ParseDb still points at the old source
    // entry rather than at either failed candidate revision.
    std::fs::write(&path, "pub struct Published;\n")
        .expect("published fixture source should be restored");
    let text = project
        .snapshot()
        .file_text_for_span(
            rg_def_map::PackageSlot(0),
            file_id,
            rg_parse::Span {
                text: rg_parse::TextSpan { start: 0, end: 21 },
            },
        )
        .expect("published source should load after rejected candidate")
        .expect("published source span should exist");
    assert_eq!(text, "pub struct Published;");
}

#[test]
fn captured_saved_source_publishes_the_captured_text_and_revision() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "captured_saved_source_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Before;
"#,
    );
    let path = fixture.path("src/lib.rs");
    let mut project = fixture.build_project();
    let previous_generation = project.generation_id();
    let captured = CapturedSource::new(&path, "pub struct Captured;\n")
        .expect("existing fixture source should capture");
    std::fs::write(&path, captured.text()).expect("disk should contain the proposed saved value");

    let summary = project
        .apply_change(SavedFileChange::captured(captured.clone()))
        .expect("matching captured source should publish");

    assert_ne!(project.generation_id(), previous_generation);
    assert_eq!(summary.affected_packages, [rg_def_map::PackageSlot(0)]);
    let canonical = path
        .canonicalize()
        .expect("published fixture source should canonicalize");
    let entry = project
        .state
        .parse_db()
        .source_inventory()
        .entry(&canonical)
        .expect("published captured source should remain in the inventory");
    assert_eq!(entry.revision(), captured.revision());
    assert_eq!(
        entry
            .text()
            .expect("published captured source should reload from matching disk")
            .as_ref(),
        "pub struct Captured;\n"
    );
}

#[test]
fn captured_saved_source_rejects_newer_disk_without_changing_the_published_project() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "captured_saved_source_race_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Published;
"#,
    );
    let path = fixture.path("src/lib.rs");
    let mut project = fixture.build_project();
    let published_generation = project.generation_id();
    let file_id = ProjectFixture::file_id_for_path_in(project.state.parse_db(), &path);

    let captured = CapturedSource::new(&path, "pub struct Proposed;\n")
        .expect("existing fixture source should capture");
    std::fs::write(&path, "pub struct Newer;\n")
        .expect("disk should advance beyond the captured proposal");
    let error = project
        .apply_change(SavedFileChange::captured(captured))
        .expect_err("a newer disk value must reject the captured candidate");

    let canonical = path
        .canonicalize()
        .expect("fixture source should canonicalize");
    assert_eq!(
        Project::stale_source_path(&error),
        Some(canonical.as_path()),
        "validation should preserve the typed stale source identity"
    );
    assert_eq!(project.generation_id(), published_generation);

    // Restore the disk proof for the still-published generation, then verify that the failed
    // proposal never leaked into its ParseDb.
    std::fs::write(&path, "pub struct Published;\n")
        .expect("published fixture source should be restored");
    let text = project
        .snapshot()
        .file_text_for_span(
            rg_def_map::PackageSlot(0),
            file_id,
            rg_parse::Span {
                text: rg_parse::TextSpan { start: 0, end: 21 },
            },
        )
        .expect("published source should load after rejected proposal")
        .expect("published source span should exist");
    assert_eq!(text, "pub struct Published;");
}

#[test]
fn graph_rebuild_cannot_acknowledge_bytes_newer_than_a_captured_source() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "captured_graph_rebuild_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Published;
"#,
    );
    let source = fixture.path("src/lib.rs");
    let mut project = fixture.build_project();
    let published_generation = project.generation_id();
    let captured = CapturedSource::new(&source, "pub struct Proposed;\n")
        .expect("existing fixture source should capture");
    std::fs::write(&source, "pub struct Newer;\n")
        .expect("disk should advance beyond the captured watcher value");

    let error = project
        .apply_changes([
            SavedFileChange::fs_path(fixture.path("Cargo.toml")),
            SavedFileChange::captured(captured),
        ])
        .expect_err("graph rebuilding must retain the captured Rust-source identity");

    assert!(
        Project::stale_source_path(&error).is_some(),
        "the graph candidate should fail through saved-source validation: {error:#}"
    );
    assert_eq!(project.generation_id(), published_generation);
}

#[test]
fn unchanged_captured_save_does_not_publish_a_new_generation() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "unchanged_captured_save_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct AlreadySaved;
"#,
    );
    let path = fixture.path("src/lib.rs");
    let mut project = fixture.build_project();
    let generation = project.generation_id();
    let captured = CapturedSource::new(&path, "pub struct AlreadySaved;\n")
        .expect("existing fixture source should capture");

    let summary = project
        .apply_change(SavedFileChange::captured(captured))
        .expect("unchanged captured save should be accepted");

    assert_eq!(summary, AnalysisChangeSummary::default());
    assert_eq!(project.generation_id(), generation);
}

#[test]
fn missing_only_saved_change_batch_preserves_published_generation() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "source_generation_noop_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Published;
"#,
    );
    let mut project = fixture.build_project();
    let published_generation = project.generation_id();

    let summary = project
        .apply_changes([SavedFileChange::fs_path(fixture.path("src/disappeared.rs"))])
        .expect("an obsolete saved change should be a successful no-op");

    assert_eq!(summary, AnalysisChangeSummary::default());
    assert_eq!(
        project.generation_id(),
        published_generation,
        "a no-op batch must not publish a new project generation",
    );
}

#[test]
fn saved_module_rename_retires_historical_source() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "source_generation_module_rename_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod old;

//- /src/old.rs
pub struct Before;
"#,
    );
    let mut project = fixture.build_project();
    let root = fixture.path("src/lib.rs");
    let old = fixture.path("src/old.rs");
    let old_canonical = old
        .canonicalize()
        .expect("old module path should canonicalize before rename");
    let new = fixture.path("src/new.rs");

    std::fs::rename(&old, &new).expect("fixture module should be renamed");
    std::fs::write(&root, "mod new;\n").expect("fixture root should name the new module");
    let summary = project
        .apply_changes([
            SavedFileChange::fs_path(&root),
            SavedFileChange::fs_path(&new),
        ])
        .expect("module rename should publish a new source generation");

    assert_eq!(summary.affected_packages, [rg_def_map::PackageSlot(0)]);
    assert!(
        project
            .state
            .parse_db()
            .source_inventory()
            .entry(&old_canonical)
            .is_none(),
        "renamed module source should leave the published inventory",
    );
    assert!(
        !project.state.parse_db().contains_file_path(&old_canonical),
        "renamed module source should leave the package file table",
    );

    // A later save proves that the retired path cannot poison every candidate forked from this
    // generation.
    std::fs::write(&new, "pub struct After;\n")
        .expect("renamed module should accept a later saved edit");
    project
        .apply_change(SavedFileChange::fs_path(&new))
        .expect("later save should not validate the retired module path");
}

#[test]
fn offloaded_line_index_rejects_newer_disk_revision() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "source_generation_line_index_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Before;
"#,
    );
    let path = fixture.path("src/lib.rs");
    let project =
        fixture.build_project_with_package_residency_policy(PackageResidencyPolicy::AllOffloadable);
    let file_id = ProjectFixture::file_id_for_path_in(project.state.parse_db(), &path);
    std::fs::write(&path, "\n\npub struct After;\n")
        .expect("newer fixture source should be written");

    let error = project
        .snapshot()
        .file_line_index(rg_def_map::PackageSlot(0), file_id)
        .expect_err("line index reload should reject a newer source revision");
    assert!(
        error.chain().any(|cause| matches!(
            cause.downcast_ref::<rg_source::SourceError>(),
            Some(rg_source::SourceError::Stale { .. })
        )),
        "line-index failure should retain the typed stale-source cause: {error:#}",
    );
}

#[test]
fn saved_change_without_an_affected_package_still_seals_the_generation() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[workspace]
members = ["crates/app"]
resolver = "3"

//- /crates/app/Cargo.toml
[package]
name = "source_generation_irrelevant_change_fixture"
version = "0.1.0"
edition = "2024"

//- /crates/app/src/lib.rs
pub struct App;
"#,
    );
    let mut project = fixture.build_project();
    fixture.write_fixture_files(
        r#"
//- /generated/irrelevant.rs
pub struct Irrelevant;
"#,
    );
    let path = fixture.path("generated/irrelevant.rs");

    let summary = project
        .apply_change(SavedFileChange::fs_path(&path))
        .expect("irrelevant saved source should produce a valid generation");

    assert!(summary.changed_files.is_empty());
    assert!(summary.affected_packages.is_empty());
    assert!(
        project.state.parse_db().source_inventory().is_sealed(),
        "a published generation must be sealed even when no package was rebuilt",
    );
    project
        .state
        .parse_db()
        .validate_saved_sources()
        .expect("the published source set should remain valid");

    assert!(
        project
            .state
            .parse_db()
            .source_inventory()
            .entry(&path)
            .is_none(),
        "an unrelated watcher path should not become part of the published inventory",
    );
    std::fs::remove_file(&path).expect("irrelevant fixture source should be removable");
    let root = fixture.path("crates/app/src/lib.rs");
    std::fs::write(&root, "pub struct Updated;\n")
        .expect("known fixture source should accept a later edit");
    project
        .apply_change(SavedFileChange::fs_path(root))
        .expect("removed unrelated source must not poison later saved updates");
}

#[test]
fn project_memory_hooks_report_fresh_build_lifecycle_points() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "memory_hooks_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;
"#,
    );
    let workspace = fixture.workspace_metadata();
    let points = Arc::new(Mutex::new(Vec::new()));
    let hooks: Arc<dyn ProjectMemoryHooks> = Arc::new(RecordingMemoryHooks {
        points: Arc::clone(&points),
    });

    Project::builder(workspace)
        .memory_hooks(hooks)
        .build()
        .expect("project build with memory hooks should succeed");

    let points = points
        .lock()
        .expect("recorded memory hook points should not be poisoned");
    assert_eq!(
        points.as_slice(),
        [
            ProjectMemoryPurgePoint::AfterItemTreeSyntaxEviction,
            ProjectMemoryPurgePoint::AfterDefMapBuild,
            ProjectMemoryPurgePoint::AfterBodyIrBuild,
            ProjectMemoryPurgePoint::AfterProjectBuild,
        ],
        "fresh builds should expose the high-value transient memory boundaries",
    );
}

#[test]
fn batched_saved_source_changes_skip_missing_paths_and_rebuild_packages_once() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "batch_rebuild_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod account;
mod user;

//- /src/account.rs
pub struct Account;

//- /src/user.rs
pub struct User;
"#,
    );
    let points = Arc::new(Mutex::new(Vec::new()));
    let hooks: Arc<dyn ProjectMemoryHooks> = Arc::new(RecordingMemoryHooks {
        points: Arc::clone(&points),
    });
    let mut project = Project::builder(fixture.workspace_metadata())
        .memory_hooks(hooks)
        .build()
        .expect("analysis project should build");
    points
        .lock()
        .expect("recorded memory hook points should not be poisoned")
        .clear();

    let saved_files = fixture.write_fixture_files(
        r#"
//- /src/account.rs
pub struct SavedAccount;

//- /src/user.rs
pub struct SavedUser;
"#,
    );
    let mut changes = saved_files
        .files()
        .iter()
        .map(|file| SavedFileChange::fs_path(fixture.path(file.relative_path())))
        .collect::<Vec<_>>();
    changes.insert(
        1,
        SavedFileChange::fs_path(fixture.path("src/disappeared.rs")),
    );

    let summary = project
        .apply_changes(changes)
        .expect("batched source changes should apply");

    assert_eq!(
        summary.changed_files.len(),
        2,
        "both saved module files should be reported as changed"
    );
    assert_eq!(
        summary.affected_packages.len(),
        1,
        "one package should be rebuilt for the source-change batch"
    );
    assert_eq!(
        points
            .lock()
            .expect("recorded memory hook points should not be poisoned")
            .as_slice(),
        [
            ProjectMemoryPurgePoint::AfterItemTreeSyntaxEviction,
            ProjectMemoryPurgePoint::AfterPackageRebuild,
        ],
        "a multi-file source batch should rebuild the affected package closure once",
    );
}

#[test]
fn repeated_saved_source_notifications_do_not_rebuild_unchanged_packages() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "unchanged_notification_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct AlreadyApplied;
"#,
    );
    let points = Arc::new(Mutex::new(Vec::new()));
    let hooks: Arc<dyn ProjectMemoryHooks> = Arc::new(RecordingMemoryHooks {
        points: Arc::clone(&points),
    });
    let mut project = Project::builder(fixture.workspace_metadata())
        .memory_hooks(hooks)
        .build()
        .expect("analysis project should build");
    let generation = project.generation_id();
    points
        .lock()
        .expect("recorded memory hook points should not be poisoned")
        .clear();

    let summary = project
        .apply_change(SavedFileChange::fs_path(fixture.path("src/lib.rs")))
        .expect("an unchanged watcher replay should be accepted");

    assert_eq!(summary, AnalysisChangeSummary::default());
    assert_eq!(
        project.generation_id(),
        generation,
        "an unchanged watcher replay must not publish a replacement generation",
    );
    assert!(
        points
            .lock()
            .expect("recorded memory hook points should not be poisoned")
            .is_empty(),
        "an unchanged watcher replay must not enter package rebuilding",
    );
}

#[test]
fn source_batch_skips_file_removed_after_canonicalization() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "removed_during_batch_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod user;

//- /src/user.rs
pub struct User;
"#,
    );
    let mut project = fixture.build_project();
    let transient = fixture.path("src/transient.rs");
    let user = fixture.path("src/user.rs");
    std::fs::write(&transient, "pub struct Transient;\n")
        .expect("transient fixture should be writable");
    std::fs::write(&user, "pub struct SavedUser;\n").expect("user fixture should be writable");

    // The iterator removes transient.rs only when Project asks for the second item. This places the
    // removal after transient.rs was canonicalized and before the source-capture phase begins.
    let transient_for_changes = transient.clone();
    let user_for_changes = user.clone();
    let mut next_change = 0usize;
    let changes = std::iter::from_fn(move || match next_change {
        0 => {
            next_change += 1;
            Some(SavedFileChange::fs_path(&transient_for_changes))
        }
        1 => {
            next_change += 1;
            std::fs::remove_file(&transient_for_changes)
                .expect("transient fixture should be removable between batch phases");
            Some(SavedFileChange::fs_path(&user_for_changes))
        }
        _ => None,
    });

    let summary = project
        .apply_changes(changes)
        .expect("surviving source changes should still apply");

    let [changed_file] = summary.changed_files.as_slice() else {
        panic!("only the surviving user file should be reported as changed");
    };
    let changed_path = project
        .state
        .parse_db()
        .package(changed_file.package.0)
        .expect("changed package should exist")
        .parsed_file(changed_file.file)
        .expect("changed file should exist")
        .path()
        .to_path_buf();
    assert_eq!(
        changed_path,
        user.canonicalize()
            .expect("user fixture should canonicalize"),
        "a disappeared path should not prevent later batch members from rebuilding"
    );
}

fn project_build_checkpoints(
    snapshot: &rg_profile::test_support::TestSnapshot,
) -> &[rg_profile::ProfileCheckpoint] {
    snapshot
        .inner()
        .checkpoints(crate::profile::metric::CHECKPOINTS.path())
        .expect("project build checkpoints should be recorded")
}

fn checkpoint_optional_bytes(checkpoint: &rg_profile::ProfileCheckpoint, key: &str) -> Option<u64> {
    let value = checkpoint
        .values
        .iter()
        .find(|value| value.key == key)
        .unwrap_or_else(|| panic!("checkpoint should include {key:?}"));
    match value.value {
        rg_profile::ProfileMeasurement::Empty => None,
        rg_profile::ProfileMeasurement::Bytes(bytes) => Some(bytes),
        ref value => panic!("checkpoint value {key:?} should be bytes or empty, got {value:?}"),
    }
}

#[test]
fn dynamic_profile_records_timing_only_project_build_checkpoints() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "timing_profile_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;
"#,
    );
    let workspace = fixture.workspace_metadata();
    let run =
        rg_profile::test_support::ProfileTest::start(crate::profile_descriptors(), "project.build");

    Project::builder(workspace)
        .build()
        .expect("project build should succeed");
    let snapshot = run.finish();
    let checkpoints = project_build_checkpoints(&snapshot);
    let labels = checkpoints
        .iter()
        .map(|checkpoint| checkpoint.label.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        labels,
        [
            "after parse",
            "after cache probe",
            "after item-tree",
            "after item-tree syntax eviction",
            "after cache source fingerprints",
            "after def-map",
            "after semantic-ir",
            "after item-tree drop",
            "after body-ir",
            "after parse syntax eviction",
            "after project",
        ],
        "dynamic profile should report the same build checkpoints as memory profiling",
    );
    assert!(
        checkpoints
            .iter()
            .all(|checkpoint| checkpoint.phase_elapsed <= checkpoint.elapsed),
        "phase durations should be bounded by cumulative elapsed time"
    );
    for checkpoint in checkpoints {
        assert_eq!(
            checkpoint
                .values
                .iter()
                .map(|value| value.key.as_str())
                .collect::<Vec<_>>(),
            [
                "retained_bytes",
                "active_retained_bytes",
                "allocated_bytes",
                "active_bytes",
                "resident_bytes",
                "mapped_bytes",
            ],
            "timing-only checkpoints should keep the memory column shape",
        );
        assert!(
            checkpoint
                .values
                .iter()
                .all(|value| value.value == rg_profile::ProfileMeasurement::Empty),
            "timing-only profiling should not run memory samplers",
        );
    }
}

#[test]
fn early_start_build_can_finish_split_indexing_later() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "early_start_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn value() -> usize {
    1
}
"#,
    );
    let workspace = fixture.workspace_metadata();

    let mut project = Project::builder(workspace)
        .split_indexing_mode(SplitIndexingMode::EarlyStart)
        .build()
        .expect("early-start project build should succeed");

    let stats = project.stats().body_ir;
    assert_eq!(stats.crate_count, 1);
    assert_eq!(stats.complete_crate_count, 0);
    assert_eq!(stats.missing_crate_count, 1);
    assert_eq!(stats.body_count, 0);

    let source_path = fixture
        .path("src/lib.rs")
        .canonicalize()
        .expect("fixture source path should canonicalize");
    let source_entry = project
        .state
        .parse_db()
        .source_inventory()
        .entry(&source_path)
        .expect("fixture source should have a generation entry");
    let evicted_source_memory = source_entry.memory_size();

    project
        .split_indexing()
        .finish()
        .expect("deferred indexing should succeed");

    let stats = project.stats().body_ir;
    assert_eq!(stats.crate_count, 1);
    assert_eq!(stats.complete_crate_count, 1);
    assert_eq!(stats.missing_crate_count, 0);
    assert_eq!(stats.body_count, 1);
    assert_eq!(
        source_entry.memory_size(),
        evicted_source_memory,
        "deferred finishing should release source text reloaded by Body IR",
    );
}

#[test]
fn early_start_source_update_keeps_body_ir_deferred() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "early_start_update_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn before() -> usize {
    1
}
"#,
    );
    let source = fixture.path("src/lib.rs");
    let mut project = Project::builder(fixture.workspace_metadata())
        .split_indexing_mode(SplitIndexingMode::EarlyStart)
        .build()
        .expect("early-start project build should succeed");
    project
        .split_indexing()
        .finish()
        .expect("initial deferred indexing should succeed");
    assert_eq!(project.stats().body_ir.complete_crate_count, 1);

    std::fs::write(
        &source,
        r#"
pub fn after() -> usize {
    2
}
"#,
    )
    .expect("fixture source should be replaced");
    project
        .apply_change(SavedFileChange::fs_path(source))
        .expect("saved source update should succeed");

    let stats = project.stats().body_ir;
    assert_eq!(stats.complete_crate_count, 0);
    assert_eq!(stats.missing_crate_count, 1);
    assert_eq!(stats.body_count, 0);

    project
        .split_indexing()
        .finish()
        .expect("updated deferred indexing should succeed");
    let stats = project.stats().body_ir;
    assert_eq!(stats.complete_crate_count, 1);
    assert_eq!(stats.missing_crate_count, 0);
    assert_eq!(stats.body_count, 1);
}

#[test]
fn file_prepare_materializes_deferred_analysis_incrementally() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "file_ensure_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod other;

pub fn first() -> usize {
    1
}

//- /src/other.rs
pub fn second() -> usize {
    2
}
"#,
    );
    let workspace = fixture.workspace_metadata();

    let mut project = Project::builder(workspace)
        .split_indexing_mode(SplitIndexingMode::EarlyStart)
        .build()
        .expect("early-start project build should succeed");

    let stats = project.stats().body_ir;
    assert_eq!(stats.missing_crate_count, 1);
    assert_eq!(stats.body_count, 0);

    let lib_context = project
        .snapshot()
        .file_contexts_for_path(fixture.path("src/lib.rs"))
        .expect("lib file contexts should resolve")
        .pop()
        .expect("lib file should have one context");
    let lib_path = fixture
        .path("src/lib.rs")
        .canonicalize()
        .expect("lib source path should canonicalize");
    let lib_source = project
        .state
        .parse_db()
        .source_inventory()
        .entry(&lib_path)
        .expect("lib source should have a generation entry");
    let evicted_lib_memory = lib_source.memory_size();
    project
        .split_indexing()
        .materialize(AnalysisSurface::Files(&[(
            lib_context.package,
            lib_context.file,
        )]))
        .expect("lib file deferred analysis should materialize");

    let stats = project.stats().body_ir;
    assert_eq!(stats.complete_crate_count, 0);
    assert_eq!(stats.partial_crate_count, 1);
    assert_eq!(stats.body_count, 1);
    assert_eq!(
        lib_source.memory_size(),
        evicted_lib_memory,
        "file materialization should release source text reloaded by Body IR",
    );

    let other_context = project
        .snapshot()
        .file_contexts_for_path(fixture.path("src/other.rs"))
        .expect("other file contexts should resolve")
        .pop()
        .expect("other file should have one context");
    let other_path = fixture
        .path("src/other.rs")
        .canonicalize()
        .expect("other source path should canonicalize");
    let other_source = project
        .state
        .parse_db()
        .source_inventory()
        .entry(&other_path)
        .expect("other source should have a generation entry");
    let evicted_other_memory = other_source.memory_size();
    project
        .split_indexing()
        .materialize(AnalysisSurface::Files(&[(
            other_context.package,
            other_context.file,
        )]))
        .expect("other file deferred analysis should materialize");

    let stats = project.stats().body_ir;
    assert_eq!(stats.complete_crate_count, 1);
    assert_eq!(stats.partial_crate_count, 0);
    assert_eq!(stats.body_count, 2);
    assert_eq!(
        other_source.memory_size(),
        evicted_other_memory,
        "incremental materialization should release newly reloaded source text",
    );
}

#[test]
fn split_indexing_prevents_reference_and_rename_false_negatives() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "reference_surface_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod other;

pub struct User {
    pub na$subject$me: usize,
}

//- /src/other.rs
use crate::User;

pub fn demo(user: User) -> usize {
    user.name
}
"#,
    );
    let workspace = fixture.workspace_metadata();

    let mut project = Project::builder(workspace)
        .split_indexing_mode(SplitIndexingMode::EarlyStart)
        .build()
        .expect("early-start project build should succeed");

    let subject = fixture.markers().position("subject");
    let lib_context = project
        .snapshot()
        .file_contexts_for_path(fixture.path(&subject.path))
        .expect("subject file contexts should resolve")
        .pop()
        .expect("subject file should have one context");
    let other_context = project
        .snapshot()
        .file_contexts_for_path(fixture.path("src/other.rs"))
        .expect("other file contexts should resolve")
        .pop()
        .expect("other file should have one context");
    let target = lib_context
        .crates
        .first()
        .copied()
        .expect("subject file should belong to one target");

    let search_files = {
        let snapshot = project.snapshot();
        let analysis = snapshot
            .full_analysis()
            .expect("early-start analysis should materialize");
        let declaration_targets = analysis
            .goto_definition(target, lib_context.file, subject.offset)
            .expect("definition lookup should resolve")
            .into_iter()
            .map(|target| target.crate_ref)
            .collect::<Vec<_>>();
        let search_targets =
            snapshot.reference_search_crates(lib_context.package, &declaration_targets);
        let labels = analysis
            .reference_search_labels(target, lib_context.file, subject.offset)
            .expect("reference labels should resolve");
        let files = snapshot
            .reference_search_files_matching_labels(&search_targets, &labels)
            .expect("reference text prefilter should resolve")
            .expect("reference text prefilter should find label-bearing files");

        assert!(
            files
                .iter()
                .any(|file| file.crate_ref == target && file.file_id == other_context.file),
            "reference scan surface should include the file with body references",
        );

        files
    };

    let search_body_files = search_files
        .iter()
        .map(|file| (file.crate_ref.package, file.file_id))
        .collect::<Vec<_>>();
    project
        .split_indexing()
        .materialize(AnalysisSurface::Files(&search_body_files))
        .expect("reference scan files should materialize on demand");

    let snapshot = project.snapshot();
    let analysis = snapshot
        .full_analysis()
        .expect("completed analysis should materialize");
    let query = ReferenceQuery::find_references_in_files(&search_files, true);
    let references = analysis
        .references(target, lib_context.file, subject.offset, query)
        .expect("references should resolve after on-demand materialization");
    assert!(
        references
            .iter()
            .any(|reference| reference.file_id == other_context.file),
        "references should include body occurrences from the on-demand scan surface",
    );

    let query = ReferenceQuery::find_references_in_files(&search_files, true);
    let rename = analysis
        .rename(target, lib_context.file, subject.offset, "label", query)
        .expect("rename should resolve after on-demand materialization")
        .expect("rename should be available for the selected field");
    assert!(
        rename.edits.iter().any(|edit| {
            edit.file_id == other_context.file
                && edit.old_text == "name"
                && edit.new_text == "label"
        }),
        "rename should edit body occurrences from the on-demand scan surface",
    );
}

#[test]
fn merging_finished_split_indexing_does_not_downgrade_on_demand_package() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "background_merge_app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "dep", package = "background_merge_dep" }

//- /src/lib.rs
pub fn app_value() -> usize {
    dep::dep_value()
}

//- /dep/Cargo.toml
[package]
name = "background_merge_dep"
version = "0.1.0"
edition = "2024"

//- /dep/src/lib.rs
pub fn dep_value() -> usize {
    let value = 1usize;
    val$dep_ref$ue
}
"#,
    );
    let workspace = fixture.workspace_metadata();

    let mut project = Project::builder(workspace)
        .split_indexing_mode(SplitIndexingMode::EarlyStart)
        .build()
        .expect("early-start project build should succeed");
    let finished = project
        .detach_split_indexing()
        .finish()
        .expect("background deferred indexing should succeed");
    let dep_ref = fixture.markers().position("dep_ref");
    let dep_context = project
        .snapshot()
        .file_contexts_for_path(fixture.path(&dep_ref.path))
        .expect("dependency file contexts should resolve")
        .pop()
        .expect("dependency file should have one context");
    let dep_target = dep_context
        .crates
        .first()
        .copied()
        .expect("dependency file should belong to one target");

    project
        .split_indexing()
        .materialize(AnalysisSurface::Crates(&dep_context.crates))
        .expect("on-demand dependency deferred indexing should succeed");
    assert!(
        project
            .snapshot()
            .full_analysis()
            .expect("analysis should materialize after on-demand dependency finish")
            .type_at(dep_target, dep_context.file, dep_ref.offset)
            .expect("dependency body-local type query should resolve")
            .is_some(),
        "dependency body should be queryable after on-demand finish",
    );

    assert!(
        project
            .split_indexing()
            .merge_finished(finished)
            .expect("background deferred indexing should merge"),
        "workspace package finish should still merge",
    );
    assert!(
        project
            .snapshot()
            .full_analysis()
            .expect("analysis should materialize after background merge")
            .type_at(dep_target, dep_context.file, dep_ref.offset)
            .expect("dependency body-local type query should resolve after merge")
            .is_some(),
        "background finish must not reinstall the clone's skipped dependency bodies",
    );
}

#[test]
fn final_detached_merge_keeps_priority_package_offloaded() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "offloaded_priority_merge"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn value() -> usize {
    1
}
"#,
    );
    let mut project = Project::builder(fixture.workspace_metadata())
        .split_indexing_mode(SplitIndexingMode::EarlyStart)
        .package_residency_policy(PackageResidencyPolicy::AllOffloadable)
        .build()
        .expect("early-start offloadable project build should succeed");
    let priority_finished = Arc::new(Mutex::new(None));
    let priority_publication = Arc::clone(&priority_finished);
    let final_finished = project
        .detach_split_indexing()
        .finish_with_package_priority(
            || vec![rg_def_map::PackageSlot(0)],
            move |finished| {
                let previous = priority_publication
                    .lock()
                    .expect("priority publication should not be poisoned")
                    .replace(finished);
                assert!(
                    previous.is_none(),
                    "one-package build should publish priority data once",
                );
            },
        )
        .expect("background deferred indexing should succeed");
    let priority_finished = priority_finished
        .lock()
        .expect("priority publication should not be poisoned")
        .take()
        .expect("priority package should publish before the final result");

    assert!(
        project
            .split_indexing()
            .merge_finished(priority_finished)
            .expect("priority package should merge"),
        "priority publication should finish the saved package",
    );
    assert_eq!(
        project.stats().body_ir.crate_count,
        0,
        "priority publication should return the finished package to offloaded residency",
    );

    assert!(
        !project
            .split_indexing()
            .merge_finished(final_finished)
            .expect("final background result should reconcile"),
        "the final result should not reinstall an already-offloaded priority package",
    );
    assert_eq!(
        project.stats().body_ir.crate_count,
        0,
        "final reconciliation should preserve offloaded residency",
    );
}

#[test]
fn residency_keeps_partial_deferred_payload_transient_and_resident() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[workspace]
members = ["app", "helper"]

//- /app/Cargo.toml
[package]
name = "partial_residency_app"
version = "0.1.0"
edition = "2024"

//- /app/src/lib.rs
mod other;

pub fn first() -> usize {
    let value = 1usize;
    val$app_ref$ue
}

//- /app/src/other.rs
pub fn second() -> usize {
    2
}

//- /helper/Cargo.toml
[package]
name = "partial_residency_helper"
version = "0.1.0"
edition = "2024"

//- /helper/src/lib.rs
pub fn helper() -> usize {
    1
}
"#,
    );
    let workspace = fixture.workspace_metadata();

    let mut project = Project::builder(workspace)
        .split_indexing_mode(SplitIndexingMode::EarlyStart)
        .package_residency_policy(PackageResidencyPolicy::AllOffloadable)
        .build()
        .expect("early-start project build should succeed");
    let app_ref = fixture.markers().position("app_ref");
    let lib_context = project
        .snapshot()
        .file_contexts_for_path(fixture.path(&app_ref.path))
        .expect("lib file contexts should resolve")
        .pop()
        .expect("lib file should have one context");
    let helper_context = project
        .snapshot()
        .file_contexts_for_path(fixture.path("helper/src/lib.rs"))
        .expect("helper file contexts should resolve")
        .pop()
        .expect("helper file should have one context");
    let app_target = lib_context
        .crates
        .first()
        .copied()
        .expect("app file should belong to one target");

    project
        .split_indexing()
        .materialize(AnalysisSurface::Files(&[(
            lib_context.package,
            lib_context.file,
        )]))
        .expect("lib file deferred analysis should materialize");
    let stats = project.stats().body_ir;
    assert_eq!(stats.partial_crate_count, 1);
    assert_eq!(stats.body_count, 1);

    project
        .split_indexing()
        .materialize(AnalysisSurface::Files(&[(
            helper_context.package,
            helper_context.file,
        )]))
        .expect("helper deferred indexing should apply residency");
    let stats = project.stats().body_ir;
    assert_eq!(stats.partial_crate_count, 1);
    assert_eq!(stats.body_count, 1);

    assert!(
        project
            .snapshot()
            .full_analysis()
            .expect("analysis should materialize after helper residency")
            .type_at(app_target, lib_context.file, app_ref.offset)
            .expect("app body-local type query should resolve")
            .is_some(),
        "partial on-demand app body should remain resident after another package applies residency",
    );
}

#[test]
fn file_prepare_keeps_finished_offloaded_payload_lazy() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "finished_offload_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn value() -> usize {
    let value = 1usize;
    val$ref$ue
}
"#,
    );
    let workspace = fixture.workspace_metadata();

    let mut project = Project::builder(workspace)
        .split_indexing_mode(SplitIndexingMode::EarlyStart)
        .package_residency_policy(PackageResidencyPolicy::AllOffloadable)
        .build()
        .expect("early-start project build should succeed");
    let reference = fixture.markers().position("ref");
    let context = project
        .snapshot()
        .file_contexts_for_path(fixture.path(&reference.path))
        .expect("fixture file contexts should resolve")
        .pop()
        .expect("fixture file should have one context");
    let target = context
        .crates
        .first()
        .copied()
        .expect("fixture file should belong to one target");

    project
        .split_indexing()
        .finish()
        .expect("deferred indexing should finish and restore residency");
    assert!(
        project
            .state
            .body_ir
            .resident_package(context.package)
            .is_none(),
        "finished all-offloadable package should return to lazy cache-backed residency",
    );

    project
        .split_indexing()
        .materialize(AnalysisSurface::Files(&[(context.package, context.file)]))
        .expect("file-local preparation should treat finished offloaded payload as ready");
    assert!(
        project
            .state
            .body_ir
            .resident_package(context.package)
            .is_none(),
        "file-local preparation should not rebuild a finished offloaded package from source",
    );

    assert!(
        project
            .snapshot()
            .full_analysis()
            .expect("analysis should lazy-load the finished offloaded package")
            .type_at(target, context.file, reference.offset)
            .expect("body-local type query should resolve from lazy package data")
            .is_some(),
        "finished offloaded body data should still be queryable through lazy loading",
    );
}

#[test]
fn process_memory_sampler_enables_retained_build_memory() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "process_memory_profile_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;
"#,
    );
    let workspace = fixture.workspace_metadata();
    let run =
        rg_profile::test_support::ProfileTest::start(crate::profile_descriptors(), "project.build");

    Project::builder(workspace)
        .process_memory_sampler(|| {
            Some(BuildProcessMemory {
                allocated_bytes: 11,
                active_bytes: 13,
                resident_bytes: 17,
                mapped_bytes: 19,
            })
        })
        .build()
        .expect("process-memory-profiled project build should succeed");
    let snapshot = run.finish();
    let after_parse = project_build_checkpoints(&snapshot)
        .iter()
        .find(|checkpoint| checkpoint.label == "after parse")
        .expect("profile should contain the parse checkpoint");

    assert!(
        checkpoint_optional_bytes(after_parse, "retained_bytes").is_some_and(|bytes| bytes > 0),
        "process memory sampling should also enable retained object measurements"
    );
    assert_eq!(
        checkpoint_optional_bytes(after_parse, "allocated_bytes"),
        Some(11),
        "process memory sampling should still record allocator counters"
    );
}

#[test]
fn profiled_split_indexing_reports_finish_and_residency_memory() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "profiled_split_finish_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn answer() -> i32 {
    42
}
"#,
    );
    let workspace = fixture.workspace_metadata();
    let run =
        rg_profile::test_support::ProfileTest::start(crate::profile_descriptors(), "project.build");

    let mut project = Project::builder(workspace)
        .split_indexing_mode(SplitIndexingMode::EarlyStart)
        .package_residency_policy(PackageResidencyPolicy::AllOffloadable)
        .process_memory_sampler(|| {
            Some(BuildProcessMemory {
                allocated_bytes: 11,
                active_bytes: 13,
                resident_bytes: 17,
                mapped_bytes: 19,
            })
        })
        .build()
        .expect("early-start project build should succeed");
    project
        .split_indexing()
        .finish_profiled(|| {
            Some(BuildProcessMemory {
                allocated_bytes: 31,
                active_bytes: 37,
                resident_bytes: 41,
                mapped_bytes: 43,
            })
        })
        .expect("profiled deferred indexing should succeed");

    let snapshot = run.finish();
    let checkpoints = project_build_checkpoints(&snapshot);
    let finish_labels = checkpoints
        .iter()
        .filter(|checkpoint| checkpoint_optional_bytes(checkpoint, "allocated_bytes") == Some(31))
        .map(|checkpoint| checkpoint.label.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        finish_labels,
        [
            "after deferred indexing",
            "before package cache write",
            "after package cache write",
            "after package payload offload",
            "after package offload cleanup",
            "after deferred indexing cleanup",
        ],
        "deferred indexing should profile both lowering and the follow-up residency pass",
    );

    let deferred_finish = checkpoints
        .iter()
        .find(|checkpoint| checkpoint.label == "after deferred indexing")
        .expect("profile should contain the deferred indexing checkpoint");
    assert!(
        checkpoint_optional_bytes(deferred_finish, "retained_bytes").is_some_and(|bytes| bytes > 0),
        "profiled deferred indexing should record retained project memory",
    );
}

#[test]
fn residency_profile_reports_offload_checkpoints() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "residency_profile_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;
"#,
    );
    let workspace = fixture.workspace_metadata();
    let run =
        rg_profile::test_support::ProfileTest::start(crate::profile_descriptors(), "project.build");

    Project::builder(workspace)
        .package_residency_policy(PackageResidencyPolicy::AllOffloadable)
        .build()
        .expect("profiled offloadable project build should succeed");
    let snapshot = run.finish();
    let labels = project_build_checkpoints(&snapshot)
        .iter()
        .map(|checkpoint| checkpoint.label.as_str())
        .collect::<Vec<_>>();

    for label in [
        "before package cache write",
        "after package cache write",
        "after package payload offload",
        "after package offload cleanup",
    ] {
        assert!(
            labels.contains(&label),
            "offloadable builds should report the {label:?} residency checkpoint"
        );
    }
}

#[test]
fn project_build_records_def_map_profile_when_profile_run_is_active() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "def_map_profile_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
macro_rules! make_item {
    ($name:ident) => {
        pub struct $name;
    };
}

make_item!(User);
make_item!(Admin);
"#,
    );
    let workspace = fixture.workspace_metadata();
    let run = rg_profile::test_support::ProfileTest::start(
        crate::profile_descriptors(),
        "def_map.finalization,def_map.macros",
    );

    Project::builder(workspace)
        .build()
        .expect("project build should succeed");
    let snapshot = run.finish();

    snapshot.assert_counter_path_with_message(
        "def_map.macros.calls.expanded",
        2,
        "the fixture should expand both macro calls",
    );
    snapshot.assert_counter_path_with_message(
        "def_map.macros.compile.attempts",
        1,
        "multiple calls to one macro definition should share compiled macro data",
    );
    snapshot.assert_counter_path_with_message(
        "def_map.macros.compile.cache_hits",
        1,
        "the second call should reuse the cached compiled macro",
    );
    snapshot.assert_counter_path_with_message(
        "def_map.macros.generated.sources_parsed",
        2,
        "each expanded generated item source should be parsed",
    );
}

#[test]
fn reference_search_crates_follow_workspace_reverse_dependencies() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[workspace]
members = ["crates/dep", "crates/mid", "crates/app", "crates/independent"]
resolver = "3"

//- /crates/dep/Cargo.toml
[package]
name = "dep"
version = "0.1.0"
edition = "2024"

//- /crates/dep/src/lib.rs
pub struct Dep;

//- /crates/mid/Cargo.toml
[package]
name = "mid"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "../dep" }

//- /crates/mid/src/lib.rs
pub struct Mid(dep::Dep);

//- /crates/app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
mid = { path = "../mid" }

//- /crates/app/src/lib.rs
pub struct App(mid::Mid);

//- /crates/independent/Cargo.toml
[package]
name = "independent"
version = "0.1.0"
edition = "2024"

//- /crates/independent/src/lib.rs
pub struct Independent;
"#,
    );
    let project = fixture.build_project();
    let snapshot = project.snapshot();
    let dep_package = ProjectFixture::package_slot_by_name_in(snapshot.parse_db(), "dep");
    let app_package = ProjectFixture::package_slot_by_name_in(snapshot.parse_db(), "app");
    let dep_file = ProjectFixture::file_id_for_path_in(
        snapshot.parse_db(),
        &fixture.path("crates/dep/src/lib.rs"),
    );
    let dep_target = snapshot
        .crates_for_file(dep_package, dep_file)
        .expect("dep target lookup should succeed")
        .into_iter()
        .next()
        .expect("dep lib file should belong to the dep lib target");

    let package_names = snapshot
        .reference_search_crates(app_package, &[dep_target])
        .into_iter()
        .map(|target| {
            snapshot
                .parse_db()
                .package(target.package.0)
                .expect("reference search target package should exist")
                .package_name()
                .to_owned()
        })
        .collect::<BTreeSet<_>>();

    assert_eq!(
        package_names,
        BTreeSet::from(["app".to_string(), "dep".to_string(), "mid".to_string()]),
        "workspace reference searches should skip unrelated workspace packages"
    );
}

#[test]
fn profiled_build_reports_phase_checkpoints_without_exposing_phase_dbs() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "profile_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;
"#,
    );
    let workspace = fixture.workspace_metadata();
    let run =
        rg_profile::test_support::ProfileTest::start(crate::profile_descriptors(), "project.build");

    Project::builder(workspace)
        .measure_retained_memory(true)
        .build()
        .expect("profiled project build should succeed");
    let snapshot = run.finish();
    let checkpoints = project_build_checkpoints(&snapshot);
    let labels = checkpoints
        .iter()
        .map(|checkpoint| checkpoint.label.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        labels,
        [
            "after parse",
            "after cache probe",
            "after item-tree",
            "after item-tree syntax eviction",
            "after cache source fingerprints",
            "after def-map",
            "after semantic-ir",
            "after item-tree drop",
            "after body-ir",
            "after parse syntax eviction",
            "after project",
        ]
    );

    assert!(
        checkpoints
            .iter()
            .filter_map(|checkpoint| checkpoint_optional_bytes(checkpoint, "retained_bytes"))
            .all(|bytes| bytes > 0),
        "retained checkpoints should record non-zero memory"
    );
    assert!(
        checkpoints
            .iter()
            .all(
                |checkpoint| checkpoint_optional_bytes(checkpoint, "active_retained_bytes")
                    .is_some()
            ),
        "retained profiling should record active live-state memory for every checkpoint"
    );

    let item_tree_drop = checkpoints
        .iter()
        .find(|checkpoint| checkpoint.label == "after item-tree drop")
        .expect("profile should contain item-tree drop checkpoint");
    assert_eq!(
        checkpoint_optional_bytes(item_tree_drop, "retained_bytes"),
        None,
        "process-only checkpoints should not pretend to sample a dropped phase object"
    );
}

#[test]
fn build_memory_snapshot_captures_requested_transient_point() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "build_memory_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;
"#,
    );
    let workspace = fixture.workspace_metadata();
    let run = rg_profile::test_support::ProfileTest::start(
        crate::profile_descriptors(),
        "project.build.def_map",
    );
    Project::builder(workspace)
        .measure_retained_memory(true)
        .build()
        .expect("memory-targeted project build should succeed");
    let snapshot = run.finish();
    let memory = snapshot
        .inner()
        .memory_snapshot(crate::profile::metric::DEF_MAP_MEMORY.path())
        .expect("requested memory point should capture detailed memory");

    assert!(
        memory.retained_bytes > 0,
        "build memory snapshot should report retained bytes"
    );

    let paths = memory
        .records
        .iter()
        .map(|record| record.path.as_str())
        .collect::<Vec<_>>();
    assert!(
        paths.iter().any(|path| path.starts_with("build.def_map")),
        "def-map memory snapshot should include def-map memory"
    );
    assert!(
        paths.iter().any(|path| path.starts_with("build.item_tree")),
        "def-map memory snapshot should still include live item-tree memory"
    );
}

#[test]
fn reparses_known_file_in_place() {
    let mut fixture = HostFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "host_update_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;
"#,
    );
    let before_file_id = fixture.file_id_for_path("src/lib.rs");

    fixture.check(
        &[HostObservation::workspace_symbols("User")],
        expect![[r#"
            workspace symbols `User`
            - struct User @ host_update_fixture[lib] src/lib.rs
        "#]],
    );

    fixture.check_save(
        r#"
//- /src/lib.rs
pub struct Account;
"#,
        &[
            HostObservation::workspace_symbols("Account"),
            HostObservation::workspace_symbols("User"),
        ],
        expect![[r#"
            changed files
            - host_update_fixture src/lib.rs

            affected packages
            - host_update_fixture

            changed targets
            - host_update_fixture[lib]

            workspace symbols `Account`
            - struct Account @ host_update_fixture[lib] src/lib.rs

            workspace symbols `User`
            - <none>
        "#]],
    );

    let after_file_id = fixture.file_id_for_path("src/lib.rs");
    assert_eq!(
        after_file_id, before_file_id,
        "known file reparses should keep the package-local FileId stable"
    );
}

#[test]
fn reads_saved_disk_text_for_modules_discovered_after_the_change() {
    let mut fixture = HostFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "host_new_module_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Root;

//- /src/api.rs
pub struct DiskOnly;
"#,
    );

    fixture.check_save(
        r#"
//- /src/api.rs
pub struct SavedOnly;

//- /src/lib.rs
mod api;
"#,
        &[
            HostObservation::workspace_symbols("SavedOnly"),
            HostObservation::workspace_symbols("DiskOnly"),
        ],
        expect![[r#"
            changed files
            - host_new_module_fixture src/api.rs
            - host_new_module_fixture src/lib.rs

            affected packages
            - host_new_module_fixture

            changed targets
            - host_new_module_fixture[lib]

            workspace symbols `SavedOnly`
            - struct SavedOnly @ host_new_module_fixture[lib] src/api.rs

            workspace symbols `DiskOnly`
            - <none>
        "#]],
    );
}

#[test]
fn rebuilds_project_after_manifest_adds_dependency() {
    let mut fixture = HostFixture::build(
        r#"
//- /Cargo.toml
[workspace]
members = ["crates/dep", "crates/app"]
resolver = "3"

//- /crates/dep/Cargo.toml
[package]
name = "dep"
version = "0.1.0"
edition = "2024"

//- /crates/dep/src/lib.rs
pub struct Api;

//- /crates/app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

//- /crates/app/src/lib.rs
pub fn use_dep(value: dep::Api) {
    let same = val$0ue;
}
"#,
    );

    fixture.check(
        &[HostObservation::type_names_at("app marker 0", "app", "0")],
        expect![[r#"
            type names at `app marker 0`
            - <none>
        "#]],
    );

    fixture.check_save(
        r#"
//- /crates/app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "../dep" }
"#,
        &[HostObservation::type_names_at("app marker 0", "app", "0")],
        expect![[r#"
            changed files
            - <none>

            affected packages
            - app
            - dep

            changed targets
            - app[lib]
            - dep[lib]

            type names at `app marker 0`
            - Api
        "#]],
    );
}

#[test]
fn rebuilds_project_after_manifest_adds_target() {
    let mut fixture = HostFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "target_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Lib;

//- /src/tool.rs
fn main() {}
"#,
    );

    fixture.check(
        &[HostObservation::file_contexts("tool file", "src/tool.rs")],
        expect![[r#"
            file contexts `tool file`
            - <none>
        "#]],
    );

    fixture.check_save(
        r#"
//- /Cargo.toml
[package]
name = "target_fixture"
version = "0.1.0"
edition = "2024"

[[bin]]
name = "tool"
path = "src/tool.rs"
"#,
        &[HostObservation::file_contexts("tool file", "src/tool.rs")],
        expect![[r#"
            changed files
            - <none>

            affected packages
            - target_fixture

            changed targets
            - target_fixture[bin]
            - target_fixture[lib]

            file contexts `tool file`
            - target_fixture src/tool.rs -> target_fixture[bin]
        "#]],
    );
}

#[test]
fn rebuilds_project_after_new_workspace_member_manifest_is_saved() {
    let mut fixture = HostFixture::build(
        r#"
//- /Cargo.toml
[workspace]
members = ["crates/*"]
resolver = "3"

//- /crates/app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

//- /crates/app/src/lib.rs
pub struct App;
"#,
    );

    fixture.check_save(
        r#"
//- /crates/new_pkg/Cargo.toml
[package]
name = "new_pkg"
version = "0.1.0"
edition = "2024"

//- /crates/new_pkg/src/lib.rs
pub struct NewType;
"#,
        &[HostObservation::workspace_symbols("NewType")],
        expect![[r#"
            changed files
            - new_pkg crates/new_pkg/src/lib.rs

            affected packages
            - app
            - new_pkg

            changed targets
            - app[lib]
            - new_pkg[lib]

            workspace symbols `NewType`
            - struct NewType @ new_pkg[lib] crates/new_pkg/src/lib.rs
        "#]],
    );
}

#[test]
fn discovers_new_workspace_member_after_manifest_becomes_valid() {
    let mut fixture = HostFixture::build(
        r#"
//- /Cargo.toml
[workspace]
members = ["crates/*"]
resolver = "3"

//- /crates/app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

//- /crates/app/src/lib.rs
pub struct App;
"#,
    );

    fixture.check_save(
        r#"
//- /crates/new_pkg/Cargo.toml
[package]
name = "new_pkg"
version = "0.1.0"
edition = "2024"
"#,
        &[HostObservation::workspace_symbols("NewType")],
        expect![[r#"
            changed files
            - <none>

            affected packages
            - <none>

            changed targets
            - <none>

            workspace symbols `NewType`
            - <none>
        "#]],
    );

    fixture.check_save(
        r#"
//- /crates/new_pkg/src/lib.rs
pub struct NewType;
"#,
        &[HostObservation::workspace_symbols("NewType")],
        expect![[r#"
            changed files
            - new_pkg crates/new_pkg/src/lib.rs

            affected packages
            - app
            - new_pkg

            changed targets
            - app[lib]
            - new_pkg[lib]

            workspace symbols `NewType`
            - struct NewType @ new_pkg[lib] crates/new_pkg/src/lib.rs
        "#]],
    );
}

#[test]
fn workspace_graph_rebuild_reports_changed_crates_when_packages_are_offloaded() {
    let mut fixture = HostFixture::build_with_package_residency_policy(
        r#"
//- /Cargo.toml
[package]
name = "offloaded_target_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Lib;

//- /src/tool.rs
fn main() {}
"#,
        PackageResidencyPolicy::AllOffloadable,
    );

    fixture.check_save(
        r#"
//- /Cargo.toml
[package]
name = "offloaded_target_fixture"
version = "0.1.0"
edition = "2024"

[[bin]]
name = "tool"
path = "src/tool.rs"
"#,
        &[],
        expect![[r#"
            changed files
            - <none>

            affected packages
            - offloaded_target_fixture

            changed targets
            - offloaded_target_fixture[bin]
            - offloaded_target_fixture[lib]
        "#]],
    );
}

#[test]
fn rebuilds_project_after_auto_discovered_target_is_added() {
    let mut fixture = HostFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "autotarget_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Lib;
"#,
    );

    fixture.check_save(
        r#"
//- /tests/smoke.rs
#[test]
fn smoke() {}
"#,
        &[HostObservation::file_contexts(
            "smoke test",
            "tests/smoke.rs",
        )],
        expect![[r#"
            changed files
            - autotarget_fixture tests/smoke.rs

            affected packages
            - autotarget_fixture

            changed targets
            - autotarget_fixture[lib]
            - autotarget_fixture[test]

            file contexts `smoke test`
            - autotarget_fixture tests/smoke.rs -> autotarget_fixture[test]
        "#]],
    );
}

#[test]
fn updates_existing_auto_discovered_target_without_full_rebuild() {
    let mut fixture = HostFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "autotarget_update_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Lib;

//- /tests/smoke.rs
#[test]
fn smoke() {}
"#,
    );

    fixture.check_save(
        r#"
//- /tests/smoke.rs
#[test]
fn changed_smoke() {}
"#,
        &[],
        expect![[r#"
            changed files
            - autotarget_update_fixture tests/smoke.rs

            affected packages
            - autotarget_update_fixture

            changed targets
            - autotarget_update_fixture[test]
        "#]],
    );
}

#[test]
fn rebuilds_project_after_lockfile_changes() {
    let mut fixture = HostFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "lock_fixture"
version = "0.1.0"
edition = "2024"

//- /Cargo.lock
# This file is automatically @generated by Cargo.
# It is not intended for manual editing.
version = 3

[[package]]
name = "lock_fixture"
version = "0.1.0"

//- /src/lib.rs
pub struct Lib;
"#,
    );

    fixture.check_save(
        r#"
//- /Cargo.lock
# This file is automatically @generated by Cargo.
# It is not intended for manual editing.
# Saved lockfile change.
version = 3

[[package]]
name = "lock_fixture"
version = "0.1.0"
"#,
        &[],
        expect![[r#"
            changed files
            - <none>

            affected packages
            - lock_fixture

            changed targets
            - lock_fixture[lib]
        "#]],
    );
}

#[test]
fn resolves_lsp_file_contexts_from_paths() {
    let fixture = HostFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "file_context_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod shared;

//- /src/main.rs
mod shared;

fn main() {}

//- /src/shared.rs
pub struct Shared;

//- /src/orphan.rs
pub struct Orphan;
"#,
    );

    fixture.check(
        &[
            HostObservation::file_contexts("lib root", "src/lib.rs"),
            HostObservation::file_contexts("bin root", "src/main.rs"),
            HostObservation::file_contexts("shared module", "src/shared.rs"),
            HostObservation::file_contexts("orphan file", "src/orphan.rs"),
        ],
        expect![[r#"
            file contexts `lib root`
            - file_context_fixture src/lib.rs -> file_context_fixture[lib]

            file contexts `bin root`
            - file_context_fixture src/main.rs -> file_context_fixture[bin]

            file contexts `shared module`
            - file_context_fixture src/shared.rs -> file_context_fixture[bin], file_context_fixture[lib]

            file contexts `orphan file`
            - <none>
        "#]],
    );
}

#[test]
fn rebuilds_package_roots_for_new_saved_module_files() {
    let mut fixture = HostFixture::build(
        r#"
//- /Cargo.toml
[workspace]
members = ["crates/dep", "crates/app"]
resolver = "3"

//- /crates/dep/Cargo.toml
[package]
name = "dep"
version = "0.1.0"
edition = "2024"

//- /crates/dep/src/lib.rs
pub struct Root;

//- /crates/app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "../dep" }

//- /crates/app/src/lib.rs
pub fn use_dep(value: dep::api::Api) {
    let same = val$0ue;
}
"#,
    );

    fixture.check_save(
        r#"
//- /crates/dep/src/lib.rs
pub mod api;
pub struct Root;
"#,
        &[HostObservation::type_names_at("app marker 0", "app", "0")],
        expect![[r#"
            changed files
            - dep crates/dep/src/lib.rs

            affected packages
            - app
            - dep

            changed targets
            - dep[lib]

            type names at `app marker 0`
            - <none>
        "#]],
    );

    fixture.check_save(
        r#"
//- /crates/dep/src/api.rs
pub struct Api;
"#,
        &[HostObservation::type_names_at("app marker 0", "app", "0")],
        expect![[r#"
            changed files
            - dep crates/dep/src/api.rs

            affected packages
            - app
            - dep

            changed targets
            - dep[lib]

            type names at `app marker 0`
            - Api
        "#]],
    );
}

#[test]
fn removes_modules_from_index_after_mod_declarations_are_removed() {
    let mut fixture = HostFixture::build(
        r#"
//- /Cargo.toml
[workspace]
members = ["crates/dep", "crates/app"]
resolver = "3"

//- /crates/dep/Cargo.toml
[package]
name = "dep"
version = "0.1.0"
edition = "2024"

//- /crates/dep/src/lib.rs
pub mod api;
pub struct Root;

//- /crates/dep/src/api.rs
pub struct Api;

//- /crates/app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "../dep" }

//- /crates/app/src/lib.rs
pub fn use_dep(value: dep::api::Api) {
    let same = val$0ue;
}
"#,
    );

    fixture.check(
        &[
            HostObservation::type_names_at("app marker 0", "app", "0"),
            HostObservation::workspace_symbols("Api"),
        ],
        expect![[r#"
            type names at `app marker 0`
            - Api

            workspace symbols `Api`
            - module api @ dep[lib] crates/dep/src/lib.rs
            - struct Api @ dep[lib] crates/dep/src/api.rs
        "#]],
    );

    fixture.check_save(
        r#"
//- /crates/dep/src/lib.rs
pub struct Root;
"#,
        &[
            HostObservation::type_names_at("app marker 0", "app", "0"),
            HostObservation::workspace_symbols("Api"),
        ],
        expect![[r#"
            changed files
            - dep crates/dep/src/lib.rs

            affected packages
            - app
            - dep

            changed targets
            - dep[lib]

            type names at `app marker 0`
            - <none>

            workspace symbols `Api`
            - <none>
        "#]],
    );
}

#[test]
fn reports_reverse_dependent_packages_as_affected() {
    let mut fixture = HostFixture::build(
        r#"
//- /Cargo.toml
[workspace]
members = ["crates/dep", "crates/app"]
resolver = "3"

//- /crates/dep/Cargo.toml
[package]
name = "dep"
version = "0.1.0"
edition = "2024"

//- /crates/dep/src/lib.rs
pub struct Api;

//- /crates/app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "../dep" }

//- /crates/app/src/lib.rs
pub fn use_dep(_: dep::Api) {}
"#,
    );

    fixture.check_save(
        r#"
//- /crates/dep/src/lib.rs
pub struct Api;
pub struct Extra;
"#,
        &[],
        expect![[r#"
            changed files
            - dep crates/dep/src/lib.rs

            affected packages
            - app
            - dep

            changed targets
            - dep[lib]
        "#]],
    );
}

#[test]
fn rebuilds_reverse_dependent_packages_after_dependency_changes() {
    let mut fixture = HostFixture::build(
        r#"
//- /Cargo.toml
[workspace]
members = ["crates/dep", "crates/app"]
resolver = "3"

//- /crates/dep/Cargo.toml
[package]
name = "dep"
version = "0.1.0"
edition = "2024"

//- /crates/dep/src/lib.rs
pub struct Api;

//- /crates/app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "../dep" }

//- /crates/app/src/lib.rs
pub fn use_dep(value: dep::Api) {
    let same = val$0ue;
}
"#,
    );

    fixture.check(
        &[HostObservation::type_names_at("app marker 0", "app", "0")],
        expect![[r#"
            type names at `app marker 0`
            - Api
        "#]],
    );

    fixture.check_save(
        r#"
//- /crates/dep/src/lib.rs
pub struct Renamed;
"#,
        &[HostObservation::type_names_at("app marker 0", "app", "0")],
        expect![[r#"
            changed files
            - dep crates/dep/src/lib.rs

            affected packages
            - app
            - dep

            changed targets
            - dep[lib]

            type names at `app marker 0`
            - <none>
        "#]],
    );
}

#[test]
fn rebuilds_offloaded_path_dependency_after_source_change() {
    let mut fixture = HostFixture::build_with_package_residency_policy(
        r#"
//- /Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "dep" }

//- /src/lib.rs
pub fn use_dep(_: dep::Api) {}

//- /dep/Cargo.toml
[package]
name = "dep"
version = "0.1.0"
edition = "2024"

//- /dep/src/lib.rs
pub struct Api;
"#,
        PackageResidencyPolicy::WorkspaceResident,
    );

    fixture.check(
        &[HostObservation::workspace_symbols("Api")],
        expect![[r#"
            workspace symbols `Api`
            - struct Api @ dep[lib] dep/src/lib.rs
        "#]],
    );

    fixture.check_save(
        r#"
//- /dep/src/lib.rs
pub struct Renamed;
"#,
        &[
            HostObservation::workspace_symbols("Renamed"),
            HostObservation::workspace_symbols("Api"),
        ],
        expect![[r#"
            changed files
            - dep dep/src/lib.rs

            affected packages
            - app
            - dep

            changed targets
            - dep[lib]

            workspace symbols `Renamed`
            - struct Renamed @ dep[lib] dep/src/lib.rs

            workspace symbols `Api`
            - <none>
        "#]],
    );
}

#[test]
fn queries_report_missing_offloaded_package_cache_artifacts() {
    let fixture = HostFixture::build_with_package_residency_policy(
        r#"
//- /Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "dep" }

//- /src/lib.rs
pub fn use_dep(_: dep::Api) {}

//- /dep/Cargo.toml
[package]
name = "dep"
version = "0.1.0"
edition = "2024"

//- /dep/src/lib.rs
pub struct Api;
"#,
        PackageResidencyPolicy::WorkspaceResident,
    );

    assert!(fixture.package_cache_artifact_exists("dep"));
    fixture.remove_package_cache_artifacts();
    assert!(!fixture.package_cache_artifact_exists("dep"));

    let error = fixture.workspace_symbols_error("Api");
    assert!(
        error.contains("offloaded package slot PackageSlot(1) is missing from backing storage"),
        "{error}",
    );
    assert!(!fixture.package_cache_artifact_exists("dep"));
}

#[test]
fn queries_report_corrupt_offloaded_package_cache_artifacts() {
    let fixture = HostFixture::build_with_package_residency_policy(
        r#"
//- /Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "dep" }

//- /src/lib.rs
pub fn use_dep(_: dep::Api) {}

//- /dep/Cargo.toml
[package]
name = "dep"
version = "0.1.0"
edition = "2024"

//- /dep/src/lib.rs
pub struct Api;
"#,
        PackageResidencyPolicy::WorkspaceResident,
    );

    fixture.corrupt_package_cache_artifact("dep");

    let error = fixture.workspace_symbols_error("Api");
    assert!(
        error.contains("offloaded package slot PackageSlot(1) has malformed cache data"),
        "{error}",
    );
    assert!(fixture.package_cache_artifact_exists("dep"));
}

#[test]
fn file_local_queries_do_not_materialize_unrelated_offloaded_packages() {
    let fixture = HostFixture::build_with_package_residency_policy(
        r#"
//- /Cargo.toml
[workspace]
members = ["app", "dep", "unrelated"]
resolver = "3"

//- /app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "../dep" }

//- /app/src/lib.rs
pub struct Local;
pub fn use_dep(_: dep::Api) {}

//- /dep/Cargo.toml
[package]
name = "dep"
version = "0.1.0"
edition = "2024"

//- /dep/src/lib.rs
pub struct Api;

//- /unrelated/Cargo.toml
[package]
name = "unrelated"
version = "0.1.0"
edition = "2024"

//- /unrelated/src/lib.rs
pub struct Unrelated;
"#,
        PackageResidencyPolicy::AllOffloadable,
    );

    assert!(fixture.package_cache_artifact_exists("unrelated"));
    fixture.remove_package_cache_artifact("unrelated");
    assert!(!fixture.package_cache_artifact_exists("unrelated"));

    assert_eq!(
        fixture.document_symbol_names("app/src/lib.rs"),
        vec!["Local", "use_dep"],
    );
    assert!(
        !fixture.package_cache_artifact_exists("unrelated"),
        "narrow file-local queries should not recover artifacts outside their package subset",
    );
}

#[test]
fn source_updates_do_not_materialize_unrelated_offloaded_packages() {
    let mut fixture = HostFixture::build_with_package_residency_policy(
        r#"
//- /Cargo.toml
[workspace]
members = ["app", "dep", "unrelated"]
resolver = "3"

//- /app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "../dep" }

//- /app/src/lib.rs
pub struct Before;
pub fn use_dep(_: dep::Api) {}

//- /dep/Cargo.toml
[package]
name = "dep"
version = "0.1.0"
edition = "2024"

//- /dep/src/lib.rs
pub struct Api;

//- /unrelated/Cargo.toml
[package]
name = "unrelated"
version = "0.1.0"
edition = "2024"

//- /unrelated/src/lib.rs
pub struct Unrelated;
"#,
        PackageResidencyPolicy::AllOffloadable,
    );

    assert!(fixture.package_cache_artifact_exists("unrelated"));
    fixture.remove_package_cache_artifact("unrelated");
    assert!(!fixture.package_cache_artifact_exists("unrelated"));

    fixture.check_save(
        r#"
//- /app/src/lib.rs
pub struct After;
pub fn use_dep(_: dep::Api) {}
"#,
        &[HostObservation::resident_stats("after save")],
        expect![[r#"
            changed files
            - app app/src/lib.rs

            affected packages
            - app

            changed targets
            - app[lib]

            resident stats `after save`
            - def-map crates 0
            - semantic crates 0
            - body crates 0
        "#]],
    );

    assert_eq!(
        fixture.document_symbol_names("app/src/lib.rs"),
        vec!["After", "use_dep"],
    );
    assert!(
        !fixture.package_cache_artifact_exists("unrelated"),
        "source updates should not recover artifacts outside their rebuild package subset",
    );
}

#[test]
fn source_updates_rebuild_missing_offloaded_package_cache_artifacts() {
    let mut fixture = HostFixture::build_with_package_residency_policy(
        r#"
//- /Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "dep" }

//- /src/lib.rs
use dep::Api;
pub struct Before;
pub fn use_dep(_: Api) {}

//- /dep/Cargo.toml
[package]
name = "dep"
version = "0.1.0"
edition = "2024"

//- /dep/src/lib.rs
pub struct Api;
"#,
        PackageResidencyPolicy::WorkspaceResident,
    );

    fixture.remove_package_cache_artifacts();

    fixture.check_save(
        r#"
//- /src/lib.rs
use dep::Api;
pub struct After;
pub fn use_dep(_: Api) {}
"#,
        &[
            HostObservation::workspace_symbols("After"),
            HostObservation::workspace_symbols("Api"),
        ],
        expect![[r#"
            changed files
            - app src/lib.rs

            affected packages
            - app

            changed targets
            - app[lib]

            workspace symbols `After`
            - struct After @ app[lib] src/lib.rs

            workspace symbols `Api`
            - struct Api @ dep[lib] dep/src/lib.rs
        "#]],
    );
    assert!(fixture.package_cache_artifact_exists("dep"));
}

#[test]
fn source_updates_rebuild_corrupt_offloaded_package_cache_artifacts() {
    let mut fixture = HostFixture::build_with_package_residency_policy(
        r#"
//- /Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "dep" }

//- /src/lib.rs
use dep::Api;
pub struct Before;
pub fn use_dep(_: Api) {}

//- /dep/Cargo.toml
[package]
name = "dep"
version = "0.1.0"
edition = "2024"

//- /dep/src/lib.rs
pub struct Api;
"#,
        PackageResidencyPolicy::WorkspaceResident,
    );

    fixture.corrupt_package_cache_artifact("dep");

    fixture.check_save(
        r#"
//- /src/lib.rs
use dep::Api;
pub struct After;
pub fn use_dep(_: Api) {}
"#,
        &[
            HostObservation::workspace_symbols("After"),
            HostObservation::workspace_symbols("Api"),
        ],
        expect![[r#"
            changed files
            - app src/lib.rs

            affected packages
            - app

            changed targets
            - app[lib]

            workspace symbols `After`
            - struct After @ app[lib] src/lib.rs

            workspace symbols `Api`
            - struct Api @ dep[lib] dep/src/lib.rs
        "#]],
    );
    assert!(fixture.package_cache_artifact_exists("dep"));
}

#[test]
fn source_updates_restore_offloaded_residency_for_unchanged_packages() {
    let mut fixture = HostFixture::build_with_package_residency_policy(
        r#"
//- /Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "dep" }

//- /src/lib.rs
pub struct Before;
pub fn use_dep(_: dep::Api) {}

//- /dep/Cargo.toml
[package]
name = "dep"
version = "0.1.0"
edition = "2024"

//- /dep/src/lib.rs
pub struct Api;
"#,
        PackageResidencyPolicy::AllOffloadable,
    );

    fixture.check(
        &[HostObservation::resident_stats("after build")],
        expect![[r#"
            resident stats `after build`
            - def-map crates 0
            - semantic crates 0
            - body crates 0
        "#]],
    );

    fixture.check_save(
        r#"
//- /src/lib.rs
pub struct After;
pub fn use_dep(_: dep::Api) {}
"#,
        &[
            HostObservation::resident_stats("after save"),
            HostObservation::workspace_symbols("After"),
        ],
        expect![[r#"
            changed files
            - app src/lib.rs

            affected packages
            - app

            changed targets
            - app[lib]

            resident stats `after save`
            - def-map crates 0
            - semantic crates 0
            - body crates 0

            workspace symbols `After`
            - struct After @ app[lib] src/lib.rs
        "#]],
    );
}

#[test]
fn rebuilds_transitive_reverse_dependent_packages_after_dependency_changes() {
    let mut fixture = HostFixture::build(
        r#"
//- /Cargo.toml
[workspace]
members = ["crates/dep", "crates/mid", "crates/app"]
resolver = "3"

//- /crates/dep/Cargo.toml
[package]
name = "dep"
version = "0.1.0"
edition = "2024"

//- /crates/dep/src/lib.rs
pub struct Api;

//- /crates/mid/Cargo.toml
[package]
name = "mid"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "../dep" }

//- /crates/mid/src/lib.rs
pub fn make() -> dep::Api {
    loop {}
}

//- /crates/app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
mid = { path = "../mid" }

//- /crates/app/src/lib.rs
pub fn use_mid() {
    let value = mid::make();
    let same = val$0ue;
}
"#,
    );

    fixture.check(
        &[HostObservation::type_names_at("app marker 0", "app", "0")],
        expect![[r#"
            type names at `app marker 0`
            - Api
        "#]],
    );

    fixture.check_save(
        r#"
//- /crates/dep/src/lib.rs
pub struct Renamed;
"#,
        &[HostObservation::type_names_at("app marker 0", "app", "0")],
        expect![[r#"
            changed files
            - dep crates/dep/src/lib.rs

            affected packages
            - app
            - dep
            - mid

            changed targets
            - dep[lib]

            type names at `app marker 0`
            - <none>
        "#]],
    );
}
