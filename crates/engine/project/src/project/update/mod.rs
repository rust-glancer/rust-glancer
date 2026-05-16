//! Live project updates after a project has already been built.

mod package;
mod source;
mod workspace;
mod workspace_graph;

use anyhow::Context as _;
use rg_body_ir::BodyIrFile;
use rg_def_map::PackageSlot;

use super::{AnalysisChangeSummary, ChangedFile, Project, SavedFileChange, state::ProjectState};
use workspace_graph::WorkspaceGraphChanges;

pub(crate) use package::rebuild_resident_from_source;

pub(super) fn reindex_workspace(project: &mut Project) -> anyhow::Result<()> {
    workspace::rebuild_workspace_graph(project, None)
        .context("while attempting to reindex analysis project from workspace root")?;
    Ok(())
}

pub(super) fn apply_change(
    project: &mut Project,
    change: SavedFileChange,
) -> anyhow::Result<AnalysisChangeSummary> {
    let graph_changes = WorkspaceGraphChanges::check(
        project.state.workspace(),
        project.state.parse_db(),
        &project.state.cargo_metadata_config,
        &change,
    );

    match graph_changes {
        WorkspaceGraphChanges::Changed => {
            workspace::rebuild_workspace_graph(project, Some(&change))
                .context("while attempting to rebuild analysis project after workspace change")
        }
        WorkspaceGraphChanges::Unchanged => source::apply_source_change(project, change),
    }
}

pub(super) fn rebuild_dirty_overlay_packages(
    state: &mut ProjectState,
    packages: &[PackageSlot],
    body_files: &[BodyIrFile],
) -> anyhow::Result<()> {
    package::rebuild_dirty_overlay_packages(state, packages, body_files)
}

pub(super) fn affected_packages(
    project: &Project,
    changed_files: &[ChangedFile],
    fallback_package_roots: &[PackageSlot],
) -> Vec<PackageSlot> {
    let mut changed_package_ids = changed_files
        .iter()
        .filter_map(|changed_file| {
            project
                .state
                .workspace()
                .packages()
                .get(changed_file.package.0)
                .map(|package| package.id.clone())
        })
        .collect::<Vec<_>>();

    for package_slot in fallback_package_roots {
        let Some(package) = project.state.workspace().packages().get(package_slot.0) else {
            continue;
        };
        if !changed_package_ids.contains(&package.id) {
            changed_package_ids.push(package.id.clone());
        }
    }

    project
        .state
        .workspace()
        .reverse_dependency_closure(&changed_package_ids)
        .into_iter()
        .map(PackageSlot)
        .collect()
}
