//! Live project updates after a project has already been built.

mod package;
mod source;
mod workspace;
mod workspace_graph;

use anyhow::Context as _;
use rg_def_map::PackageSlot;

use super::{AnalysisChangeSummary, ChangedFile, Project, SavedFileChange};
use workspace_graph::WorkspaceGraphChanges;

pub(crate) use package::rebuild_resident_from_source;

/// Whether one canonical path batch changed the candidate's published project state.
pub(super) enum ProjectChangeApplication {
    Unchanged,
    Applied(AnalysisChangeSummary),
}

pub(super) fn reindex_workspace(project: &mut Project) -> anyhow::Result<()> {
    workspace::rebuild_workspace_graph(project, &[])
        .context("while attempting to reindex analysis project from workspace root")?;
    Ok(())
}

/// Applies one nonempty canonicalized change batch to a private project candidate.
pub(super) fn apply_canonical_changes(
    project: &mut Project,
    changes: Vec<SavedFileChange>,
) -> anyhow::Result<ProjectChangeApplication> {
    debug_assert!(
        !changes.is_empty(),
        "candidate updates should contain at least one canonical change",
    );

    let graph_changed = changes.iter().any(|change| {
        WorkspaceGraphChanges::check(
            project.state.workspace(),
            project.state.parse_db(),
            &project.state.cargo_metadata_config,
            change,
        ) == WorkspaceGraphChanges::Changed
    });

    if graph_changed {
        workspace::rebuild_workspace_graph(project, &changes)
            .map(ProjectChangeApplication::Applied)
            .context("while attempting to rebuild analysis project after workspace change")
    } else {
        let summary = source::apply_source_changes(project, changes)
            .context("while attempting to apply saved source changes")?;
        if summary.is_empty() {
            Ok(ProjectChangeApplication::Unchanged)
        } else {
            Ok(ProjectChangeApplication::Applied(summary))
        }
    }
}

pub(super) fn affected_packages(
    project: &Project,
    changed_files: &[ChangedFile],
    fallback_package_roots: &[PackageSlot],
) -> Vec<PackageSlot> {
    let changed_package_ids = changed_packages(changed_files, fallback_package_roots)
        .iter()
        .filter_map(|package_slot| {
            project
                .state
                .workspace()
                .packages()
                .get(package_slot.0)
                .map(|package| package.id.clone())
        })
        .collect::<Vec<_>>();

    project
        .state
        .workspace()
        .reverse_dependency_closure(&changed_package_ids)
        .into_iter()
        .map(PackageSlot)
        .collect()
}

pub(super) fn changed_packages(
    changed_files: &[ChangedFile],
    fallback_package_roots: &[PackageSlot],
) -> Vec<PackageSlot> {
    let mut packages = changed_files
        .iter()
        .map(|changed_file| changed_file.package)
        .chain(fallback_package_roots.iter().copied())
        .collect::<Vec<_>>();
    packages.sort_by_key(|package| package.0);
    packages.dedup();
    packages
}
