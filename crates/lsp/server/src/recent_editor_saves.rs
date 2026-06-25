//! Filters repeated incoming file changes.
//!
//! Editor saves can also show up as watched-file changes. We keep a short cache of recently saved
//! files and ignore watched changes that still match the same file metadata.

use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};

use tokio::sync::Mutex;

const RECENT_EDITOR_SAVE_TTL: Duration = Duration::from_secs(2);
const MAX_RECENT_EDITOR_SAVES: usize = 128;

/// Filter for incoming file changes that repeat a recent editor save.
///
/// If file metadata changes after the save, for example because `rustfmt` rewrote the file, the
/// watched change goes through.
#[derive(Debug, Default)]
pub(crate) struct RecentEditorSaves {
    inner: Mutex<RecentEditorSavesInner>,
}

impl RecentEditorSaves {
    /// Remember the current disk identity of a file just saved by the editor.
    pub(crate) async fn record_editor_save(&self, path: &Path) {
        self.inner.lock().await.record(path);
    }

    /// Drop watched paths whose current disk identity matches a recent editor save.
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

/// One recorded editor save plus the timestamp used for cache expiry.
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

/// Disk identity used to decide whether a watched event is the same save.
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
