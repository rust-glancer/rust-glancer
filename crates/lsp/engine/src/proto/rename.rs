use std::collections::HashMap;

use anyhow::Context as _;
use gen_lsp_types::{PrepareRenamePlaceholder, PrepareRenameResult, TextEdit, Uri, WorkspaceEdit};
use rg_analysis::{RenameEdit, RenameTarget};
use rg_lsp_proto::path_to_file_uri;
use rg_project::ProjectSnapshot;

use crate::proto::{position, text_edit};

pub(crate) fn prepare_rename(
    snapshot: ProjectSnapshot<'_>,
    target: RenameTarget,
) -> anyhow::Result<PrepareRenameResult> {
    Ok(PrepareRenameResult::PrepareRenamePlaceholder(
        PrepareRenamePlaceholder {
            range: position::range(snapshot.sources().line_index(target.file_id)?, target.span),
            placeholder: target.placeholder,
        },
    ))
}

pub(crate) fn workspace_edit(
    snapshot: ProjectSnapshot<'_>,
    edits: Vec<RenameEdit>,
    cancellation: &rg_std::CancellationToken,
) -> anyhow::Result<WorkspaceEdit> {
    let mut changes = HashMap::<Uri, Vec<TextEdit>>::new();

    for edit in edits {
        rg_std::check_cancel!(cancellation, "rename edit conversion");
        let path = snapshot.sources().path(edit.file_id)?;
        let uri = path_to_file_uri(path).with_context(|| {
            format!(
                "while attempting to convert file path `{}` to URI for rename edit in package {:?}, file {:?}",
                path.display(),
                edit.crate_ref.package,
                edit.file_id
            )
        })?;
        let line_index = snapshot.sources().line_index(edit.file_id)?;
        let text_edit = text_edit::new(
            line_index,
            position::range(line_index, edit.span),
            edit.new_text,
        );

        let file_edits = changes.entry(uri).or_default();
        if !file_edits.contains(&text_edit) {
            file_edits.push(text_edit);
        }
    }

    Ok(WorkspaceEdit {
        changes: Some(changes),
        ..Default::default()
    })
}
