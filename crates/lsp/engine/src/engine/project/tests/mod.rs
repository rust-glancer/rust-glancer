use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
    mpsc,
};

use rg_lsp_proto::{AnalysisConfig, SysrootDiscovery};
use rg_project::{ProjectMemoryHooks, ProjectMemoryPurgePoint, SavedFileChange};
use test_fixture::fixture_crate;

use super::{MAX_STALE_SOURCE_RETRIES, ProjectConfiguration, ProjectCoordinator};
use crate::memory::MemoryControl;

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
                .try_update(Ordering::AcqRel, Ordering::Acquire, |remaining| {
                    remaining.checked_sub(1)
                })
        else {
            return;
        };
        std::fs::write(
            &self.path,
            format!("pub struct Concurrent{previous};\npub fn saved_body() {{}}\n"),
        )
        .expect("source mutation hook should replace fixture source");
    }
}

#[derive(Debug)]
struct SourceBurst {
    armed: AtomicBool,
    attempts: AtomicUsize,
    replacements: Vec<(std::path::PathBuf, &'static str)>,
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

// Read a local binding through the published snapshot so this also checks cache-backed bodies.
fn assert_saved_body_type(project: &ProjectCoordinator, path: &std::path::Path) {
    let source = std::fs::read_to_string(path).expect("fixture source should exist");
    let offset = source.rfind("value").expect("fixture has a local value") as u32;
    let snapshot = project
        .saved_snapshot()
        .expect("saved project should be available");
    let context = snapshot
        .file_context_for_path(path)
        .expect("fixture context should load")
        .expect("fixture source should be indexed");
    let analysis = snapshot
        .analysis_for_crates(&[context.crate_ref], rg_std::CancellationToken::new())
        .expect("saved analysis should open");
    let ty = analysis
        .type_at(context.crate_ref, context.file, offset)
        .expect("saved local type should resolve")
        .expect("saved local has a type");
    assert_eq!(
        ty.primitive(),
        Some(rg_ir_model::PrimitiveTy::UnsignedInt(
            rg_ir_model::UnsignedIntTy::Usize
        ))
    );
}

#[test]
fn eager_startup_and_reindex_have_body_analysis() {
    let fixture = fixture_crate(
        r#"
            //- /Cargo.toml
            [package]
            name = "eager_indexing_fixture"
            version = "0.1.0"
            edition = "2024"

            //- /src/lib.rs
            pub fn indexed_body() -> usize {
                let value = 1usize;
                value
            }
            "#,
    );
    let (sender, receiver) = mpsc::channel();
    let memory_control: Arc<dyn MemoryControl> = Arc::new(());
    let mut project = ProjectCoordinator::new(sender, memory_control);
    project
        .initialize(
            fixture.path(""),
            ProjectConfiguration::from(AnalysisConfig {
                sysroot_discovery: SysrootDiscovery::Disabled,
                ..AnalysisConfig::default()
            }),
        )
        .expect("eager fixture project should initialize");

    assert!(
        matches!(receiver.try_recv(), Err(mpsc::TryRecvError::Empty)),
        "initialization finishes synchronously",
    );
    assert_saved_body_type(&project, &fixture.path("src/lib.rs"));

    project
        .reindex_workspace()
        .expect("eager workspace reindex should succeed");
    assert_saved_body_type(&project, &fixture.path("src/lib.rs"));
    assert!(
        matches!(receiver.try_recv(), Err(mpsc::TryRecvError::Empty)),
        "reindex finishes synchronously",
    );
}

#[test]
fn cached_startup_has_body_analysis() {
    let fixture = fixture_crate(
        r#"
            //- /Cargo.toml
            [package]
            name = "cached_startup_fixture"
            version = "0.1.0"
            edition = "2024"

            //- /src/lib.rs
            pub fn cached_body() -> usize { let value = 1usize; value }
            "#,
    );
    let configuration = ProjectConfiguration::from(AnalysisConfig {
        sysroot_discovery: SysrootDiscovery::Disabled,
        ..AnalysisConfig::default()
    });
    let (sender, _receiver) = mpsc::channel();
    let memory_control: Arc<dyn MemoryControl> = Arc::new(());
    let mut project = ProjectCoordinator::new(sender, memory_control);
    project
        .initialize(fixture.path(""), configuration.clone())
        .expect("cold fixture project should initialize");
    drop(project);

    let (sender, _receiver) = mpsc::channel();
    let memory_control: Arc<dyn MemoryControl> = Arc::new(());
    let mut project = ProjectCoordinator::new(sender, memory_control);
    project
        .initialize(fixture.path(""), configuration)
        .expect("cached fixture project should initialize");
    assert_saved_body_type(&project, &fixture.path("src/lib.rs"));
}

#[test]
fn saved_project_change_exhausts_source_retries_without_publishing() {
    let fixture = fixture_crate(
        r#"
            //- /Cargo.toml
            [package]
            name = "stale_source_retry_fixture"
            version = "0.1.0"
            edition = "2024"

            //- /src/lib.rs
            pub struct Published;
            pub fn saved_body() {}
            "#,
    );
    let source = fixture.path("src/lib.rs");
    let hooks = Arc::new(SourceMutations {
        remaining: AtomicUsize::new(0),
        path: source.clone(),
    });
    let (sender, receiver) = mpsc::channel();
    let memory_control: Arc<dyn MemoryControl> = Arc::new(());
    let mut project = ProjectCoordinator::new(sender, memory_control);
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

    // A continuously rewritten source must eventually return control to the dispatcher. Every
    // failed attempt leaves the published generation untouched, so the next watcher batch can
    // safely retry after this command reports the race.
    let published_generation = project.project.generation();
    std::fs::write(
        &source,
        "pub struct NeverSettles;\npub fn saved_body() {}\n",
    )
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
        "rejected candidates enqueue no work"
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
            pub fn saved_body() {}

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
    let mut project = ProjectCoordinator::new(sender, memory_control);
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
        "an unchanged watcher replay enqueues no work",
    );

    hooks.attempts.store(0, Ordering::Release);
    hooks.armed.store(true, Ordering::Release);
    std::fs::write(
        &root,
        "mod account;\nmod user;\npub struct Candidate;\npub fn saved_body() {}\n",
    )
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
