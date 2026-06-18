//! Cargo-backed diagnostics for the LSP server.
//!
//! This module runs `cargo check`/`cargo clippy` outside the synchronous analysis engine.
//!
//! `DiagnosticsHandle` is created next to the analysis engine and shares the same document freshness
//! store. It emits diagnostics and progress through the service notification channel, so cargo
//! diagnostics stays independent from query/indexing requests and from the concrete LSP client.

use std::{
    collections::{BTreeSet, hash_map::DefaultHasher},
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    sync::Arc,
};

use ls_types::ProgressToken;
use rg_lsp_proto::{AnalysisConfig, DiagnosticsConfig};
use tokio::{sync::Mutex, task::JoinHandle};

use crate::{documents::DocumentStore, service::ServiceNotificationsSink};

mod cargo;
mod command;
mod progress;
mod publish;
mod task;

use self::{
    progress::{DiagnosticsProgress, ProgressFinish},
    task::DiagnosticsTaskContext,
};

/// Launches Cargo diagnostics independently from the synchronous analysis engine.
#[derive(Clone, Debug)]
pub(crate) struct DiagnosticsHandle {
    notifications: ServiceNotificationsSink,
    documents: Arc<Mutex<DocumentStore>>,
    inner: Arc<Mutex<DiagnosticsHandleInner>>,
    current: Arc<Mutex<Option<CurrentDiagnostics>>>,
}

impl DiagnosticsHandle {
    pub(crate) fn new(
        notifications: ServiceNotificationsSink,
        documents: Arc<Mutex<DocumentStore>>,
    ) -> Self {
        Self {
            notifications,
            documents,
            inner: Arc::default(),
            current: Arc::default(),
        }
    }

    pub(crate) async fn configure(
        &self,
        workspace_root: PathBuf,
        config: DiagnosticsConfig,
        analysis: AnalysisConfig,
    ) {
        let mut inner = self.inner.lock().await;
        inner.workspace_root = Some(workspace_root);
        inner.config = config;
        inner.analysis = analysis;
    }

    pub(crate) async fn launch_on_startup(&self) {
        self.launch(DiagnosticsTrigger::Startup).await;
    }

    pub(crate) async fn launch_on_save(&self, saved_path: PathBuf) {
        self.launch(DiagnosticsTrigger::Save { path: saved_path })
            .await;
    }

    async fn launch(&self, trigger: DiagnosticsTrigger) {
        let Some(snapshot) = self.prepare_launch(trigger).await else {
            return;
        };

        self.cancel_current().await;
        let progress_token = ProgressToken::String(Self::progress_token(
            &snapshot.workspace_root,
            snapshot.generation,
        ));
        let progress = DiagnosticsProgress::new(self.notifications.clone(), progress_token);
        let task = DiagnosticsTaskContext::new(
            self.notifications.clone(),
            Arc::clone(&self.documents),
            Arc::clone(&self.inner),
            Arc::clone(&self.current),
        )
        .spawn(snapshot, progress.clone());

        *self.current.lock().await = Some(CurrentDiagnostics { task, progress });
    }

    pub(crate) async fn shutdown(&self) {
        if let Some(current) = self.current.lock().await.take() {
            current.task.abort();
            current.progress.finish(ProgressFinish::Cancelled).await;
        }
    }

    async fn prepare_launch(&self, trigger: DiagnosticsTrigger) -> Option<DiagnosticsSnapshot> {
        let mut inner = self.inner.lock().await;
        if !trigger.enabled(&inner.config) {
            return None;
        }
        let Some(workspace_root) = inner.workspace_root.clone() else {
            tracing::debug!("cargo diagnostics requested before workspace configuration");
            return None;
        };

        inner.generation += 1;
        Some(DiagnosticsSnapshot {
            generation: inner.generation,
            workspace_root,
            config: inner.config.clone(),
            analysis: inner.analysis.clone(),
            trigger,
        })
    }

    async fn cancel_current(&self) {
        if let Some(current) = self.current.lock().await.take() {
            current.task.abort();
            current.progress.finish(ProgressFinish::Cancelled).await;
            tracing::debug!("cancelled previous cargo diagnostics run");
        }
    }

    fn progress_token(workspace_root: &Path, generation: u64) -> String {
        let mut hasher = DefaultHasher::new();
        workspace_root.hash(&mut hasher);
        format!(
            "rust-glancer/diagnostics/{:x}/{generation}",
            hasher.finish()
        )
    }
}

#[derive(Debug)]
struct CurrentDiagnostics {
    task: JoinHandle<()>,
    progress: DiagnosticsProgress,
}

#[derive(Debug, Default)]
struct DiagnosticsHandleInner {
    workspace_root: Option<PathBuf>,
    config: DiagnosticsConfig,
    analysis: AnalysisConfig,
    // Every launched cargo diagnostics task gets a monotonically increasing generation. A task
    // only publishes when it still matches the latest generation, so stale tasks cannot overwrite
    // newer diagnostics.
    generation: u64,
    // Cargo omits files that no longer have diagnostics, but LSP clients require an explicit empty
    // diagnostic list to clear old entries. Track the last published set so the next run can clear
    // stale files.
    published_paths: BTreeSet<PathBuf>,
}

#[derive(Debug)]
struct DiagnosticsSnapshot {
    generation: u64,
    workspace_root: PathBuf,
    config: DiagnosticsConfig,
    analysis: AnalysisConfig,
    trigger: DiagnosticsTrigger,
}

#[derive(Debug)]
enum DiagnosticsTrigger {
    Startup,
    Save { path: PathBuf },
}

impl DiagnosticsTrigger {
    fn enabled(&self, config: &DiagnosticsConfig) -> bool {
        match self {
            Self::Startup => config.on_startup,
            Self::Save { .. } => config.on_save,
        }
    }
}

impl std::fmt::Display for DiagnosticsTrigger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Startup => f.write_str("startup"),
            Self::Save { path } => write!(f, "save:{}", path.display()),
        }
    }
}
