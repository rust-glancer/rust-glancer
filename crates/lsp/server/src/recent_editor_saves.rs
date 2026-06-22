//! File save de-duplication logic. We both watch file edits to detect edits
//! made outside of the editor, and we receive `did_save` events. These are not
//! deduplicated by the client, so we need to deduplicate them ourselves.
//!
//! We do it on the server, not in the engine, because engines already deal with
//! a lot of complex logic related to open documents and dirty states, so at
//! least this way we keep an invariant that incoming changes are not duplicates
//! of each other.

use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};

use tokio::sync::Mutex;

const RECENT_EDITOR_SAVE_TTL: Duration = Duration::from_secs(2);
const MAX_RECENT_EDITOR_SAVES: usize = 128;

/// Short-lived filter for watched-file echoes produced by ordinary editor saves.
///
/// We keep it as a LSP-ingress concern: it only remembers disk metadata that just came
/// through `textDocument/didSave`. This way the rest of the application can assume
/// that changes are not duplicated and we don't have to implement this logic on the
/// engine level.
#[derive(Debug, Default)]
pub(crate) struct RecentEditorSaves {
    inner: Mutex<RecentEditorSavesInner>,
}

impl RecentEditorSaves {
    /// Some file has been saved recently.
    pub(crate) async fn record_editor_save(&self, path: &Path) {
        self.inner.lock().await.record(path);
    }

    /// Removes paths that are recent echoes of editor saves.
    pub(crate) async fn saves_to_process(&self, paths: Vec<PathBuf>) -> Vec<PathBuf> {
        let mut inner = self.inner.lock().await;
        paths
            .into_iter()
            .filter(|path| !inner.is_save_echo(path))
            .collect()
    }
}

#[derive(Debug, Default)]
struct RecentEditorSavesInner {
    entries: Vec<RecentEditorSave>,
}

impl RecentEditorSavesInner {
    /// Some file has been saved recently.
    fn record(&mut self, path: &Path) {
        self.record_at(path, Instant::now());
    }

    /// Should we ignore save on this path, because the same save was recorded recently?
    fn is_save_echo(&mut self, path: &Path) -> bool {
        self.is_save_echo_at(path, Instant::now())
    }

    fn record_at(&mut self, path: &Path, now: Instant) {
        self.prune_expired(now);
        let Some(saved) = RecentEditorSave::from_path(path, now) else {
            return;
        };

        self.entries.retain(|entry| entry.path != saved.path);
        self.entries.push(saved);
        if self.entries.len() > MAX_RECENT_EDITOR_SAVES {
            self.entries.remove(0);
        }
    }

    fn is_save_echo_at(&mut self, path: &Path, now: Instant) -> bool {
        self.prune_expired(now);
        let Some((path, metadata)) = file_identity(path) else {
            return false;
        };

        self.entries
            .iter()
            .any(|entry| entry.path == path && entry.metadata == metadata)
    }

    fn prune_expired(&mut self, now: Instant) {
        self.entries.retain(|entry| {
            now.checked_duration_since(entry.recorded_at)
                .is_none_or(|elapsed| elapsed <= RECENT_EDITOR_SAVE_TTL)
        });
    }
}

/// What file was saved, which metadata did it have, and when did it happen.
#[derive(Clone, Debug)]
struct RecentEditorSave {
    path: PathBuf,
    metadata: FileIdentity,
    recorded_at: Instant,
}

impl RecentEditorSave {
    fn from_path(path: &Path, recorded_at: Instant) -> Option<Self> {
        let (path, metadata) = file_identity(path)?;
        Some(Self {
            path,
            metadata,
            recorded_at,
        })
    }
}

/// File metadata sufficient to distinguish different file versions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileIdentity {
    len: u64,
    modified: SystemTime,
}

fn file_identity(path: &Path) -> Option<(PathBuf, FileIdentity)> {
    let metadata = fs::metadata(path).ok()?;
    if !metadata.is_file() {
        return None;
    }

    Some((
        path.canonicalize().unwrap_or_else(|_| path.to_path_buf()),
        FileIdentity {
            len: metadata.len(),
            modified: metadata.modified().ok()?,
        },
    ))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use test_fixture::fixture_crate;

    use super::{RECENT_EDITOR_SAVE_TTL, RecentEditorSaves, RecentEditorSavesInner};

    #[tokio::test]
    async fn matching_saved_metadata_is_save_echo() {
        let fixture = fixture_crate(
            r#"
            //- /src/lib.rs
            pub fn saved() {}
            "#,
        );
        let path = fixture.path("src/lib.rs");
        let saves = RecentEditorSaves::default();

        saves.record_editor_save(&path).await;

        assert!(saves.saves_to_process(vec![path]).await.is_empty());
    }

    #[tokio::test]
    async fn changed_file_metadata_is_not_save_echo() {
        let fixture = fixture_crate(
            r#"
            //- /src/lib.rs
            pub fn saved() {}
            "#,
        );
        let path = fixture.path("src/lib.rs");
        let saves = RecentEditorSaves::default();

        saves.record_editor_save(&path).await;
        std::fs::write(&path, "pub fn external_agent_edit() {}\n")
            .expect("fixture file should be writable");

        assert_eq!(saves.saves_to_process(vec![path.clone()]).await, vec![path]);
    }

    #[tokio::test]
    async fn mixed_batches_keep_non_echo_paths() {
        let fixture = fixture_crate(
            r#"
            //- /src/lib.rs
            pub fn saved() {}

            //- /src/agent.rs
            pub fn external() {}
            "#,
        );
        let saved_path = fixture.path("src/lib.rs");
        let external_path = fixture.path("src/agent.rs");
        let saves = RecentEditorSaves::default();

        saves.record_editor_save(&saved_path).await;

        assert_eq!(
            saves
                .saves_to_process(vec![saved_path, external_path.clone()])
                .await,
            vec![external_path]
        );
    }

    #[test]
    fn expired_saved_metadata_is_not_save_echo() {
        let fixture = fixture_crate(
            r#"
            //- /src/lib.rs
            pub fn saved() {}
            "#,
        );
        let path = fixture.path("src/lib.rs");
        let now = std::time::Instant::now();
        let mut saves = RecentEditorSavesInner::default();

        saves.record_at(&path, now);

        assert!(!saves.is_save_echo_at(
            &path,
            now + RECENT_EDITOR_SAVE_TTL + Duration::from_millis(1)
        ));
    }
}
