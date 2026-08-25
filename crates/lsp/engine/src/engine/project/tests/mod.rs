use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    time::Duration,
};

use rg_lsp_proto::{
    AnalysisConfig, DeferredIndexingOutcome, PackageResidencyPolicy, ServiceNotification,
    SysrootDiscovery,
};
use rg_project::{ProjectMemoryHooks, ProjectMemoryPurgePoint, SavedFileChange};
use test_fixture::fixture_crate;

use super::{MAX_STALE_SOURCE_RETRIES, ProjectConfiguration, ProjectCoordinator};
use crate::{
    engine::{QueuedEngineCommand, command::EngineCommand},
    memory::MemoryControl,
    service::{ServiceNotificationPublisher, ServiceNotificationsSink},
};

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

/// Apply coalesced background progress the same way the real dispatcher does, then return the next
/// command whose behavior the test is actually exercising.
fn receive_non_progress_command(
    project: &mut ProjectCoordinator,
    receiver: &mpsc::Receiver<QueuedEngineCommand>,
) -> EngineCommand {
    loop {
        let queued = receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("deferred indexing should return an engine command");
        match queued.command {
            EngineCommand::DeferredIndexingProgress {
                generation,
                progress,
            } => project.deferred_indexing_progress(generation, progress),
            command => return command,
        }
    }
}

fn assert_progress_then_finished(notifications: &[ServiceNotification]) {
    assert!(
        notifications.iter().any(|notification| matches!(
            notification,
            ServiceNotification::DeferredIndexingProgress { .. }
        )),
        "deferred indexing should publish at least one package progress snapshot",
    );
    assert!(matches!(
        notifications.last(),
        Some(ServiceNotification::DeferredIndexingFinished {
            outcome: DeferredIndexingOutcome::Succeeded,
            ..
        })
    ));
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
                // The final assertions observe resident Body IR, so keep this lifecycle fixture
                // independent from the client-facing residency default.
                package_residency_policy: PackageResidencyPolicy::AllResident,
                sysroot_discovery: SysrootDiscovery::Disabled,
                ..AnalysisConfig::default()
            }),
        )
        .expect("fixture project should initialize");
    assert!(matches!(
        recorded.take().as_slice(),
        [ServiceNotification::DeferredIndexingStarted { .. }]
    ));

    let initial = receive_non_progress_command(&mut project, &receiver);
    let EngineCommand::DeferredIndexingFinished { generation, result } = initial else {
        panic!("initial background command should finish deferred indexing");
    };
    let _ = memory_hooks.take();
    project.deferred_indexing_finished(generation, result);
    assert_eq!(
        memory_hooks.take().last(),
        Some(&ProjectMemoryPurgePoint::AfterDeferredIndexingFinish),
        "the detached result should die before the final deferred-indexing purge",
    );
    assert_progress_then_finished(&recorded.take());

    project
        .saved_project_changes(vec![SavedFileChange::fs_path(source.clone())])
        .expect("an unchanged watcher replay should be accepted");
    assert!(
        recorded.take().is_empty(),
        "an unchanged foreground cycle should not invent deferred work",
    );

    std::fs::write(&source, "pub fn updated() -> usize { 2 }\n")
        .expect("fixture source should be replaced");
    project
        .saved_project_changes(vec![SavedFileChange::fs_path(source)])
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

    let updated = receive_non_progress_command(&mut project, &receiver);
    let EngineCommand::DeferredIndexingFinished { generation, result } = updated else {
        panic!("updated background command should finish deferred indexing");
    };
    project.deferred_indexing_finished(generation, result);
    assert_progress_then_finished(&recorded.take());
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
fn current_deferred_failure_is_published_without_discarding_the_queryable_project() {
    let fixture = fixture_crate(
        r#"
            //- /Cargo.toml
            [package]
            name = "deferred_failure_fixture"
            version = "0.1.0"
            edition = "2024"

            //- /src/lib.rs
            pub fn still_queryable() {}
            "#,
    );
    let (sender, receiver) = mpsc::channel();
    let memory_control: Arc<dyn MemoryControl> = Arc::new(());
    let recorded = RecordingNotifications::default();
    let notifications = ServiceNotificationsSink::from_publisher(recorded.clone());
    let mut project = ProjectCoordinator::new(sender, memory_control, notifications);
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

    let finished = receive_non_progress_command(&mut project, &receiver);
    let EngineCommand::DeferredIndexingFinished { generation, .. } = finished else {
        panic!("background command should reach its terminal result");
    };
    project.deferred_indexing_finished(
        generation,
        Err(anyhow::anyhow!("synthetic body indexing failure")),
    );

    project
        .saved_snapshot()
        .expect("early-start project should remain queryable after deferred failure");
    assert!(matches!(
        recorded.take().last(),
        Some(ServiceNotification::DeferredIndexingFinished {
            outcome: DeferredIndexingOutcome::Failed { message },
            ..
        }) if message == "synthetic body indexing failure"
    ));
}

#[test]
fn open_document_package_is_published_before_full_deferred_result() {
    let fixture = fixture_crate(
        r#"
            //- /Cargo.toml
            [workspace]
            members = ["app", "helper"]
            resolver = "2"

            //- /app/Cargo.toml
            [package]
            name = "deferred_priority_app"
            version = "0.1.0"
            edition = "2024"

            //- /app/src/lib.rs
            pub fn app() -> usize { 1 }

            //- /helper/Cargo.toml
            [package]
            name = "deferred_priority_helper"
            version = "0.1.0"
            edition = "2024"

            //- /helper/src/lib.rs
            pub fn first() -> usize { 1 }
            pub fn second() -> usize { 2 }
            pub fn third() -> usize { 3 }
            "#,
    );
    let (sender, receiver) = mpsc::channel();
    let memory_control: Arc<dyn MemoryControl> = Arc::new(());
    let recorded = RecordingNotifications::default();
    let notifications = ServiceNotificationsSink::from_publisher(recorded.clone());
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
    assert!(matches!(
        recorded.take().as_slice(),
        [ServiceNotification::DeferredIndexingStarted { .. }]
    ));

    // The worker is already live when this didOpen-derived hint reaches its package queue.
    project.set_deferred_indexing_priority(fixture.path("helper/src/lib.rs"), true);
    let first = receive_non_progress_command(&mut project, &receiver);
    let EngineCommand::DeferredIndexingPriorityPackageFinished {
        generation,
        finished,
    } = first
    else {
        panic!("the open-document package should publish before the final background result");
    };
    project.deferred_indexing_priority_package_finished(generation, *finished);

    let first_stats = project
        .saved_snapshot()
        .expect("partially finished saved project should be available")
        .stats()
        .body_ir;
    assert_eq!(first_stats.complete_crate_count, 1);
    assert_eq!(
        first_stats.body_count, 3,
        "the three-body open-document package should be the first publication",
    );
    assert!(
        recorded.take().iter().all(|notification| !matches!(
            notification,
            ServiceNotification::DeferredIndexingFinished { .. }
        )),
        "an intermediate package publication must not end deferred indexing",
    );

    let final_command = receive_non_progress_command(&mut project, &receiver);
    let EngineCommand::DeferredIndexingFinished { generation, result } = final_command else {
        panic!("the final package result should complete deferred indexing");
    };
    project.deferred_indexing_finished(generation, result);

    let finished_stats = project
        .saved_snapshot()
        .expect("finished saved project should be available")
        .stats()
        .body_ir;
    assert_eq!(finished_stats.complete_crate_count, 2);
    assert_eq!(finished_stats.body_count, 4);
    assert!(matches!(
        recorded.take().last(),
        Some(ServiceNotification::DeferredIndexingFinished { .. })
    ));
}

#[test]
fn saved_project_change_retries_source_races_but_preserves_a_finite_lane_budget() {
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
    let initial = receive_non_progress_command(&mut project, &receiver);
    let EngineCommand::DeferredIndexingFinished { generation, result } = initial else {
        panic!("initial background command should finish deferred indexing");
    };
    project.deferred_indexing_finished(generation, result);

    std::fs::write(&source, "pub struct Candidate;\n")
        .expect("candidate fixture source should be written");
    // Two writes force the replacement candidate itself to become stale once more. Recovery
    // must remain indexing until the third attempt captures a quiet generation.
    hooks.remaining.store(2, Ordering::Release);
    project
        .saved_project_changes(vec![SavedFileChange::fs_path(source.clone())])
        .expect("stale candidate should be retried from the newer disk revision");

    let deferred = receive_non_progress_command(&mut project, &receiver);
    let EngineCommand::DeferredIndexingFinished { generation, result } = deferred else {
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
        .saved_project_changes(vec![SavedFileChange::fs_path(source)])
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
fn saved_project_retry_collects_the_settled_source_burst() {
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

    let initial = receive_non_progress_command(&mut project, &receiver);
    let EngineCommand::DeferredIndexingFinished { generation, result } = initial else {
        panic!("initial background command should finish deferred indexing");
    };
    project.deferred_indexing_finished(generation, result);

    let published_generation = project.project.generation();
    project
        .saved_project_changes(vec![SavedFileChange::fs_path(root.clone())])
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
        .saved_project_changes(vec![SavedFileChange::fs_path(root)])
        .expect("settled source burst should publish after one retry");

    assert_eq!(
        hooks.attempts.load(Ordering::Acquire),
        2,
        "all sources changed during the first candidate should join the same retry",
    );
}
