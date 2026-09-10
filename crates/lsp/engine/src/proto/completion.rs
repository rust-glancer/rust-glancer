use gen_lsp_types::{
    CompletionItem as LspCompletionItem, CompletionItemKind, CompletionItemTextEdit, Documentation,
    InsertTextFormat, MarkupContent, MarkupKind,
};
use rg_analysis::{
    CompletionApplicability, CompletionEdit, CompletionInsertText, CompletionItem, CompletionKind,
};
use rg_parse::LineIndex;

use crate::proto::{markdown, position, text_edit};

pub(crate) fn completion_item(item: CompletionItem, line_index: &LineIndex) -> LspCompletionItem {
    let detail = completion_detail(item.detail, item.applicability);
    let insert_text_format = completion_insert_text_format(&item.insert_text);
    let text_edit = completion_text_edit(&item.label, item.insert_text, item.edit, line_index);
    let additional_text_edits = (!item.additional_edits.is_empty()).then(|| {
        item.additional_edits
            .into_iter()
            .map(|edit| {
                text_edit::new(
                    line_index,
                    position::range(line_index, edit.replace),
                    edit.new_text,
                )
            })
            .collect()
    });

    LspCompletionItem {
        label: item.label,
        kind: Some(completion_kind(item.kind)),
        detail,
        documentation: item.documentation.and_then(markdown_documentation),
        sort_text: Some(item.sort_text),
        filter_text: item.filter_text,
        insert_text_format,
        text_edit,
        additional_text_edits,
        ..Default::default()
    }
}

fn completion_kind(kind: CompletionKind) -> CompletionItemKind {
    match kind {
        CompletionKind::Attribute => CompletionItemKind::Property,
        CompletionKind::Const => CompletionItemKind::Constant,
        CompletionKind::Enum => CompletionItemKind::Enum,
        CompletionKind::EnumVariant => CompletionItemKind::EnumMember,
        CompletionKind::Field => CompletionItemKind::Field,
        CompletionKind::Function => CompletionItemKind::Function,
        CompletionKind::InherentMethod | CompletionKind::TraitMethod => CompletionItemKind::Method,
        CompletionKind::Keyword => CompletionItemKind::Keyword,
        CompletionKind::Label | CompletionKind::Lifetime => CompletionItemKind::Reference,
        CompletionKind::Macro => CompletionItemKind::Function,
        CompletionKind::Module => CompletionItemKind::Module,
        CompletionKind::PrimitiveType => CompletionItemKind::Keyword,
        CompletionKind::Postfix => CompletionItemKind::Snippet,
        CompletionKind::Static => CompletionItemKind::Variable,
        CompletionKind::Struct | CompletionKind::Union => CompletionItemKind::Struct,
        CompletionKind::Trait => CompletionItemKind::Interface,
        CompletionKind::TypeAlias => CompletionItemKind::Class,
        CompletionKind::TypeParameter => CompletionItemKind::TypeParameter,
        CompletionKind::Variable => CompletionItemKind::Variable,
        CompletionKind::Value => CompletionItemKind::Value,
    }
}

fn completion_insert_text_format(insert_text: &CompletionInsertText) -> Option<InsertTextFormat> {
    match insert_text {
        CompletionInsertText::Plain | CompletionInsertText::Text(_) => None,
        CompletionInsertText::Snippet(_) => Some(InsertTextFormat::Snippet),
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
) -> Option<CompletionItemTextEdit> {
    edit.map(|edit| {
        let new_text = match insert_text {
            CompletionInsertText::Plain => label.to_string(),
            CompletionInsertText::Text(text) => text,
            CompletionInsertText::Snippet(snippet) => snippet,
        };
        CompletionItemTextEdit::TextEdit(text_edit::new(
            line_index,
            position::range(line_index, edit.replace),
            new_text,
        ))
    })
}

#[cfg(test)]
mod tests {
    use gen_lsp_types::{
        CompletionItemKind, CompletionItemTextEdit, Documentation, InsertTextFormat, MarkupContent,
        MarkupKind,
    };
    use rg_analysis::{
        CompletionAdditionalEdit, CompletionApplicability, CompletionEdit, CompletionInsertText,
        CompletionItem, CompletionKind, CompletionTarget, KeywordCompletion,
    };
    use rg_ir_model::{Span, TextSpan};
    use rg_parse::LineIndex;

    use super::completion_item;

    #[test]
    fn renders_plain_completion_metadata_and_replacement_edit() {
        let completion = completion_item(
            CompletionItem {
                label: "name".to_string(),
                filter_text: Some("na".to_string()),
                kind: CompletionKind::InherentMethod,
                target: CompletionTarget::Keyword(KeywordCompletion::Fn),
                applicability: CompletionApplicability::Maybe,
                detail: Some("fn name(&self)".to_string()),
                documentation: Some("Display name.".to_string()),
                sort_text: "name|01".to_string(),
                insert_text: CompletionInsertText::Plain,
                edit: Some(CompletionEdit {
                    replace: Span {
                        text: TextSpan { start: 5, end: 7 },
                    },
                }),
                additional_edits: Vec::new(),
            },
            &LineIndex::new("user.na"),
        );

        assert_eq!(completion.kind, Some(CompletionItemKind::Method));
        assert_eq!(
            completion.detail.as_deref(),
            Some("fn name(&self) (maybe applicable)")
        );
        assert_eq!(
            completion.documentation,
            Some(Documentation::MarkupContent(MarkupContent {
                kind: MarkupKind::Markdown,
                value: "Display name.".to_string(),
            }))
        );
        assert_eq!(completion.sort_text.as_deref(), Some("name|01"));
        assert_eq!(completion.filter_text.as_deref(), Some("na"));
        assert_eq!(completion.insert_text_format, None);
        assert_eq!(completion.additional_text_edits, None);

        let Some(CompletionItemTextEdit::TextEdit(edit)) = completion.text_edit else {
            panic!("completion should use a replacement text edit");
        };
        assert_eq!(edit.new_text, "name");
        assert_eq!(edit.range.start.line, 0);
        assert_eq!(edit.range.start.character, 5);
        assert_eq!(edit.range.end.line, 0);
        assert_eq!(edit.range.end.character, 7);
    }

    #[test]
    fn renders_crlf_snippet_and_additional_edits_through_the_whole_item() {
        let completion = completion_item(
            CompletionItem {
                label: "HashMap".to_string(),
                filter_text: None,
                kind: CompletionKind::Keyword,
                target: CompletionTarget::Keyword(KeywordCompletion::Use),
                applicability: CompletionApplicability::Known,
                detail: None,
                documentation: None,
                sort_text: String::new(),
                insert_text: CompletionInsertText::Snippet(
                    "HashMap::<${1:K}, ${2:V}>$0\n".to_string(),
                ),
                edit: Some(CompletionEdit {
                    replace: Span {
                        text: TextSpan { start: 0, end: 5 },
                    },
                }),
                additional_edits: vec![CompletionAdditionalEdit {
                    replace: Span {
                        text: TextSpan { start: 0, end: 0 },
                    },
                    new_text: "use std::collections::HashMap;\n".to_string(),
                }],
            },
            &LineIndex::new("HashM\r\n"),
        );

        assert_eq!(
            completion.insert_text_format,
            Some(InsertTextFormat::Snippet)
        );
        let Some(CompletionItemTextEdit::TextEdit(edit)) = completion.text_edit else {
            panic!("snippet completion should use a replacement text edit");
        };
        assert_eq!(edit.new_text, "HashMap::<${1:K}, ${2:V}>$0\r\n");

        let [additional_edit] = completion
            .additional_text_edits
            .as_deref()
            .expect("completion should contain its auto-import edit")
        else {
            panic!("completion should contain one auto-import edit");
        };
        assert_eq!(
            additional_edit.new_text,
            "use std::collections::HashMap;\r\n"
        );
    }
}
