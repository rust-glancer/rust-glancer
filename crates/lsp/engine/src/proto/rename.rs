use std::collections::HashMap;

use anyhow::Context as _;
use ls_types::{PrepareRenameResponse, TextEdit, Uri, WorkspaceEdit};
use rg_analysis::{RenameEdit, RenameTarget};
use rg_def_map::PackageSlot;
use rg_parse::{FileId, Span};
use rg_project::ProjectSnapshot;

use crate::proto::position;

pub(crate) fn prepare_rename(
    snapshot: ProjectSnapshot<'_>,
    package: PackageSlot,
    target: RenameTarget,
) -> anyhow::Result<PrepareRenameResponse> {
    Ok(PrepareRenameResponse::RangeWithPlaceholder {
        range: range_for_file(snapshot, package, target.file_id, target.span)?,
        placeholder: target.placeholder,
    })
}

pub(crate) fn workspace_edit(
    snapshot: ProjectSnapshot<'_>,
    edits: Vec<RenameEdit>,
) -> anyhow::Result<WorkspaceEdit> {
    let mut changes = HashMap::<Uri, Vec<TextEdit>>::new();

    for edit in edits {
        let path = snapshot
            .file_path(edit.target.package, edit.file_id)
            .with_context(|| {
                format!(
                    "while attempting to find file path for rename edit in package {:?}, file {:?}",
                    edit.target.package, edit.file_id
                )
            })?;
        let uri = Uri::from_file_path(path).with_context(|| {
            format!(
                "while attempting to convert file path `{}` to URI for rename edit in package {:?}, file {:?}",
                path.display(),
                edit.target.package,
                edit.file_id
            )
        })?;
        let range = range_for_file(snapshot, edit.target.package, edit.file_id, edit.span)?;
        let text_edit = TextEdit {
            range,
            new_text: edit.new_text,
        };

        let file_edits = changes.entry(uri).or_default();
        if !file_edits.contains(&text_edit) {
            file_edits.push(text_edit);
        }
    }

    Ok(WorkspaceEdit::new(changes))
}

fn range_for_file(
    snapshot: ProjectSnapshot<'_>,
    package_slot: PackageSlot,
    file_id: FileId,
    span: Span,
) -> anyhow::Result<ls_types::Range> {
    let line_index = snapshot
        .file_line_index(package_slot, file_id)
        .context("while attempting to find file for rename range conversion")?;

    Ok(position::range(line_index, span))
}
