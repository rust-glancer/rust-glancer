//! Cargo-backed diagnostics for the LSP server.
//!
//! This module runs `cargo check`/`cargo clippy` outside the synchronous analysis engine.
//!
//! `CheckHandle` is owned by `EngineHandle`. It emits diagnostics and progress as engine events,
//! so cargo diagnostics can later move into a subprocess without keeping a direct LSP-client
//! dependency in this crate.

use std::{collections::BTreeSet, path::PathBuf, sync::Arc};

use ls_types::ProgressToken;
use tokio::{sync::Mutex, task::JoinHandle};

use crate::{documents::DocumentStore, events::EngineEventSink};

mod command;
mod config;
mod diagnostics;
mod progress;
mod publish;
mod task;

use self::{
    progress::{CheckProgress, ProgressFinish},
    task::CheckTaskContext,
};

pub use self::config::CheckConfig;

/// Launches Cargo diagnostics independently from the synchronous analysis engine.
#[derive(Clone, Debug)]
pub(crate) struct CheckHandle {
    events: EngineEventSink,
    documents: Arc<Mutex<DocumentStore>>,
    inner: Arc<Mutex<CheckHandleInner>>,
    current: Arc<Mutex<Option<CurrentCheck>>>,
}

impl CheckHandle {
    pub(crate) fn new(events: EngineEventSink, documents: Arc<Mutex<DocumentStore>>) -> Self {
        Self {
            events,
            documents,
            inner: Arc::default(),
            current: Arc::default(),
        }
    }

    pub(crate) async fn configure(&self, workspace_root: PathBuf, config: CheckConfig) {
        let mut inner = self.inner.lock().await;
        inner.workspace_root = Some(workspace_root);
        inner.config = config;
    }

    pub(crate) async fn launch_on_startup(&self) {
        self.launch(CheckTrigger::Startup).await;
    }

    pub(crate) async fn launch_on_save(&self, saved_path: PathBuf) {
        self.launch(CheckTrigger::Save { path: saved_path }).await;
    }

    async fn launch(&self, trigger: CheckTrigger) {
        let Some(snapshot) = self.prepare_launch(trigger).await else {
            return;
        };

        self.cancel_current().await;
        let progress_token =
            ProgressToken::String(format!("rust-glancer/check/{}", snapshot.generation));
        let progress = CheckProgress::new(self.events.clone(), progress_token);
        let task = CheckTaskContext::new(
            self.events.clone(),
            Arc::clone(&self.documents),
            Arc::clone(&self.inner),
            Arc::clone(&self.current),
        )
        .spawn(snapshot, progress.clone());

        *self.current.lock().await = Some(CurrentCheck { task, progress });
    }

    pub(crate) async fn shutdown(&self) {
        if let Some(current) = self.current.lock().await.take() {
            current.task.abort();
            current.progress.finish(ProgressFinish::Cancelled).await;
        }
    }

    async fn prepare_launch(&self, trigger: CheckTrigger) -> Option<CheckSnapshot> {
        let mut inner = self.inner.lock().await;
        if !trigger.enabled(&inner.config) {
            return None;
        }
        let Some(workspace_root) = inner.workspace_root.clone() else {
            tracing::debug!("cargo diagnostics requested before workspace configuration");
            return None;
        };

        inner.generation += 1;
        Some(CheckSnapshot {
            generation: inner.generation,
            workspace_root,
            config: inner.config.clone(),
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
}

#[derive(Debug)]
struct CurrentCheck {
    task: JoinHandle<()>,
    progress: CheckProgress,
}

#[derive(Debug, Default)]
struct CheckHandleInner {
    workspace_root: Option<PathBuf>,
    config: CheckConfig,
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
struct CheckSnapshot {
    generation: u64,
    workspace_root: PathBuf,
    config: CheckConfig,
    trigger: CheckTrigger,
}

#[derive(Debug)]
enum CheckTrigger {
    Startup,
    Save { path: PathBuf },
}

impl CheckTrigger {
    fn enabled(&self, config: &CheckConfig) -> bool {
        match self {
            Self::Startup => config.on_startup,
            Self::Save { .. } => config.on_save,
        }
    }
}

impl std::fmt::Display for CheckTrigger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Startup => f.write_str("startup"),
            Self::Save { path } => write!(f, "save:{}", path.display()),
        }
    }
}
