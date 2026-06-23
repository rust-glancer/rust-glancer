//! Filters LSP watched-file notifications before engine routing.
//!
//! The extension watches Rust source and Cargo inputs, but this server edge still owns URI
//! conversion, event-kind filtering, and editor-save echo suppression. Engine routing then receives
//! plain paths and can stay independent from LSP watcher details.

use std::path::{Path, PathBuf};

use tower_lsp_server::ls_types::*;

use crate::{
    engine_registry::EngineRegistry, methods::uri_to_path, recent_editor_saves::RecentEditorSaves,
};

pub(crate) async fn did_change_watched_files(
    registry: &EngineRegistry,
    recent_editor_saves: &RecentEditorSaves,
    params: DidChangeWatchedFilesParams,
) {
    let paths = watched_rust_paths(params);
    let paths = recent_editor_saves.saves_to_process(paths).await;
    if paths.is_empty() {
        return;
    }

    tracing::debug!(
        rust_paths = paths.len(),
        "routing external Rust project changes"
    );
    registry.external_project_paths_changed(paths).await;
}

fn watched_rust_paths(params: DidChangeWatchedFilesParams) -> Vec<PathBuf> {
    // We don't watch for deleted files, as file deletion is not really a
    // valid rust mechanic. If `mod foo;` is already deleted, we don't care
    // about this file anyway. If it still exists, the project is in an invalid
    // state as it will miss the module, and keeping analysis for it is not the
    // worst idea.
    // There is an edge case where it will be valid (e.g. removing an autodiscovery
    // target), but this is not worth supporting and will be processed by the next
    // save anyway.
    params
        .changes
        .into_iter()
        .filter(|change| {
            matches!(
                change.typ,
                FileChangeType::CREATED | FileChangeType::CHANGED
            )
        })
        .filter_map(|change| uri_to_path(&change.uri))
        .filter(|path| is_rust_path(path))
        .collect()
}

fn is_rust_path(path: &Path) -> bool {
    let file_name = path.file_name().and_then(|name| name.to_str());
    path.extension().and_then(|extension| extension.to_str()) == Some("rs")
        || matches!(file_name, Some("Cargo.toml" | "Cargo.lock"))
}

#[cfg(test)]
mod tests {
    use std::{path::Path, str::FromStr as _};

    use tower_lsp_server::ls_types::Uri;

    use super::*;

    #[test]
    fn watched_rust_paths_accepts_changed_and_created_rust_paths() {
        let root = std::env::current_dir().expect("test process should have a current directory");
        let created = root.join("src/generated.rs");
        let changed = root.join("src/lib.rs");
        let deleted = root.join("src/old.rs");
        let manifest = root.join("Cargo.toml");
        let lockfile = root.join("Cargo.lock");
        let notes = root.join("notes.md");

        let params = DidChangeWatchedFilesParams {
            changes: vec![
                file_event(&created, FileChangeType::CREATED),
                file_event(&changed, FileChangeType::CHANGED),
                file_event(&deleted, FileChangeType::DELETED),
                file_event(&manifest, FileChangeType::CHANGED),
                file_event(&lockfile, FileChangeType::CREATED),
                file_event(&notes, FileChangeType::CHANGED),
                FileEvent {
                    uri: Uri::from_str("untitled:Scratch.rs")
                        .expect("untitled URI should be valid"),
                    typ: FileChangeType::CHANGED,
                },
            ],
        };

        assert_eq!(
            watched_rust_paths(params),
            vec![created, changed, manifest, lockfile]
        );
    }

    fn file_event(path: &Path, typ: FileChangeType) -> FileEvent {
        FileEvent {
            uri: Uri::from_file_path(path).expect("test path should convert to URI"),
            typ,
        }
    }
}
