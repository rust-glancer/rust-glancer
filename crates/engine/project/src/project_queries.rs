//! Opens the workspace's saved query cache. Ajika owns the filesystem lock.

use std::{ffi::OsStr, path::PathBuf, time::Duration};

use rg_source::{QueryDatabase, QueryServices};
use rg_workspace::WorkspaceMetadata;

// Change this epoch when persisted results or their producing semantics change.
const EPOCH: &str = "rust-glancer-target-analysis-7";
const RETENTION: ajika::Retention = ajika::Retention {
    max_entries: 1_000_000,
    // This caps durable storage, not resident memory. Large declaration graphs need enough room
    // to survive startup; otherwise the first body request rebuilds evicted dependency targets.
    max_bytes: 4 * 1024 * 1024 * 1024,
    max_idle: Duration::from_secs(30 * 24 * 60 * 60),
};

pub(crate) fn open(workspace: &WorkspaceMetadata) -> anyhow::Result<QueryDatabase> {
    let target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace.cargo_target_dir().to_path_buf());
    let name = workspace
        .workspace_root()
        .file_name()
        .unwrap_or_else(|| OsStr::new("workspace"));
    let root = target_dir.join("rust_glancer").join(name).join("queries");
    // Separate cache slots let multiple editor engines analyze one workspace concurrently.
    for slot in 1..=1024 {
        match QueryDatabase::open(
            root.join(slot.to_string()),
            EPOCH,
            ajika::Inputs::default(),
            QueryServices::default(),
            RETENTION,
        ) {
            Ok(database) => return Ok(database),
            Err(error) => match error.kind() {
                ajika::ErrorKind::Persistence(error)
                    if error.kind() == ajika::PersistenceErrorKind::InUse => {}
                _ => return Err(error.into()),
            },
        }
    }
    anyhow::bail!("no free query cache slots under {}", root.display())
}
