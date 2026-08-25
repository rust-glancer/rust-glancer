use std::collections::HashMap;

use anyhow::Context as _;
use ls_types::{PrepareRenameResponse, TextEdit, Uri, WorkspaceEdit};
use rg_analysis::{RenameEdit, RenameTarget};
use rg_def_map::PackageSlot;
use rg_lsp_proto::path_to_file_uri;
use rg_parse::{FileId, Span};
use rg_project::ProjectSnapshot;

use crate::proto::{position, text_edit};

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
            .file_path(edit.crate_ref.package, edit.file_id)
            .with_context(|| {
                format!(
                    "while attempting to find file path for rename edit in package {:?}, file {:?}",
                    edit.crate_ref.package, edit.file_id
                )
            })?;
        let uri = path_to_file_uri(path).with_context(|| {
            format!(
                "while attempting to convert file path `{}` to URI for rename edit in package {:?}, file {:?}",
                path.display(),
                edit.crate_ref.package,
                edit.file_id
            )
        })?;
        let line_index = snapshot
            .file_line_index(edit.crate_ref.package, edit.file_id)?
            .context("while attempting to find file for rename edit conversion")?;
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

    Ok(WorkspaceEdit::new(changes))
}

fn range_for_file(
    snapshot: ProjectSnapshot<'_>,
    package_slot: PackageSlot,
    file_id: FileId,
    span: Span,
) -> anyhow::Result<ls_types::Range> {
    let line_index = snapshot
        .file_line_index(package_slot, file_id)?
        .context("while attempting to find file for rename range conversion")?;

    Ok(position::range(line_index, span))
}
