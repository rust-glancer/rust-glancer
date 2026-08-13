use std::{
    path::PathBuf,
    sync::{Arc, mpsc},
    time::Duration,
};

use rg_lsp_proto::{
    AnalysisConfig, DocumentRevision, EditorDocumentSnapshot, EditorSnapshot,
    EditorSnapshotRevision, OpenDocumentSession, PackageResidencyPolicy, SysrootDiscovery,
};
use rg_project::{AnalysisSurface, FileContext, SavedFileChange, SourceOverrideScope};
use test_fixture::{CrateFixture, fixture_crate};

use super::NoopNotifications;
use crate::{
    engine::{
        command::EngineCommand,
        project::{ProjectConfiguration, ProjectCoordinator},
    },
    memory::MemoryControl,
    service::ServiceNotificationsSink,
};

struct SourceOverrideFixture {
    fixture: CrateFixture,
    source: PathBuf,
    clean_text: String,
    context: FileContext,
    project: ProjectCoordinator,
}

impl SourceOverrideFixture {
    /// Build a fully indexed saved project so each test can focus on editor selection behavior.
    fn new() -> Self {
        let fixture = fixture_crate(
            r#"
                //- /Cargo.toml
                [package]
                name = "source_override_fixture"
                version = "0.1.0"
                edition = "2024"

                //- /src/lib.rs
                pub fn saved() -> usize {
                    1
                }
                "#,
        );
        let source = fixture.path("src/lib.rs");
        let clean_text = std::fs::read_to_string(&source)
            .expect("fixture source should be readable before project initialization");
        let (sender, receiver) = mpsc::channel();
        let memory_control: Arc<dyn MemoryControl> = Arc::new(());
        let notifications = ServiceNotificationsSink::from_publisher(NoopNotifications);
        let mut project = ProjectCoordinator::new(sender, memory_control, notifications);
        project
            .initialize(
                fixture.path(""),
                ProjectConfiguration::from(AnalysisConfig {
                    package_residency_policy: PackageResidencyPolicy::AllOffloadable,
                    sysroot_discovery: SysrootDiscovery::Disabled,
                    ..AnalysisConfig::default()
                }),
            )
            .expect("fixture project should initialize");

        // Merge the deferred payload so cache tests do not also exercise query materialization.
        let initial = receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("initial deferred indexing should return");
        let EngineCommand::DeferredIndexingFinished { generation, result } = initial.command else {
            panic!("initial background command should finish deferred indexing");
        };
        project.deferred_indexing_finished(generation, result);

        let context = project
            .saved_snapshot()
            .expect("saved project should be available")
            .file_contexts_for_path(&source)
            .expect("fixture file contexts should resolve")
            .pop()
            .expect("fixture file should have one context");

        Self {
            fixture,
            source,
            clean_text,
            context,
            project,
        }
    }

    fn snapshot(&self, revision: u64, text: &str) -> EditorSnapshot {
        editor_snapshot(self.source.clone(), revision, text)
    }

    fn query(&mut self, editor: &EditorSnapshot, scope: SourceOverrideScope) {
        self.project
            .with_query_snapshot(Some((editor, scope)), |_| Ok(()))
            .expect("editor query should select a project");
    }

    fn rebuild_count(&self) -> usize {
        self.project.project.source_override_rebuild_count()
    }
}

#[test]
fn clean_editor_selection_expires_with_its_saved_source_bytes() {
    let mut fixture = SourceOverrideFixture::new();
    let clean = fixture.snapshot(1, &fixture.clean_text);
    let before_selection = fixture.rebuild_count();

    // Clean editor text selects the saved project. Because no derived project is retained, that
    // comparison can satisfy either query scope without another rebuild.
    fixture.query(&clean, SourceOverrideScope::ChangedPackages);
    assert!(!fixture.project.project.has_cached_override_project());
    assert_eq!(fixture.rebuild_count(), before_selection + 1);
    fixture.query(&clean, SourceOverrideScope::ReverseDependencyClosure);
    assert_eq!(fixture.rebuild_count(), before_selection + 1);

    // Request cleanup also releases saved source text. The clean selection must expire with that
    // premise so the same immutable editor snapshot can restore its bytes on the next request.
    fixture.project.release_query_memory();
    std::fs::remove_file(&fixture.source)
        .expect("clean open source should disappear after the first request");
    let before_restore = fixture.rebuild_count();
    let restored_text = fixture
        .project
        .with_query_snapshot(
            Some((&clean, SourceOverrideScope::ChangedPackages)),
            |snapshot| {
                snapshot
                    .file_source_text(fixture.context.package, fixture.context.file)
                    .map(|text| text.expect("known fixture source should remain queryable"))
            },
        )
        .expect("same-revision clean editor query should restore its captured bytes");
    assert_eq!(restored_text.as_ref(), fixture.clean_text);
    assert_eq!(fixture.rebuild_count(), before_restore + 1);
}

#[test]
fn source_override_selection_survives_materialization_and_reuses_broader_scope() {
    let mut fixture = SourceOverrideFixture::new();
    let dirty = fixture.snapshot(2, "pub fn dirty() -> usize { 2 }\n");
    let files = [(fixture.context.package, fixture.context.file)];
    let crates = fixture.context.crates.clone();
    let before_local = fixture.rebuild_count();

    // A ready project needs no materialization work. Preparing different query surfaces must
    // continue using the same source-override project selected by the first query.
    fixture.query(&dirty, SourceOverrideScope::ChangedPackages);
    fixture
        .project
        .materialize_query_project(
            &dirty,
            SourceOverrideScope::ChangedPackages,
            AnalysisSurface::Files(&files),
        )
        .expect("file query surface should already be ready");
    fixture
        .project
        .materialize_query_project(
            &dirty,
            SourceOverrideScope::ChangedPackages,
            AnalysisSurface::FilesAndCrates {
                files: &files,
                crates: &crates,
            },
        )
        .expect("mixed query surface should already be ready");
    fixture.query(&dirty, SourceOverrideScope::ChangedPackages);
    assert_eq!(fixture.rebuild_count(), before_local + 1);

    // Widening the scope replaces the local project once. That broader project already contains
    // the changed packages, so a later local query can reuse it.
    let before_widening = fixture.rebuild_count();
    fixture.query(&dirty, SourceOverrideScope::ReverseDependencyClosure);
    assert_eq!(fixture.rebuild_count(), before_widening + 1);
    fixture.query(&dirty, SourceOverrideScope::ChangedPackages);
    assert_eq!(fixture.rebuild_count(), before_widening + 1);
}

#[test]
fn source_override_cache_is_keyed_by_editor_and_saved_revisions() {
    let mut fixture = SourceOverrideFixture::new();
    let dirty = fixture.snapshot(2, "pub fn dirty() -> usize { 2 }\n");
    fixture.query(&dirty, SourceOverrideScope::ReverseDependencyClosure);

    let next_editor = fixture.snapshot(3, "pub fn next_editor() -> usize { 3 }\n");
    let before_editor_change = fixture.rebuild_count();
    fixture.query(&next_editor, SourceOverrideScope::ReverseDependencyClosure);
    assert_eq!(fixture.rebuild_count(), before_editor_change + 1);

    std::fs::write(&fixture.source, "pub fn newer_saved() -> usize { 4 }\n")
        .expect("fixture saved source should be writable");
    fixture
        .project
        .saved_project_changes(vec![SavedFileChange::fs_path(&fixture.source)])
        .expect("saved generation should advance");
    let before_saved_change = fixture.rebuild_count();
    fixture.query(&next_editor, SourceOverrideScope::ReverseDependencyClosure);
    assert_eq!(fixture.rebuild_count(), before_saved_change + 1);
}

#[test]
fn unknown_editor_source_replaces_a_prior_derived_selection() {
    let mut fixture = SourceOverrideFixture::new();
    let dirty = fixture.snapshot(2, "pub fn dirty() -> usize { 2 }\n");
    fixture.query(&dirty, SourceOverrideScope::ChangedPackages);
    assert!(fixture.project.project.has_cached_override_project());

    // An open path outside the frozen source generation contributes no captured source. Selecting
    // it succeeds with the saved project and must not leave the previous derived project hidden.
    let unknown = editor_snapshot(
        fixture.fixture.path("src/does-not-exist.rs"),
        3,
        "pub fn missing() {}\n",
    );
    fixture.query(&unknown, SourceOverrideScope::ChangedPackages);
    assert!(!fixture.project.project.has_cached_override_project());
}

#[test]
fn incomplete_editor_query_materializes_its_selected_source_override_project() {
    let fixture = fixture_crate(
        r#"
            //- /Cargo.toml
            [package]
            name = "incomplete_source_override_materialization_fixture"
            version = "0.1.0"
            edition = "2024"

            //- /src/lib.rs
            pub fn saved() -> usize {
                1
            }
            "#,
    );
    let source = fixture.path("src/lib.rs");
    let (sender, receiver) = mpsc::channel();
    let memory_control: Arc<dyn MemoryControl> = Arc::new(());
    let notifications = ServiceNotificationsSink::from_publisher(NoopNotifications);
    let mut project = ProjectCoordinator::new(sender, memory_control, notifications);
    project
        .initialize(
            fixture.path(""),
            ProjectConfiguration::from(AnalysisConfig {
                package_residency_policy: PackageResidencyPolicy::AllResident,
                sysroot_discovery: SysrootDiscovery::Disabled,
                ..AnalysisConfig::default()
            }),
        )
        .expect("fixture project should initialize");

    // Let background indexing finish, but hold its result outside the coordinator. The saved
    // project therefore remains deterministically incomplete while no worker is still running.
    let initial = receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("initial deferred indexing should return");
    let EngineCommand::DeferredIndexingFinished { generation, result } = initial.command else {
        panic!("initial background command should finish deferred indexing");
    };
    let incomplete_stats = project
        .saved_snapshot()
        .expect("saved project should be available")
        .stats()
        .body_ir;
    assert_eq!(incomplete_stats.complete_crate_count, 0);
    assert_eq!(incomplete_stats.missing_crate_count, 1);

    let context = project
        .saved_snapshot()
        .expect("saved project should be available")
        .file_contexts_for_path(&source)
        .expect("fixture file contexts should resolve")
        .pop()
        .expect("fixture file should have one context");
    let files = [(context.package, context.file)];
    let dirty = editor_snapshot(source.clone(), 2, "pub fn dirty() -> usize { 2 }\n");
    let saved_text = std::fs::read_to_string(&source).expect("fixture source should be readable");
    std::fs::remove_file(&source)
        .expect("fixture source should disappear after its editor identity was captured");

    // Query preparation fills the project selected from this exact editor snapshot. It must not
    // reload the open source from disk or construct a second project for materialization.
    project
        .materialize_query_project(
            &dirty,
            SourceOverrideScope::ChangedPackages,
            AnalysisSurface::Files(&files),
        )
        .expect("incomplete editor query surface should materialize");
    project
        .with_query_snapshot(Some((&dirty, SourceOverrideScope::ChangedPackages)), |_| {
            Ok(())
        })
        .expect("second query should reuse its selected source-override project");
    assert_eq!(
        project.project.source_override_rebuild_count(),
        1,
        "materializing the selected source-override project should not rebuild it",
    );
    let saved_stats = project
        .saved_snapshot()
        .expect("saved project should remain available")
        .stats()
        .body_ir;
    assert_eq!(saved_stats.complete_crate_count, 0);
    assert_eq!(saved_stats.missing_crate_count, 1);

    std::fs::write(&source, saved_text)
        .expect("fixture saved source should be restored before background merge");
    project.deferred_indexing_finished(generation, result);
}

fn editor_snapshot(path: PathBuf, revision: u64, text: &str) -> EditorSnapshot {
    let source_path = path.canonicalize().unwrap_or_else(|_| path.clone());
    EditorSnapshot::new(
        EditorSnapshotRevision::new(revision),
        vec![
            EditorDocumentSnapshot::new(
                path,
                OpenDocumentSession::new(1),
                DocumentRevision::new(revision),
                i32::try_from(revision).ok(),
                text.to_string(),
            )
            .with_source_path(source_path),
        ],
    )
}
