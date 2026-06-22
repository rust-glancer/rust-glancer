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
    let paths = watched_rust_source_paths(params);
    let paths = recent_editor_saves.saves_to_process(paths).await;
    if paths.is_empty() {
        return;
    }

    tracing::debug!(
        source_files = paths.len(),
        "routing external Rust source changes"
    );
    registry.external_project_paths_changed(paths).await;
}

fn watched_rust_source_paths(params: DidChangeWatchedFilesParams) -> Vec<PathBuf> {
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
        .filter(|path| is_rust_source_path(path))
        .collect()
}

fn is_rust_source_path(path: &Path) -> bool {
    path.extension().and_then(|extension| extension.to_str()) == Some("rs")
}

#[cfg(test)]
mod tests {
    use std::{path::Path, str::FromStr as _};

    use tower_lsp_server::ls_types::Uri;

    use super::*;

    #[test]
    fn watched_rust_source_paths_accepts_changed_and_created_rust_files() {
        let root = std::env::current_dir().expect("test process should have a current directory");
        let created = root.join("src/generated.rs");
        let changed = root.join("src/lib.rs");
        let deleted = root.join("src/old.rs");
        let manifest = root.join("Cargo.toml");

        let params = DidChangeWatchedFilesParams {
            changes: vec![
                file_event(&created, FileChangeType::CREATED),
                file_event(&changed, FileChangeType::CHANGED),
                file_event(&deleted, FileChangeType::DELETED),
                file_event(&manifest, FileChangeType::CHANGED),
                FileEvent {
                    uri: Uri::from_str("untitled:Scratch.rs")
                        .expect("untitled URI should be valid"),
                    typ: FileChangeType::CHANGED,
                },
            ],
        };

        assert_eq!(watched_rust_source_paths(params), vec![created, changed]);
    }

    fn file_event(path: &Path, typ: FileChangeType) -> FileEvent {
        FileEvent {
            uri: Uri::from_file_path(path).expect("test path should convert to URI"),
            typ,
        }
    }
}
