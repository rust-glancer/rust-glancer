use ls_types::{
    CompletionItem as LspCompletionItem, CompletionItemKind, CompletionTextEdit, Documentation,
    InsertTextFormat, MarkupContent, MarkupKind, TextEdit,
};
use rg_analysis::{
    CompletionApplicability, CompletionEdit, CompletionInsertText, CompletionItem, CompletionKind,
};
use rg_parse::LineIndex;

use crate::proto::{markdown, position};

pub(crate) fn completion_item(item: CompletionItem, line_index: &LineIndex) -> LspCompletionItem {
    let detail = completion_detail(item.detail, item.applicability);
    let insert_text_format = completion_insert_text_format(&item.insert_text);
    let text_edit = completion_text_edit(&item.label, item.insert_text, item.edit, line_index);

    LspCompletionItem {
        label: item.label,
        kind: Some(completion_kind(item.kind)),
        detail,
        documentation: item.documentation.and_then(markdown_documentation),
        sort_text: Some(item.sort_text),
        insert_text_format,
        text_edit,
        ..Default::default()
    }
}

fn completion_kind(kind: CompletionKind) -> CompletionItemKind {
    match kind {
        CompletionKind::Const => CompletionItemKind::CONSTANT,
        CompletionKind::Enum => CompletionItemKind::ENUM,
        CompletionKind::EnumVariant => CompletionItemKind::ENUM_MEMBER,
        CompletionKind::Field => CompletionItemKind::FIELD,
        CompletionKind::Function => CompletionItemKind::FUNCTION,
        CompletionKind::InherentMethod | CompletionKind::TraitMethod => CompletionItemKind::METHOD,
        CompletionKind::Keyword => CompletionItemKind::KEYWORD,
        CompletionKind::Macro => CompletionItemKind::FUNCTION,
        CompletionKind::Module => CompletionItemKind::MODULE,
        CompletionKind::Static => CompletionItemKind::VARIABLE,
        CompletionKind::Struct | CompletionKind::Union => CompletionItemKind::STRUCT,
        CompletionKind::Trait => CompletionItemKind::INTERFACE,
        CompletionKind::TypeAlias => CompletionItemKind::CLASS,
        CompletionKind::Variable => CompletionItemKind::VARIABLE,
    }
}

fn completion_insert_text_format(insert_text: &CompletionInsertText) -> Option<InsertTextFormat> {
    match insert_text {
        CompletionInsertText::Plain => None,
        CompletionInsertText::Snippet(_) => Some(InsertTextFormat::SNIPPET),
    }
}

fn completion_detail(
    detail: Option<String>,
    applicability: CompletionApplicability,
) -> Option<String> {
    match applicability {
        CompletionApplicability::Known => detail,
        CompletionApplicability::Maybe => Some(match detail {
            Some(detail) => format!("{detail} (maybe applicable)"),
            None => "maybe applicable".to_string(),
        }),
    }
}

fn markdown_documentation(value: String) -> Option<Documentation> {
    let value = markdown::render_rustdoc_markdown(&value)?;
    Some(Documentation::MarkupContent(MarkupContent {
        kind: MarkupKind::Markdown,
        value,
    }))
}

fn completion_text_edit(
    label: &str,
    insert_text: CompletionInsertText,
    edit: Option<CompletionEdit>,
    line_index: &LineIndex,
) -> Option<CompletionTextEdit> {
    edit.map(|edit| {
        let new_text = match insert_text {
            CompletionInsertText::Plain => label.to_string(),
            CompletionInsertText::Snippet(snippet) => snippet,
        };
        CompletionTextEdit::Edit(TextEdit {
            range: position::range(line_index, edit.replace),
            new_text,
        })
    })
}

#[cfg(test)]
mod tests {
    use ls_types::{CompletionTextEdit, Documentation, MarkupKind};
    use rg_analysis::{CompletionApplicability, CompletionEdit, CompletionInsertText};
    use rg_parse::{LineIndex, Span, TextSpan};

    use super::{
        completion_detail, completion_insert_text_format, completion_text_edit,
        markdown_documentation,
    };

    #[test]
    fn renders_metadata_and_replacement_edit() {
        let line_index = LineIndex::new("user.na");
        let edit = completion_text_edit(
            "name",
            CompletionInsertText::Plain,
            Some(CompletionEdit {
                replace: Span {
                    text: TextSpan { start: 5, end: 7 },
                },
            }),
            &line_index,
        );

        let Some(CompletionTextEdit::Edit(edit)) = edit else {
            panic!("completion should use a replacement text edit");
        };
        assert_eq!(edit.new_text, "name");
        assert_eq!(edit.range.start.line, 0);
        assert_eq!(edit.range.start.character, 5);
        assert_eq!(edit.range.end.line, 0);
        assert_eq!(edit.range.end.character, 7);

        assert_eq!(
            completion_detail(
                Some("fn name(&self)".to_string()),
                CompletionApplicability::Maybe
            )
            .as_deref(),
            Some("fn name(&self) (maybe applicable)")
        );

        let Some(Documentation::MarkupContent(docs)) =
            markdown_documentation("Display name.".to_string())
        else {
            panic!("completion should render markdown documentation");
        };
        assert_eq!(docs.kind, MarkupKind::Markdown);
        assert_eq!(docs.value, "Display name.");
    }

    #[test]
    fn renders_snippet_completion_text() {
        let line_index = LineIndex::new("fn");
        let insert_text =
            CompletionInsertText::Snippet("fn ${1:name}(${2:args}) {\n    $0\n}".to_string());
        let edit = completion_text_edit(
            "fn",
            insert_text.clone(),
            Some(CompletionEdit {
                replace: Span {
                    text: TextSpan { start: 0, end: 2 },
                },
            }),
            &line_index,
        );

        assert_eq!(
            completion_insert_text_format(&insert_text),
            Some(ls_types::InsertTextFormat::SNIPPET)
        );
        let Some(CompletionTextEdit::Edit(edit)) = edit else {
            panic!("snippet completion should use a replacement text edit");
        };
        assert_eq!(edit.new_text, "fn ${1:name}(${2:args}) {\n    $0\n}");
    }
}
