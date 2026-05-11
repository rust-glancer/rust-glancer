//! Rebuilds the whole analysis project after workspace graph changes.
//!
//! When Cargo metadata can change package, target, or dependency slots, partial reuse becomes more
//! dangerous than useful. This path reloads metadata and rebuilds every non-sysroot package so the
//! downstream phase databases return to a single consistent snapshot.

use anyhow::Context as _;

use rg_workspace::WorkspaceMetadata;

use crate::project::{AnalysisChangeSummary, ChangedFile, Project, SavedFileChange};

pub(super) fn rebuild_workspace_graph(
    project: &mut Project,
    changes: &[SavedFileChange],
) -> anyhow::Result<AnalysisChangeSummary> {
    let manifest_path = project
        .state
        .workspace()
        .workspace_root()
        .join("Cargo.toml");
    let sysroot = project.state.workspace().sysroot_sources();
    let cargo_metadata_config = project.state.cargo_metadata_config.clone();
    let workspace =
        WorkspaceMetadata::from_manifest_path_with_config(&manifest_path, &cargo_metadata_config)
            .with_context(|| format!("while attempting to load {}", manifest_path.display()))?
            .with_sysroot_sources(sysroot);
    let body_ir_policy = project.state.body_ir_policy;
    let package_residency_policy = project.state.package_residency_policy;

    // Cargo graph edits can add, remove, or reorder packages, targets, and dependencies. Starting
    // from scratch keeps every phase on one slot-stable snapshot instead of trying to partially
    // reuse state whose internal ids may no longer describe the refreshed metadata graph.
    project.state = Project::builder(workspace)
        .cargo_metadata_config(cargo_metadata_config)
        .body_ir_policy(body_ir_policy)
        .package_residency_policy(package_residency_policy)
        .build()
        .context("while attempting to build refreshed analysis project")?
        .into_project()
        .state;

    let changed_files = changed_source_files_for_saved_paths(project, changes);
    let mut affected_packages = Vec::new();
    let mut changed_targets = Vec::new();

    // The rebuilt project has already restored its package residency, so payload-heavy phase
    // databases may be empty under aggressive offloading. ProjectState keeps the small graph
    // metadata that change summaries need resident.
    for package_slot in project.state.non_sysroot_package_slots() {
        affected_packages.push(package_slot);
        changed_targets.extend(project.state.target_refs_for_package(package_slot));
    }

    Ok(AnalysisChangeSummary {
        changed_files,
        affected_packages,
        changed_targets,
    })
}

fn changed_source_files_for_saved_paths(
    project: &Project,
    changes: &[SavedFileChange],
) -> Vec<ChangedFile> {
    let mut changed_files = Vec::new();

    for change in changes {
        for file in project.state.file_refs_for_path(&change.path) {
            let changed_file = ChangedFile {
                package: file.package,
                file: file.file,
            };
            if !changed_files.contains(&changed_file) {
                changed_files.push(changed_file);
            }
        }
    }

    changed_files
}
