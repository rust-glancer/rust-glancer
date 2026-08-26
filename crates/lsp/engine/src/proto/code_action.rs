//! Conversion from analysis actions to LSP actions that already contain their document edits.
//!
//! Analysis actions still contain UTF-8 spans and know nothing about document identity. This is
//! the single boundary that converts those spans to UTF-16 ranges and attaches the captured URI
//! and document version, so an editor cannot apply the action to a different buffer revision.

use anyhow::Context as _;
use ls_types::{
    CodeAction as LspCodeAction, CodeActionKind as LspCodeActionKind, DocumentChanges, OneOf,
    OptionalVersionedTextDocumentIdentifier, TextDocumentEdit, WorkspaceEdit,
};
use rg_analysis::{CodeAction, CodeActionKind};
use rg_lsp_proto::path_to_file_uri;
use rg_parse::LineIndex;

use crate::proto::{position, text_edit};

/// Convert one validated single-document action into an LSP action with a versioned workspace
/// edit.
pub(crate) fn code_action(
    document_path: &std::path::Path,
    document_version: Option<i32>,
    line_index: &LineIndex,
    action: CodeAction,
) -> anyhow::Result<LspCodeAction> {
    let uri = path_to_file_uri(document_path).with_context(|| {
        format!(
            "while attempting to convert code-action path `{}` to URI",
            document_path.display()
        )
    })?;
    let edits = action
        .edits
        .into_iter()
        .map(|edit| {
            OneOf::Left(text_edit::new(
                line_index,
                position::range(line_index, edit.replace),
                edit.new_text,
            ))
        })
        .collect();
    let document_edit = TextDocumentEdit {
        text_document: OptionalVersionedTextDocumentIdentifier {
            uri,
            version: document_version,
        },
        edits,
    };
    Ok(LspCodeAction {
        title: action.title,
        kind: Some(match action.kind {
            CodeActionKind::QuickFix => LspCodeActionKind::QUICKFIX,
            CodeActionKind::RefactorRewrite => LspCodeActionKind::REFACTOR_REWRITE,
        }),
        diagnostics: None,
        edit: Some(WorkspaceEdit {
            changes: None,
            document_changes: Some(DocumentChanges::Edits(vec![document_edit])),
            change_annotations: None,
        }),
        command: None,
        is_preferred: action.is_preferred.then_some(true),
        disabled: None,
        data: None,
    })
}

#[cfg(test)]
mod tests {
    use ls_types::{DocumentChanges, OneOf};
    use rg_analysis::{CodeAction, CodeActionEdit, CodeActionKind};
    use rg_parse::{LineIndex, Span, TextSpan};
    use test_fixture::synthetic_test_path;

    use super::code_action;

    #[test]
    fn renders_code_action_edits_with_crlf() {
        let document_path = synthetic_test_path("workspace/main.rs");
        let action = CodeAction {
            title: "Import `crate::User`".to_string(),
            kind: CodeActionKind::QuickFix,
            is_preferred: true,
            edits: vec![CodeActionEdit {
                replace: Span {
                    text: TextSpan { start: 0, end: 0 },
                },
                new_text: "use crate::User;\n\n".to_string(),
            }],
        };

        let action = code_action(
            &document_path,
            Some(7),
            &LineIndex::new("fn main() {}\r\n"),
            action,
        )
        .expect("code action should convert to LSP");
        let Some(workspace_edit) = action.edit else {
            panic!("code action should contain a workspace edit");
        };
        let Some(DocumentChanges::Edits(document_edits)) = workspace_edit.document_changes else {
            panic!("workspace edit should contain versioned document edits");
        };
        let [document_edit] = document_edits.as_slice() else {
            panic!("workspace edit should contain one document edit");
        };
        let [OneOf::Left(edit)] = document_edit.edits.as_slice() else {
            panic!("document edit should contain one plain text edit");
        };

        assert_eq!(edit.new_text, "use crate::User;\r\n\r\n");
    }
}
