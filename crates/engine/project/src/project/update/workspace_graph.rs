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

use std::{
    ffi::OsStr,
    path::{Component, Path},
};

use rg_parse::ParseDb;
use rg_workspace::{CargoMetadataConfig, WorkspaceMetadata};

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
        cargo_metadata_config: &CargoMetadataConfig,
        change: &SavedFileChange,
    ) -> Self {
        let workspace_lockfile = workspace.workspace_root().join("Cargo.lock");
        let path = change.path.as_path();

        // If `Cargo.lock` in workspace changed (e.g. `cargo update`, rebuild).
        if path == workspace_lockfile {
            return Self::Changed;
        }

        if path.file_name() == Some(OsStr::new("Cargo.toml")) {
            // Existing graph manifests can change dependencies, target declarations, or workspace
            // membership policy, so they are always rebuild boundaries.
            if is_known_graph_manifest(workspace, path) {
                return Self::Changed;
            }

            // New packages under a workspace member glob are not visible in the old metadata graph.
            // Ask Cargo for a no-deps member list, but treat failures as "not discoverable yet":
            // editors often save a manifest before the package has a target file.
            if is_discoverable_workspace_member_manifest(workspace, cargo_metadata_config, path) {
                return Self::Changed;
            }
        }

        if path.extension() != Some(OsStr::new("rs")) || parse.contains_file_path(path) {
            return Self::Unchanged;
        }

        // This is deliberately a conservative heuristic for Cargo's default target
        // autodiscovery. Parsing each manifest just to honor rare `autotests = false`-style
        // settings is overkill for now; a full metadata reload asks Cargo for the final truth
        // if the saved path merely looks like it could introduce a target.
        for package in workspace.workspace_packages() {
            let package_root = package.root_dir();
            if path == package_root.join("src").join("main.rs") {
                return Self::Changed;
            }

            let autodiscovery_roots = [
                package_root.join("src").join("bin"),
                package_root.join("examples"),
                package_root.join("tests"),
                package_root.join("benches"),
            ];

            if autodiscovery_roots.iter().any(|root| {
                path.strip_prefix(root)
                    .is_ok_and(is_auto_discovered_target_file)
            }) {
                return Self::Changed;
            }
        }

        if let Some(manifest_path) = unknown_package_manifest_for_source_file(workspace, path)
            && is_discoverable_workspace_member_manifest(
                workspace,
                cargo_metadata_config,
                &manifest_path,
            )
        {
            return Self::Changed;
        }

        Self::Unchanged
    }
}

/// Returns whether `path` is already part of the current Cargo metadata graph.
///
/// Saves to these manifests are always graph-level changes: even if the package remains present,
/// Cargo may change package ordering, target declarations, dependency edges, or workspace policy.
fn is_known_graph_manifest(workspace: &WorkspaceMetadata, path: &Path) -> bool {
    path == workspace.workspace_root().join("Cargo.toml")
        || workspace
            .workspace_packages()
            .any(|package| package.manifest_path == path)
}

/// Asks Cargo whether an unknown manifest is now a valid workspace member.
///
/// This is intentionally a narrow `cargo metadata --no-deps` probe used only for candidate new
/// packages. If Cargo rejects the workspace because the package is half-written, save handling
/// leaves the existing graph intact and waits for a later save to make the package discoverable.
fn is_discoverable_workspace_member_manifest(
    workspace: &WorkspaceMetadata,
    cargo_metadata_config: &CargoMetadataConfig,
    candidate_manifest: &Path,
) -> bool {
    if !candidate_manifest.starts_with(workspace.workspace_root()) {
        return false;
    }

    let workspace_manifest = workspace.workspace_root().join("Cargo.toml");
    let Ok(member_manifests) =
        cargo_metadata_config.load_workspace_member_manifest_paths(workspace_manifest)
    else {
        return false;
    };

    member_manifests
        .iter()
        .any(|member_manifest| member_manifest == candidate_manifest)
}

/// Finds a nearby unknown package manifest for a saved source file.
///
/// This handles the common package-creation order where the manifest is saved before Cargo can
/// accept it, then `src/lib.rs` or `src/main.rs` is saved later. Walking up from the source path
/// lets that later source save retry discovery without scanning the whole workspace tree.
fn unknown_package_manifest_for_source_file(
    workspace: &WorkspaceMetadata,
    source_path: &Path,
) -> Option<std::path::PathBuf> {
    if !source_path.starts_with(workspace.workspace_root()) {
        return None;
    }

    for directory in source_path.ancestors().skip(1) {
        if directory == workspace.workspace_root() {
            return None;
        }

        let manifest_path = directory.join("Cargo.toml");
        if manifest_path.is_file() && !is_known_graph_manifest(workspace, &manifest_path) {
            return Some(manifest_path);
        }
    }

    None
}

fn is_auto_discovered_target_file(path_in_target_dir: &Path) -> bool {
    let mut components = path_in_target_dir.components();
    let Some(Component::Normal(target_name)) = components.next() else {
        return false;
    };

    let Some(target_root) = components.next() else {
        return Path::new(target_name).extension() == Some(OsStr::new("rs"));
    };

    components.next().is_none()
        && !target_name.is_empty()
        && target_root.as_os_str() == OsStr::new("main.rs")
}
