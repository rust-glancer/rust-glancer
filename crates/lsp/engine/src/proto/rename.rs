use std::collections::HashMap;

use anyhow::Context as _;
use gen_lsp_types::{
    DocumentChange, Edit, OptionalVersionedTextDocumentIdentifier, PrepareRenamePlaceholder,
    PrepareRenameResult, TextDocumentEdit, TextDocumentIdentifier, TextEdit, Uri, WorkspaceEdit,
};
use rg_analysis::{RenameEdit, RenameTarget};
use rg_lsp_proto::{EditorSnapshot, path_to_file_uri};
use rg_project::ProjectSnapshot;
use rg_std::CancellationToken;

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
    editor: &EditorSnapshot,
    edits: Vec<RenameEdit>,
    versioned_document_edits: bool,
    cancellation: &CancellationToken,
) -> anyhow::Result<WorkspaceEdit> {
    let mut changes = HashMap::<Uri, (Option<i32>, Vec<TextEdit>)>::new();

    for edit in edits {
        rg_std::check_cancel!(cancellation, "rename edit conversion");
        let path = snapshot.sources().path(edit.file_id)?;
        let document = editor.document_for_source(path);
        let path = document.map_or(path, |document| document.path());
        let version = document.and_then(|document| document.client_version());
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

        let (_, file_edits) = changes.entry(uri).or_insert_with(|| (version, Vec::new()));
        if !file_edits.contains(&text_edit) {
            file_edits.push(text_edit);
        }
    }

    if versioned_document_edits {
        let document_changes = changes
            .into_iter()
            .map(|(uri, (version, edits))| {
                DocumentChange::TextDocumentEdit(TextDocumentEdit {
                    text_document: OptionalVersionedTextDocumentIdentifier {
                        text_document_identifier: TextDocumentIdentifier { uri },
                        version,
                    },
                    edits: edits.into_iter().map(Edit::TextEdit).collect(),
                })
            })
            .collect();
        Ok(WorkspaceEdit {
            document_changes: Some(document_changes),
            ..Default::default()
        })
    } else {
        Ok(WorkspaceEdit {
            changes: Some(
                changes
                    .into_iter()
                    .map(|(uri, (_, edits))| (uri, edits))
                    .collect(),
            ),
            ..Default::default()
        })
    }
}
