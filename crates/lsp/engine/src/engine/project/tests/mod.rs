use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    time::Duration,
};

use rg_lsp_proto::{AnalysisConfig, PackageResidencyPolicy, ServiceNotification, SysrootDiscovery};
use rg_project::{AnalysisSurface, DirtyOverlayScope, ProjectMemoryHooks, ProjectMemoryPurgePoint};
use test_fixture::fixture_crate;

use super::{MAX_STALE_SOURCE_RETRIES, ProjectConfiguration, ProjectCoordinator};
use crate::{
    documents::{DirtyDocumentSnapshotState, DocumentStore},
    engine::command::EngineCommand,
    memory::MemoryControl,
    service::{ServiceNotificationPublisher, ServiceNotificationsSink},
};

#[derive(Debug, Default)]
struct DirtyOverlayBuilds {
    count: AtomicUsize,
}

impl DirtyOverlayBuilds {
    fn count(&self) -> usize {
        self.count.load(Ordering::Acquire)
    }
}

impl ProjectMemoryHooks for DirtyOverlayBuilds {
    fn purge(&self, point: ProjectMemoryPurgePoint) {
        if point == ProjectMemoryPurgePoint::AfterDirtyOverlayBuild {
            self.count.fetch_add(1, Ordering::AcqRel);
        }
    }
}

#[derive(Debug)]
struct SourceMutations {
    remaining: AtomicUsize,
    path: std::path::PathBuf,
}

impl ProjectMemoryHooks for SourceMutations {
    fn purge(&self, point: ProjectMemoryPurgePoint) {
        if point != ProjectMemoryPurgePoint::AfterItemTreeSyntaxEviction {
            return;
        }

        let Ok(previous) =
            self.remaining
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |remaining| {
                    remaining.checked_sub(1)
                })
        else {
            return;
        };
        std::fs::write(&self.path, format!("pub struct Concurrent{previous};\n"))
            .expect("source mutation hook should replace fixture source");
    }
}

#[derive(Debug)]
struct SourceBurst {
    armed: AtomicBool,
    attempts: AtomicUsize,
    replacements: Vec<(std::path::PathBuf, &'static str)>,
}

#[derive(Debug, Default)]
struct RecordingMemoryHooks {
    points: Mutex<Vec<ProjectMemoryPurgePoint>>,
}

impl RecordingMemoryHooks {
    fn take(&self) -> Vec<ProjectMemoryPurgePoint> {
        std::mem::take(
            &mut *self
                .points
                .lock()
                .expect("recorded memory hook points should not be poisoned"),
        )
    }
}

impl ProjectMemoryHooks for RecordingMemoryHooks {
    fn purge(&self, point: ProjectMemoryPurgePoint) {
        self.points
            .lock()
            .expect("recorded memory hook points should not be poisoned")
            .push(point);
    }
}

impl ProjectMemoryHooks for SourceBurst {
    fn purge(&self, point: ProjectMemoryPurgePoint) {
        if point != ProjectMemoryPurgePoint::AfterItemTreeSyntaxEviction {
            return;
        }
        self.attempts.fetch_add(1, Ordering::AcqRel);
        if !self.armed.swap(false, Ordering::AcqRel) {
            return;
        }

        for (path, replacement) in &self.replacements {
            std::fs::write(path, replacement)
                .expect("source burst hook should replace fixture source");
        }
    }
}

#[derive(Debug)]
struct NoopNotifications;

impl ServiceNotificationPublisher for NoopNotifications {
    fn send(&self, _notification: ServiceNotification) {}
}

#[derive(Clone, Debug, Default)]
struct RecordingNotifications {
    notifications: Arc<Mutex<Vec<ServiceNotification>>>,
}

impl RecordingNotifications {
    fn take(&self) -> Vec<ServiceNotification> {
        std::mem::take(
            &mut *self
                .notifications
                .lock()
                .expect("recorded notifications should not be poisoned"),
        )
    }
}

impl ServiceNotificationPublisher for RecordingNotifications {
    fn send(&self, notification: ServiceNotification) {
        self.notifications
            .lock()
            .expect("recorded notifications should not be poisoned")
            .push(notification);
    }
}

#[test]
fn ready_query_surfaces_and_sufficient_scope_preserve_the_matching_dirty_overlay() {
    let fixture = fixture_crate(
        r#"
            //- /Cargo.toml
            [package]
            name = "dirty_overlay_materialization_fixture"
            version = "0.1.0"
            edition = "2024"

            //- /src/lib.rs
            pub fn saved() -> usize {
                1
            }
            "#,
    );
    let source = fixture.path("src/lib.rs");
    let hooks = Arc::new(DirtyOverlayBuilds::default());
    let (sender, receiver) = mpsc::channel();
    let memory_control: Arc<dyn MemoryControl> = Arc::new(());
    let notifications = ServiceNotificationsSink::from_publisher(NoopNotifications);
    let mut project = ProjectCoordinator::new(sender, memory_control, notifications);
    project.memory_hooks = hooks.clone();
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

    // Finish and merge the deferred payload so all-offloadable queries can read its durable
    // artifact without replacing saved project state.
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
    let files = [(context.package, context.file)];
    let crates = context.crates;
    let saved_text = std::fs::read_to_string(&source).expect("saved fixture source should read");
    let mut documents = DocumentStore::default();
    documents.did_open_saved(source.clone(), Some(1), &saved_text);
    documents.did_change(
        source.clone(),
        Some(2),
        Some("pub fn dirty() -> usize { 2 }\n"),
    );
    let DirtyDocumentSnapshotState::Dirty(dirty) = documents.dirty_snapshot(&source) else {
        panic!("changed fixture document should have a dirty text snapshot");
    };

    // Document symbols can build the overlay before a body-backed query prepares its surface.
    // File and mixed reference-search preparation are both no-ops for finished artifacts, so the
    // same dirty snapshot must keep hitting one cached overlay across those query shapes.
    project
        .with_query_snapshot(Some(&dirty), DirtyOverlayScope::ChangedPackages, |_| Ok(()))
        .expect("first dirty query should build its overlay");
    project
        .materialize(AnalysisSurface::Files(&files))
        .expect("file query surface should already be ready");
    project
        .with_query_snapshot(Some(&dirty), DirtyOverlayScope::ChangedPackages, |_| Ok(()))
        .expect("second dirty query should reuse its overlay");
    project
        .materialize(AnalysisSurface::FilesAndCrates {
            files: &files,
            crates: &crates,
        })
        .expect("mixed query surface should already be ready");
    project
        .with_query_snapshot(Some(&dirty), DirtyOverlayScope::ChangedPackages, |_| Ok(()))
        .expect("third dirty query should reuse its overlay");

    assert_eq!(
        hooks.count(),
        1,
        "no-op query materialization should not evict a matching dirty overlay",
    );

    // A broader query must replace a local overlay. Once built, that broader overlay contains the
    // changed packages too, so a later local query can reuse it without a second replacement.
    project
        .with_query_snapshot(
            Some(&dirty),
            DirtyOverlayScope::ReverseDependencyClosure,
            |_| Ok(()),
        )
        .expect("workspace-wide dirty query should upgrade its overlay");
    assert_eq!(hooks.count(), 2, "broader scope should rebuild once");
    project
        .with_query_snapshot(Some(&dirty), DirtyOverlayScope::ChangedPackages, |_| Ok(()))
        .expect("local dirty query should reuse a broader overlay");
    assert_eq!(
        hooks.count(),
        2,
        "broader overlay should satisfy a later local query",
    );
}

#[test]
fn incomplete_resident_query_surface_evicts_the_matching_dirty_overlay() {
    let fixture = fixture_crate(
        r#"
            //- /Cargo.toml
            [package]
            name = "incomplete_dirty_overlay_materialization_fixture"
            version = "0.1.0"
            edition = "2024"

            //- /src/lib.rs
            pub fn saved() -> usize {
                1
            }
            "#,
    );
    let source = fixture.path("src/lib.rs");
    let hooks = Arc::new(DirtyOverlayBuilds::default());
    let (sender, receiver) = mpsc::channel();
    let memory_control: Arc<dyn MemoryControl> = Arc::new(());
    let notifications = ServiceNotificationsSink::from_publisher(NoopNotifications);
    let mut project = ProjectCoordinator::new(sender, memory_control, notifications);
    project.memory_hooks = hooks.clone();
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
    let saved_text = std::fs::read_to_string(&source).expect("saved fixture source should read");
    let mut documents = DocumentStore::default();
    documents.did_open_saved(source.clone(), Some(1), &saved_text);
    documents.did_change(
        source.clone(),
        Some(2),
        Some("pub fn dirty() -> usize { 2 }\n"),
    );
    let DirtyDocumentSnapshotState::Dirty(dirty) = documents.dirty_snapshot(&source) else {
        panic!("changed fixture document should have a dirty text snapshot");
    };

    project
        .with_query_snapshot(Some(&dirty), DirtyOverlayScope::ChangedPackages, |_| Ok(()))
        .expect("first dirty query should build its overlay");
    assert_eq!(hooks.count(), 1, "first dirty query should build once");

    // This preparation fills missing resident Body IR and replaces saved package payloads. The
    // overlay based on the old payload must be dropped before the next dirty query can run.
    project
        .materialize(AnalysisSurface::Files(&files))
        .expect("incomplete file query surface should materialize");
    project
        .with_query_snapshot(Some(&dirty), DirtyOverlayScope::ChangedPackages, |_| Ok(()))
        .expect("second dirty query should rebuild its evicted overlay");
    assert_eq!(
        hooks.count(),
        2,
        "real query materialization should evict a matching dirty overlay",
    );

    project.deferred_indexing_finished(generation, result);
}

#[test]
fn deferred_lifecycle_tracks_published_generations_not_foreground_activity() {
    let fixture = fixture_crate(
        r#"
            //- /Cargo.toml
            [package]
            name = "deferred_lifecycle_fixture"
            version = "0.1.0"
            edition = "2024"

            //- /src/lib.rs
            pub fn published() -> usize {
                1
            }
            "#,
    );
    let source = fixture.path("src/lib.rs");
    let (sender, receiver) = mpsc::channel();
    let memory_control: Arc<dyn MemoryControl> = Arc::new(());
    let recorded = RecordingNotifications::default();
    let notifications = ServiceNotificationsSink::from_publisher(recorded.clone());
    let mut project = ProjectCoordinator::new(sender, memory_control, notifications);
    let memory_hooks = Arc::new(RecordingMemoryHooks::default());
    project.memory_hooks = memory_hooks.clone();
    project
        .initialize(
            fixture.path(""),
            ProjectConfiguration::from(AnalysisConfig {
                sysroot_discovery: SysrootDiscovery::Disabled,
                ..AnalysisConfig::default()
            }),
        )
        .expect("fixture project should initialize");
    assert!(matches!(
        recorded.take().as_slice(),
        [ServiceNotification::DeferredIndexingStarted { .. }]
    ));

    let initial = receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("initial deferred indexing should return");
    let EngineCommand::DeferredIndexingFinished { generation, result } = initial.command else {
        panic!("initial background command should finish deferred indexing");
    };
    let _ = memory_hooks.take();
    project.deferred_indexing_finished(generation, result);
    assert_eq!(
        memory_hooks.take().last(),
        Some(&ProjectMemoryPurgePoint::AfterDeferredIndexingFinish),
        "the detached result should die before the final deferred-indexing purge",
    );
    assert!(matches!(
        recorded.take().as_slice(),
        [ServiceNotification::DeferredIndexingFinished { .. }]
    ));

    project
        .project_paths_changed(vec![source.clone()])
        .expect("an unchanged watcher replay should be accepted");
    assert!(
        recorded.take().is_empty(),
        "an unchanged foreground cycle should not invent deferred work",
    );

    std::fs::write(&source, "pub fn updated() -> usize { 2 }\n")
        .expect("fixture source should be replaced");
    project
        .project_paths_changed(vec![source])
        .expect("saved source change should be applied");
    assert!(matches!(
        recorded.take().as_slice(),
        [ServiceNotification::DeferredIndexingStarted { .. }]
    ));
    let foreground_stats = project
        .saved_snapshot()
        .expect("updated saved project should be available")
        .stats()
        .body_ir;
    assert_eq!(foreground_stats.complete_crate_count, 0);
    assert_eq!(foreground_stats.missing_crate_count, 1);
    assert_eq!(foreground_stats.body_count, 0);

    let updated = receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("updated deferred indexing should return");
    let EngineCommand::DeferredIndexingFinished { generation, result } = updated.command else {
        panic!("updated background command should finish deferred indexing");
    };
    project.deferred_indexing_finished(generation, result);
    assert!(matches!(
        recorded.take().as_slice(),
        [ServiceNotification::DeferredIndexingFinished { .. }]
    ));
    let finished_stats = project
        .saved_snapshot()
        .expect("finished saved project should be available")
        .stats()
        .body_ir;
    assert_eq!(finished_stats.complete_crate_count, 1);
    assert_eq!(finished_stats.missing_crate_count, 0);
    assert_eq!(finished_stats.body_count, 1);
}

#[test]
fn project_path_change_retries_source_races_but_preserves_a_finite_lane_budget() {
    let fixture = fixture_crate(
        r#"
            //- /Cargo.toml
            [package]
            name = "stale_source_retry_fixture"
            version = "0.1.0"
            edition = "2024"

            //- /src/lib.rs
            pub struct Published;
            "#,
    );
    let source = fixture.path("src/lib.rs");
    let hooks = Arc::new(SourceMutations {
        remaining: AtomicUsize::new(0),
        path: source.clone(),
    });
    let (sender, receiver) = mpsc::channel();
    let memory_control: Arc<dyn MemoryControl> = Arc::new(());
    let notifications = ServiceNotificationsSink::from_publisher(NoopNotifications);
    let mut project = ProjectCoordinator::new(sender, memory_control, notifications);
    project.memory_hooks = hooks.clone();
    project
        .initialize(
            fixture.path(""),
            ProjectConfiguration::from(AnalysisConfig {
                sysroot_discovery: SysrootDiscovery::Disabled,
                ..AnalysisConfig::default()
            }),
        )
        .expect("fixture project should initialize");

    // Reconcile initial deferred indexing so any command observed below belongs to the update.
    let initial = receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("initial deferred indexing should return");
    let EngineCommand::DeferredIndexingFinished { generation, result } = initial.command else {
        panic!("initial background command should finish deferred indexing");
    };
    project.deferred_indexing_finished(generation, result);

    std::fs::write(&source, "pub struct Candidate;\n")
        .expect("candidate fixture source should be written");
    // Two writes force the replacement candidate itself to become stale once more. Recovery
    // must remain indexing until the third attempt captures a quiet generation.
    hooks.remaining.store(2, Ordering::Release);
    project
        .project_paths_changed(vec![source.clone()])
        .expect("stale candidate should be retried from the newer disk revision");

    let deferred = receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("successful update should start deferred indexing");
    let EngineCommand::DeferredIndexingFinished { generation, result } = deferred.command else {
        panic!("updated background command should finish deferred indexing");
    };
    assert_eq!(
        generation,
        project.project.generation(),
        "a rejected candidate must not start deferred work for the old generation"
    );
    project.deferred_indexing_finished(generation, result);

    // A continuously rewritten source must eventually return control to the dispatcher. Every
    // failed attempt leaves the published generation untouched, so the next watcher batch can
    // safely retry after this command reports the race.
    let published_generation = project.project.generation();
    std::fs::write(&source, "pub struct NeverSettles;\n")
        .expect("unsettled fixture source should be written");
    hooks
        .remaining
        .store(MAX_STALE_SOURCE_RETRIES + 1, Ordering::Release);
    let error = project
        .project_paths_changed(vec![source])
        .expect_err("continuous source changes should exhaust the retry limit");

    assert!(
        rg_project::Project::stale_source_path(&error).is_some(),
        "the exhausted command should retain its typed stale-source cause"
    );
    assert_eq!(
        project.project.generation(),
        published_generation,
        "exhausted retries must preserve the last coherent generation"
    );
    assert!(
        receiver.try_recv().is_err(),
        "rejected candidates must not schedule deferred indexing"
    );
}

#[test]
fn project_path_retry_collects_the_settled_source_burst() {
    let fixture = fixture_crate(
        r#"
            //- /Cargo.toml
            [package]
            name = "stale_source_burst_fixture"
            version = "0.1.0"
            edition = "2024"

            //- /src/lib.rs
            mod account;
            mod user;
            pub struct Published;

            //- /src/account.rs
            pub struct Account;

            //- /src/user.rs
            pub struct User;
            "#,
    );
    let root = fixture.path("src/lib.rs");
    let hooks = Arc::new(SourceBurst {
        armed: AtomicBool::new(false),
        attempts: AtomicUsize::new(0),
        replacements: vec![
            (fixture.path("src/account.rs"), "pub struct SavedAccount;\n"),
            (fixture.path("src/user.rs"), "pub struct SavedUser;\n"),
        ],
    });
    let (sender, receiver) = mpsc::channel();
    let memory_control: Arc<dyn MemoryControl> = Arc::new(());
    let notifications = ServiceNotificationsSink::from_publisher(NoopNotifications);
    let mut project = ProjectCoordinator::new(sender, memory_control, notifications);
    project.memory_hooks = hooks.clone();
    project
        .initialize(
            fixture.path(""),
            ProjectConfiguration::from(AnalysisConfig {
                sysroot_discovery: SysrootDiscovery::Disabled,
                ..AnalysisConfig::default()
            }),
        )
        .expect("fixture project should initialize");

    let initial = receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("initial deferred indexing should return");
    let EngineCommand::DeferredIndexingFinished { generation, result } = initial.command else {
        panic!("initial background command should finish deferred indexing");
    };
    project.deferred_indexing_finished(generation, result);

    let published_generation = project.project.generation();
    project
        .project_paths_changed(vec![root.clone()])
        .expect("an unchanged watcher replay should be accepted");
    assert_eq!(
        project.project.generation(),
        published_generation,
        "an unchanged watcher replay should preserve saved generation identity",
    );
    assert!(
        receiver.try_recv().is_err(),
        "an unchanged watcher replay should not schedule deferred indexing",
    );

    hooks.attempts.store(0, Ordering::Release);
    hooks.armed.store(true, Ordering::Release);
    std::fs::write(&root, "mod account;\nmod user;\npub struct Candidate;\n")
        .expect("candidate fixture source should be written");
    project
        .project_paths_changed(vec![root])
        .expect("settled source burst should publish after one retry");

    assert_eq!(
        hooks.attempts.load(Ordering::Acquire),
        2,
        "all sources changed during the first candidate should join the same retry",
    );
}
