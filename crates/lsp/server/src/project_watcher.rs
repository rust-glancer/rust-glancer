//! Server-side filesystem watcher for saved project inputs.
//!
//! The analysis engine intentionally treats saved-file notifications as its filesystem coherence
//! boundary. This watcher owns that boundary for external edits, so editor-specific watcher
//! behavior cannot leave the saved project behind disk.
//!
//! A relevant native event first publishes updating status for every affected ready engine. The
//! watcher then waits for the whole edit burst to become quiet and compares its filesystem
//! snapshot. The registry captures each existing Rust source once and sends one coalesced
//! saved-project update. That update stays live through the quiet wait and engine rebuild, so the
//! editor does not show `Ready` before the replacement finishes.

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
    notify::{
        Config as NotifyConfig, EventKind, RecommendedWatcher, RecursiveMode,
        event::{AccessKind, AccessMode},
    },
};
use tokio::{sync::mpsc, task::JoinHandle};

use crate::{
    engine_registry::EngineRegistry, file_identity::FileIdentity,
    recent_editor_saves::RecentEditorSaves,
};

const WATCH_DEBOUNCE: Duration = Duration::from_millis(300);
// The notify debouncer expires paths independently, so a large checkout can produce a stream of
// batches one tick apart. Wait for a global quiet period before asking the engine to rebuild.
const WATCH_SETTLE: Duration = Duration::from_millis(600);

type ProjectDebouncer = Debouncer<RecommendedWatcher, NoCache>;

/// Keeps native filesystem watching alive for the lifetime of the LSP server.
#[derive(Debug)]
pub(crate) struct ProjectWatcher {
    _workspaces: Vec<WorkspaceWatcher>,
}

/// One native watcher, filesystem snapshot, and async forwarder for an editor workspace folder.
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
        anyhow::ensure!(
            !workspace_roots.is_empty(),
            "no workspace roots were provided for saved-project watching"
        );
        let mut workspaces = Vec::new();

        for root in workspace_roots
            .into_iter()
            .map(WorkspaceWatcher::normalize_root)
        {
            let workspace = WorkspaceWatcher::spawn(
                root.clone(),
                registry.clone(),
                recent_editor_saves.clone(),
            )
            .with_context(|| {
                format!(
                    "while attempting to start saved-project watcher for {}",
                    root.display()
                )
            })?;
            workspaces.push(workspace);
        }

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
                let Some(result) = Self::project_result(callback_root.as_path(), result) else {
                    return;
                };
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
                // The first relevant native event means the saved project may already disagree
                // with disk. Publish that fact before waiting for the checkout/write burst to
                // settle, so interactive requests do not queue behind a rebuild that has not yet
                // been submitted.
                let changes = registry
                    .begin_external_project_changes(forwarder_root.as_path())
                    .await;
                let results = Self::collect_settled_results(result, &mut receiver).await;
                Self::forward_watcher_results(
                    &mut snapshot,
                    forwarder_root.as_path(),
                    &registry,
                    &recent_editor_saves,
                    results,
                    changes,
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

    fn project_result(root: &Path, result: DebounceEventResult) -> Option<DebounceEventResult> {
        match result {
            Ok(mut events) => {
                // Filter before the async queue so target-directory churn cannot keep extending
                // the workspace settle window. Rescan events have no useful paths and must always
                // reach the snapshot recovery path.
                events.retain(|event| {
                    if event.need_rescan() {
                        return true;
                    }

                    // Linux inotify reports opening and reading a file as watcher events. Those
                    // accesses are especially common while rust-glancer indexes its own workspace,
                    // but they cannot make the saved project stale. Only an explicit
                    // close-after-write remains useful as a mutation signal; normal modify events
                    // cover backends that do not report the close mode.
                    let may_change_saved_input = match event.event.kind {
                        EventKind::Access(AccessKind::Close(AccessMode::Write)) => true,
                        EventKind::Access(_) => false,
                        _ => true,
                    };
                    may_change_saved_input
                        && event
                            .event
                            .paths
                            .iter()
                            .any(|path| WatchedProjectPath::is_watched_project_input(root, path))
                });
                (!events.is_empty()).then_some(Ok(events))
            }
            Err(errors) => Some(Err(errors)),
        }
    }

    async fn collect_settled_results(
        first: DebounceEventResult,
        receiver: &mut mpsc::UnboundedReceiver<DebounceEventResult>,
    ) -> Vec<DebounceEventResult> {
        let mut results = vec![first];

        // Reset the wait after every result. This turns notify's per-path debounce stream into one
        // workspace-level update after a checkout or agent write burst has settled.
        loop {
            match tokio::time::timeout(WATCH_SETTLE, receiver.recv()).await {
                Ok(Some(result)) => results.push(result),
                Ok(None) | Err(_) => return results,
            }
        }
    }

    /// Turn one settled native-event burst into a saved-project update.
    ///
    /// This always returns `changes` to the registry, even when every path was an editor save or
    /// disappeared during the burst. That lets unmatched early project updates end as
    /// cancellations instead of leaving the workspace permanently `Indexing`.
    #[tracing::instrument(level = "trace", skip_all, fields(root = %root.display()))]
    async fn forward_watcher_results(
        snapshot: &mut ProjectPathSnapshot,
        root: &Path,
        registry: &EngineRegistry,
        recent_editor_saves: &RecentEditorSaves,
        results: Vec<DebounceEventResult>,
        changes: crate::engine_registry::ExternalProjectChanges,
    ) {
        let paths = Self::changed_paths_for_results(snapshot, root, results);
        let path_count_before_save_filter = paths.len();
        let paths = recent_editor_saves.saves_to_process(paths);

        if paths.is_empty() {
            tracing::trace!(
                paths_before_save_filter = path_count_before_save_filter,
                forwarded_paths = 0usize,
                "server-side watched project changes filtered out"
            );
        } else {
            tracing::debug!(
                paths_before_save_filter = path_count_before_save_filter,
                forwarded_paths = paths.len(),
                "forwarding server-side watched project changes"
            );
        }

        registry
            .finish_external_project_changes(paths, changes)
            .await;
    }

    fn changed_paths_for_results(
        snapshot: &mut ProjectPathSnapshot,
        root: &Path,
        results: Vec<DebounceEventResult>,
    ) -> Vec<PathBuf> {
        let batch_count = results.len();
        let mut events = Vec::new();
        let mut errors = Vec::new();

        for result in results {
            match result {
                Ok(mut batch_events) => events.append(&mut batch_events),
                Err(mut batch_errors) => errors.append(&mut batch_errors),
            }
        }

        let event_count = events.len();
        let raw_path_count = events
            .iter()
            .map(|event| event.event.paths.len())
            .sum::<usize>();
        let need_rescan = !errors.is_empty() || events.iter().any(|event| event.need_rescan());

        if need_rescan {
            if events.iter().any(|event| event.need_rescan()) {
                tracing::warn!(
                    batches = batch_count,
                    events = event_count,
                    raw_paths = raw_path_count,
                    "project watcher requested rescan after missed events"
                );
            }
            for error in &errors {
                tracing::warn!(
                    error = %error,
                    "project watcher reported an error; rescanning workspace root"
                );
            }

            let paths = snapshot.changed_paths_after_rescan(root);
            tracing::debug!(
                batches = batch_count,
                events = event_count,
                errors = errors.len(),
                raw_paths = raw_path_count,
                relevant_paths = paths.len(),
                need_rescan = true,
                "processed settled project watcher results"
            );
            return paths;
        }

        let mut ignored_paths = 0usize;
        let mut unchanged_paths = 0usize;
        let paths = events
            .iter()
            .flat_map(|event| event.event.paths.iter())
            .filter_map(|path| {
                let project_path = WatchedProjectPath::from_event(root, path);
                let Some(project_path) = project_path else {
                    ignored_paths += 1;
                    return None;
                };
                if snapshot.refresh_path(root, &project_path) {
                    Some(project_path)
                } else {
                    unchanged_paths += 1;
                    None
                }
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if paths.is_empty() {
            tracing::trace!(
                batches = batch_count,
                events = event_count,
                raw_paths = raw_path_count,
                ignored_paths,
                unchanged_paths,
                relevant_paths = 0usize,
                need_rescan = false,
                "processed settled project watcher results"
            );
        } else {
            tracing::debug!(
                batches = batch_count,
                events = event_count,
                raw_paths = raw_path_count,
                ignored_paths,
                unchanged_paths,
                relevant_paths = paths.len(),
                need_rescan = false,
                "processed settled project watcher results"
            );
        }
        paths
    }

    fn normalize_root(path: impl AsRef<Path>) -> PathBuf {
        let path = path.as_ref();
        rg_std::path::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    }
}

struct WatchedProjectPath;

impl WatchedProjectPath {
    fn from_event(root: &Path, path: &Path) -> Option<PathBuf> {
        if !Self::is_watched_project_input(root, path) {
            return None;
        }

        Some(Self::normalize(path))
    }

    fn is_watched_project_input(root: &Path, path: &Path) -> bool {
        !Self::is_ignored(root, path) && Self::is_project_input(path)
    }

    fn identity(root: &Path, path: &Path) -> Option<(PathBuf, FileIdentity)> {
        if Self::is_ignored(root, path) || !Self::is_project_input(path) {
            return None;
        }

        FileIdentity::read(&Self::normalize(path))
    }

    fn should_visit(root: &Path, path: &Path) -> bool {
        !Self::is_ignored(root, path)
    }

    fn normalize(path: impl AsRef<Path>) -> PathBuf {
        let path = path.as_ref();
        rg_std::path::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    }

    fn is_project_input(path: &Path) -> bool {
        let file_name = path.file_name().and_then(OsStr::to_str);
        path.extension().and_then(OsStr::to_str) == Some("rs")
            || matches!(file_name, Some("Cargo.toml" | "Cargo.lock"))
    }

    fn is_ignored(root: &Path, path: &Path) -> bool {
        // Ignore directory names only inside the watched workspace. The workspace itself may live
        // below an unrelated `target`, `.git`, or dependency directory on the host filesystem.
        let Ok(relative) = path.strip_prefix(root) else {
            return true;
        };

        relative.components().any(|component| {
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
        let filter_root = root.to_path_buf();
        builder
            .hidden(false)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .filter_entry(move |entry| {
                WatchedProjectPath::should_visit(&filter_root, entry.path())
            });

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

            let Some((path, identity)) = WatchedProjectPath::identity(root, path) else {
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

    fn refresh_path(&mut self, root: &Path, path: &Path) -> bool {
        let normalized = WatchedProjectPath::normalize(path);
        match WatchedProjectPath::identity(root, &normalized) {
            Some((path, identity)) => {
                let changed = self.identities.get(&path) != Some(&identity);
                self.identities.insert(path, identity);
                changed
            }
            None => self.identities.remove(&normalized).is_some(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use notify_debouncer_full::{
        DebouncedEvent,
        notify::{
            Event, EventKind,
            event::{AccessKind, AccessMode, DataChange, ModifyKind},
        },
    };
    use test_fixture::fixture_crate;

    use super::*;

    #[test]
    fn watcher_ingress_scopes_ignored_directories_to_workspace_root() {
        let root = PathBuf::from("/checkout/target/project");
        let target_source = root.join("target/debug/build/generated.rs");
        let notes = root.join("notes.md");

        assert!(
            WorkspaceWatcher::project_result(
                &root,
                Ok(vec![
                    changed_event(target_source.clone()),
                    changed_event(notes),
                ])
            )
            .is_none(),
            "target and non-project churn should not enter the async watcher queue"
        );

        let project_source = root.join("src/lib.rs");
        let Some(Ok(events)) = WorkspaceWatcher::project_result(
            &root,
            Ok(vec![
                changed_event(target_source),
                changed_event(project_source.clone()),
            ]),
        ) else {
            panic!("a project input below an ancestor named target should remain watched");
        };
        assert_eq!(
            events
                .iter()
                .flat_map(|event| event.event.paths.iter())
                .collect::<Vec<_>>(),
            vec![&project_source],
            "ignored directories should apply only below the workspace root"
        );
    }

    #[test]
    fn watcher_ingress_drops_read_only_project_accesses() {
        let root = PathBuf::from("/workspace");
        let source = root.join("src/lib.rs");

        for kind in [
            EventKind::Access(AccessKind::Read),
            EventKind::Access(AccessKind::Open(AccessMode::Any)),
            EventKind::Access(AccessKind::Close(AccessMode::Read)),
            EventKind::Access(AccessKind::Close(AccessMode::Any)),
        ] {
            assert!(
                WorkspaceWatcher::project_result(
                    &root,
                    Ok(vec![watcher_event(source.clone(), kind,)]),
                )
                .is_none(),
                "read-only access event {kind:?} should not enter the async watcher queue",
            );
        }

        for kind in [
            EventKind::Modify(ModifyKind::Data(DataChange::Any)),
            EventKind::Access(AccessKind::Close(AccessMode::Write)),
        ] {
            assert!(
                WorkspaceWatcher::project_result(
                    &root,
                    Ok(vec![watcher_event(source.clone(), kind,)]),
                )
                .is_some(),
                "saved-input mutation event {kind:?} should enter the async watcher queue",
            );
        }
    }

    #[tokio::test]
    async fn settled_results_merge_ready_watcher_backlog() {
        let fixture = fixture_crate(
            r#"
            //- /Cargo.toml
            [package]
            name = "watcher_batch_fixture"
            version = "0.1.0"
            edition = "2024"

            //- /src/account.rs
            pub struct Account;

            //- /src/user.rs
            pub struct User;
            "#,
        );
        let root = WorkspaceWatcher::normalize_root(fixture.path(""));
        let account = root.join("src/account.rs");
        let user = root.join("src/user.rs");
        let mut snapshot = ProjectPathSnapshot::scan(&root);

        std::fs::write(&account, "pub struct SavedAccount;\n")
            .expect("account fixture should be writable");
        std::fs::write(&user, "pub struct SavedUser;\n").expect("user fixture should be writable");

        let (sender, mut receiver) = mpsc::unbounded_channel();
        sender
            .send(Ok(vec![changed_event(account.clone())]))
            .expect("first watcher result should queue");
        sender
            .send(Ok(vec![changed_event(user.clone())]))
            .expect("second watcher result should queue");
        drop(sender);

        let first = receiver
            .recv()
            .await
            .expect("first watcher result should be available");
        let results = WorkspaceWatcher::collect_settled_results(first, &mut receiver).await;
        let paths = WorkspaceWatcher::changed_paths_for_results(&mut snapshot, &root, results);
        let account =
            rg_std::path::canonicalize(account).expect("account fixture should canonicalize");
        let user = rg_std::path::canonicalize(user).expect("user fixture should canonicalize");

        assert_eq!(
            paths,
            vec![account, user],
            "ready debounce results should become one project update"
        );
    }

    fn changed_event(path: PathBuf) -> DebouncedEvent {
        watcher_event(path, EventKind::Modify(ModifyKind::Data(DataChange::Any)))
    }

    fn watcher_event(path: PathBuf, kind: EventKind) -> DebouncedEvent {
        DebouncedEvent::new(Event::new(kind).add_path(path), Instant::now())
    }
}
