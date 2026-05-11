//! Live project updates after a project has already been built.

mod package;
mod source;
mod workspace;
mod workspace_graph;

use anyhow::Context as _;

use super::{AnalysisChangeSummary, Project, SavedFileChange};
use workspace_graph::WorkspaceGraphChanges;

pub(crate) use package::rebuild_resident_from_source;

pub(super) fn reindex_workspace(project: &mut Project) -> anyhow::Result<()> {
    workspace::rebuild_workspace_graph(project, &[])
        .context("while attempting to reindex analysis project from workspace root")?;
    Ok(())
}

pub(super) fn apply_changes(
    project: &mut Project,
    changes: Vec<SavedFileChange>,
) -> anyhow::Result<AnalysisChangeSummary> {
    let graph_changes = WorkspaceGraphChanges::check(
        project.state.workspace(),
        project.state.parse_db(),
        &changes,
    );

    match graph_changes {
        WorkspaceGraphChanges::Changed => workspace::rebuild_workspace_graph(project, &changes)
            .context("while attempting to rebuild analysis project after workspace change"),
        WorkspaceGraphChanges::Unchanged => source::apply_source_changes(project, changes),
    }
}
