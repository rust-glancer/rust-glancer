//! Detects changes that invalidate the workspace package graph.
//!
//! Source saves can usually reuse package and target slots, but manifest or lockfile edits may
//! add, remove, or reorder packages, dependencies, or targets. Those graph-level changes are
//! uncommon enough that the project intentionally treats them as a full-project rebuild boundary
//! instead of forcing every downstream phase to support slot remapping.
//!
//! Saved paths are canonicalized by `Project`, and workspace metadata paths are canonicalized when
//! `WorkspaceMetadata` is built. That lets this module express graph checks as direct path
//! comparisons instead of carrying defensive path-normalization fallbacks.

use rg_parse::ParseDb;
use rg_workspace::WorkspaceMetadata;

use crate::project::SavedFileChange;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WorkspaceGraphChanges {
    Changed,
    Unchanged,
}

impl WorkspaceGraphChanges {
    pub(super) fn check(
        workspace: &WorkspaceMetadata,
        parse: &ParseDb,
        changes: &[SavedFileChange],
    ) -> Self {
        let workspace_lockfile = workspace.workspace_root().join("Cargo.lock");
        let workspace_manifest = workspace.workspace_root().join("Cargo.toml");

        for change in changes {
            let path = change.path.as_path();

            // If `Cargo.lock` in workspace changed (e.g. `cargo update`, rebuild).
            if path == workspace_lockfile {
                return Self::Changed;
            }

            let Some(path_str) = path.to_str() else {
                continue;
            };

            // If any of `Cargo.toml` files changed, rebuild.
            // TODO: Is that needed/sufficient? If new dep is added, it might not be in `Cargo` cache
            // though probably `cargo check` will update `Cargo.lock` and it will trigger the rebuild
            // right after if that's the case. Low priority, to be tested later.
            if path_str.ends_with("Cargo.toml")
                && (path == workspace_manifest
                    || workspace
                        .workspace_packages()
                        .any(|package| package.manifest_path == path))
            {
                return Self::Changed;
            }

            if !path_str.ends_with(".rs") || parse.contains_file_path(path) {
                continue;
            }

            // This is deliberately a conservative heuristic for Cargo's default target
            // autodiscovery. Parsing each manifest just to honor rare `autotests = false`-style
            // settings is overkill for now; a full metadata reload asks Cargo for the final truth
            // if the saved path merely looks like it could introduce a target.
            for package in workspace.workspace_packages() {
                let package_root = package.root_dir();
                if path == package_root.join("src/main.rs") {
                    return Self::Changed;
                }

                let Some(package_root) = package_root.to_str() else {
                    continue;
                };
                let autodiscovery_roots = [
                    format!("{package_root}/src/bin/"),
                    format!("{package_root}/examples/"),
                    format!("{package_root}/tests/"),
                    format!("{package_root}/benches/"),
                ];

                if autodiscovery_roots.iter().any(|root| {
                    path_str
                        .strip_prefix(root)
                        .is_some_and(is_auto_discovered_target_file)
                }) {
                    return Self::Changed;
                }
            }
        }

        Self::Unchanged
    }
}

fn is_auto_discovered_target_file(path_in_target_dir: &str) -> bool {
    if path_in_target_dir.ends_with(".rs") && !path_in_target_dir.contains('/') {
        return true;
    }

    let Some((target_dir, target_root)) = path_in_target_dir.split_once('/') else {
        return false;
    };

    !target_dir.is_empty() && target_root == "main.rs"
}
