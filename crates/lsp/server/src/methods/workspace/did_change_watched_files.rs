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
        changed_files = paths.len(),
        "routing external Rust source changes"
    );
    registry.external_project_paths_changed(paths).await;
}

fn watched_rust_source_paths(params: DidChangeWatchedFilesParams) -> Vec<PathBuf> {
    params
        .changes
        .into_iter()
        .filter(|change| change.typ == FileChangeType::CHANGED)
        .filter_map(|change| uri_to_path(&change.uri))
        .filter(|path| is_rust_source_path(path))
        .collect()
}

fn is_rust_source_path(path: &Path) -> bool {
    path.extension().and_then(|extension| extension.to_str()) == Some("rs")
}
