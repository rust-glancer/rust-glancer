//! Server-side filesystem watcher for saved project inputs.
//!
//! The analysis engine intentionally treats saved-file notifications as its filesystem coherence
//! boundary. This watcher owns that boundary for external edits, so editor-specific watcher
//! behavior cannot leave the saved project behind disk.

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsStr,
    path::{Component, Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::Context as _;
use ignore::WalkBuilder;
use notify_debouncer_full::{
    DebounceEventResult, Debouncer, NoCache,
    notify::{Config as NotifyConfig, RecommendedWatcher, RecursiveMode},
};
use tokio::{sync::mpsc, task::JoinHandle};

use crate::{
    engine_registry::EngineRegistry, file_identity::FileIdentity,
    recent_editor_saves::RecentEditorSaves,
};

const WATCH_DEBOUNCE: Duration = Duration::from_millis(300);

type ProjectDebouncer = Debouncer<RecommendedWatcher, NoCache>;

/// Keeps native filesystem watching alive for the lifetime of the LSP server.
#[derive(Debug)]
pub(crate) struct ProjectWatcher {
    _workspaces: Vec<WorkspaceWatcher>,
}

#[derive(Debug)]
struct WorkspaceWatcher {
    _root: PathBuf,
    _debouncer: ProjectDebouncer,
    _forwarder: JoinHandle<()>,
}

impl ProjectWatcher {
    pub(crate) fn spawn(
        workspace_roots: Vec<PathBuf>,
        registry: EngineRegistry,
        recent_editor_saves: RecentEditorSaves,
    ) -> anyhow::Result<Self> {
        let mut workspaces = Vec::new();

        for root in workspace_roots
            .into_iter()
            .map(WorkspaceWatcher::normalize_root)
        {
            match WorkspaceWatcher::spawn(
                root.clone(),
                registry.clone(),
                recent_editor_saves.clone(),
            ) {
                Ok(workspace) => workspaces.push(workspace),
                Err(error) => {
                    tracing::warn!(
                        root = %root.display(),
                        error = %error,
                        "failed to watch workspace root for saved project changes"
                    );
                }
            }
        }

        anyhow::ensure!(
            !workspaces.is_empty(),
            "no workspace roots could be watched for saved project changes"
        );

        Ok(Self {
            _workspaces: workspaces,
        })
    }
}

impl WorkspaceWatcher {
    fn spawn(
        root: PathBuf,
        registry: EngineRegistry,
        recent_editor_saves: RecentEditorSaves,
    ) -> anyhow::Result<Self> {
        let (sender, mut receiver) = mpsc::unbounded_channel::<DebounceEventResult>();
        let callback_root = root.clone();

        let mut debouncer = notify_debouncer_full::new_debouncer_opt(
            WATCH_DEBOUNCE,
            Some(WATCH_DEBOUNCE),
            move |result| {
                if sender.send(result).is_err() {
                    tracing::trace!(
                        root = %callback_root.display(),
                        "project watcher event dropped because receiver is gone"
                    );
                }
            },
            NoCache::new(),
            NotifyConfig::default(),
        )
        .context("while attempting to create project filesystem watcher")?;

        debouncer
            .watch(&root, RecursiveMode::Recursive)
            .with_context(|| {
                format!(
                    "while attempting to watch workspace root {}",
                    root.display()
                )
            })?;
        tracing::debug!(
            root = %root.display(),
            debounce_ms = WATCH_DEBOUNCE.as_millis(),
            "watching workspace root for saved project changes"
        );

        let forwarder_root = root.clone();
        let mut snapshot = ProjectPathSnapshot::scan(forwarder_root.as_path());
        let forwarder = tokio::spawn(async move {
            while let Some(result) = receiver.recv().await {
                Self::forward_watcher_result(
                    &mut snapshot,
                    forwarder_root.as_path(),
                    &registry,
                    &recent_editor_saves,
                    result,
                )
                .await;
            }
        });

        Ok(Self {
            _root: root,
            _debouncer: debouncer,
            _forwarder: forwarder,
        })
    }

    #[tracing::instrument(level = "trace", skip_all, fields(root = %root.display()))]
    async fn forward_watcher_result(
        snapshot: &mut ProjectPathSnapshot,
        root: &Path,
        registry: &EngineRegistry,
        recent_editor_saves: &RecentEditorSaves,
        result: DebounceEventResult,
    ) {
        let paths = match result {
            Ok(events) => {
                let event_count = events.len();
                let raw_path_count = events
                    .iter()
                    .map(|event| event.event.paths.len())
                    .sum::<usize>();
                if events.iter().any(|event| event.need_rescan()) {
                    tracing::warn!(
                        events = event_count,
                        raw_paths = raw_path_count,
                        "project watcher requested rescan after missed events"
                    );
                    let paths = snapshot.changed_paths_after_rescan(root);
                    tracing::debug!(
                        events = event_count,
                        raw_paths = raw_path_count,
                        relevant_paths = paths.len(),
                        need_rescan = true,
                        "processed project watcher batch"
                    );
                    paths
                } else {
                    let mut ignored_paths = 0usize;
                    let mut unchanged_paths = 0usize;
                    let paths = events
                        .iter()
                        .flat_map(|event| event.event.paths.iter())
                        .filter_map(|path| {
                            let project_path = WatchedProjectPath::from_event(path);
                            let Some(project_path) = project_path else {
                                ignored_paths += 1;
                                return None;
                            };
                            if snapshot.refresh_path(&project_path) {
                                Some(project_path)
                            } else {
                                unchanged_paths += 1;
                                None
                            }
                        })
                        .collect::<BTreeSet<_>>()
                        .into_iter()
                        .collect::<Vec<_>>();
                    tracing::debug!(
                        events = event_count,
                        raw_paths = raw_path_count,
                        ignored_paths,
                        unchanged_paths,
                        relevant_paths = paths.len(),
                        need_rescan = false,
                        "processed project watcher batch"
                    );
                    paths
                }
            }
            Err(errors) => {
                let error_count = errors.len();
                for error in errors {
                    tracing::warn!(
                        error = %error,
                        "project watcher reported an error; rescanning workspace root"
                    );
                }
                let paths = snapshot.changed_paths_after_rescan(root);
                tracing::debug!(
                    errors = error_count,
                    relevant_paths = paths.len(),
                    "processed project watcher error batch"
                );
                paths
            }
        };
        let path_count_before_save_filter = paths.len();
        let paths = recent_editor_saves.saves_to_process(paths).await;

        if paths.is_empty() {
            tracing::debug!(
                paths_before_save_filter = path_count_before_save_filter,
                forwarded_paths = 0usize,
                "server-side watched project changes filtered out"
            );
            return;
        }

        tracing::debug!(
            paths_before_save_filter = path_count_before_save_filter,
            forwarded_paths = paths.len(),
            "forwarding server-side watched project changes"
        );
        registry.external_project_paths_changed(paths).await;
    }

    fn normalize_root(path: impl AsRef<Path>) -> PathBuf {
        let path = path.as_ref();
        path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
    }
}

struct WatchedProjectPath;

impl WatchedProjectPath {
    fn from_event(path: &Path) -> Option<PathBuf> {
        if Self::is_ignored(path) || !Self::is_project_input(path) {
            return None;
        }

        Some(Self::normalize(path))
    }

    fn identity(path: &Path) -> Option<(PathBuf, FileIdentity)> {
        if Self::is_ignored(path) || !Self::is_project_input(path) {
            return None;
        }

        FileIdentity::read(&Self::normalize(path))
    }

    fn should_visit(path: &Path) -> bool {
        !Self::is_ignored(path)
    }

    fn normalize(path: impl AsRef<Path>) -> PathBuf {
        let path = path.as_ref();
        path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
    }

    fn is_project_input(path: &Path) -> bool {
        let file_name = path.file_name().and_then(OsStr::to_str);
        path.extension().and_then(OsStr::to_str) == Some("rs")
            || matches!(file_name, Some("Cargo.toml" | "Cargo.lock"))
    }

    fn is_ignored(path: &Path) -> bool {
        path.components().any(|component| {
            let Component::Normal(name) = component else {
                return false;
            };
            matches!(
                name.to_str(),
                Some(".git" | "target" | "node_modules" | ".direnv")
            )
        })
    }
}

#[derive(Debug)]
struct ProjectPathSnapshot {
    identities: BTreeMap<PathBuf, FileIdentity>,
}

impl ProjectPathSnapshot {
    /// Tracks just enough disk state to suppress watcher startup noise and full-rescan false
    /// positives. The engine remains the source of analysis truth; this snapshot only decides
    /// whether a watcher batch describes a real saved-input change. Metadata is intentionally
    /// enough here: a false positive only costs a small reindex, while hashing every file would
    /// make watcher rescans scale with source size.
    fn scan(root: &Path) -> Self {
        let started = Instant::now();
        let mut files_seen = 0usize;
        let mut identities = BTreeMap::new();
        let mut builder = WalkBuilder::new(root);
        builder
            .hidden(false)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .filter_entry(|entry| WatchedProjectPath::should_visit(entry.path()));

        for entry in builder.build() {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    tracing::debug!(
                        error = %error,
                        "failed to scan watched workspace root entry"
                    );
                    continue;
                }
            };

            let path = entry.path();
            if !entry
                .file_type()
                .is_some_and(|file_type| file_type.is_file())
            {
                continue;
            }
            files_seen += 1;

            let Some((path, identity)) = WatchedProjectPath::identity(path) else {
                continue;
            };
            identities.insert(path, identity);
        }

        tracing::debug!(
            files_seen,
            project_files_seen = identities.len(),
            elapsed_ms = started.elapsed().as_millis(),
            "snapshotted watched workspace project paths"
        );
        Self { identities }
    }

    fn changed_paths_after_rescan(&mut self, root: &Path) -> Vec<PathBuf> {
        let next = Self::scan(root);
        let mut changed = BTreeSet::new();

        for (path, identity) in &next.identities {
            if self.identities.get(path) != Some(identity) {
                changed.insert(path.clone());
            }
        }
        for path in self.identities.keys() {
            if !next.identities.contains_key(path) {
                changed.insert(path.clone());
            }
        }

        self.identities = next.identities;
        changed.into_iter().collect()
    }

    fn refresh_path(&mut self, path: &Path) -> bool {
        let normalized = WatchedProjectPath::normalize(path);
        match WatchedProjectPath::identity(&normalized) {
            Some((path, identity)) => {
                let changed = self.identities.get(&path) != Some(&identity);
                self.identities.insert(path, identity);
                changed
            }
            None => self.identities.remove(&normalized).is_some(),
        }
    }
}
