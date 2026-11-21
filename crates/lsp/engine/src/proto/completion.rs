use gen_lsp_types::{
    CompletionItem as LspCompletionItem, CompletionItemKind, CompletionItemTextEdit, Documentation,
    InsertTextFormat, MarkupContent, MarkupKind, TextEdit,
};
use rg_analysis::{
    CompletionAdditionalEdit, CompletionApplicability, CompletionEdit, CompletionInsertText,
    CompletionItem, CompletionKind,
};
use rg_lsp_proto::{CompletionCandidate, CompletionImport};
use rg_parse::LineIndex;

use crate::proto::{markdown, position, text_edit};

/// Convert one row at its position in the ranked list. Fixed-width `sortText` preserves that
/// order when the client compares the keys as strings.
pub(crate) fn completion_candidate(
    item: CompletionItem,
    line_index: &LineIndex,
    position: usize,
) -> CompletionCandidate {
    let detail = completion_detail(item.detail, item.applicability);
    let insert_text_format = completion_insert_text_format(&item.insert_text);
    let text_edit = completion_text_edit(&item.label, item.insert_text, item.edit, line_index);
    CompletionCandidate {
        item: LspCompletionItem {
            label: item.label,
            kind: Some(completion_kind(item.kind)),
            detail,
            documentation: item.documentation.and_then(markdown_documentation),
            sort_text: Some(format!("{position:016x}")),
            filter_text: item.filter_text,
            insert_text_format,
            text_edit,
            ..Default::default()
        },
        import: item.import.map(|import| CompletionImport {
            path: import.path.to_string(),
            anonymous: import.anonymous,
        }),
    }
}

pub(crate) fn import_edit(edit: CompletionAdditionalEdit, line_index: &LineIndex) -> TextEdit {
    text_edit::new(
        line_index,
        position::range(line_index, edit.replace),
        edit.new_text,
    )
}

/// Only items with an import can come back for semantic validation.
pub(crate) fn import_kind(kind: CompletionItemKind) -> Option<CompletionKind> {
    Some(match kind {
        CompletionItemKind::Method => CompletionKind::TraitMethod,
        CompletionItemKind::Function => CompletionKind::Function,
        CompletionItemKind::Constant => CompletionKind::Const,
        CompletionItemKind::Enum => CompletionKind::Enum,
        CompletionItemKind::EnumMember => CompletionKind::EnumVariant,
        CompletionItemKind::Variable => CompletionKind::Static,
        CompletionItemKind::Struct => CompletionKind::Struct,
        CompletionItemKind::Interface => CompletionKind::Trait,
        CompletionItemKind::Class => CompletionKind::TypeAlias,
        _ => return None,
    })
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
        CompletionAdditionalEdit, CompletionApplicability, CompletionEdit, CompletionImport,
        CompletionInsertText, CompletionItem, CompletionKind, CompletionTarget, KeywordCompletion,
    };
    use rg_ir_model::{Path, Span};
    use rg_parse::LineIndex;

    use super::{completion_candidate, import_edit};

    #[test]
    fn renders_plain_completion_metadata_and_replacement_edit() {
        let completion = completion_candidate(
            CompletionItem {
                label: "name".to_string(),
                filter_text: Some("na".to_string()),
                kind: CompletionKind::InherentMethod,
                target: CompletionTarget::Keyword(KeywordCompletion::Fn),
                applicability: CompletionApplicability::Maybe,
                detail: Some("fn name(&self)".to_string()),
                documentation: Some("Display name.".to_string()),
                relevance: Default::default(),
                insert_text: CompletionInsertText::Plain,
                edit: Some(CompletionEdit {
                    replace: Span { start: 5, end: 7 },
                }),
                import: None,
            },
            &LineIndex::new("user.na"),
            15,
        )
        .item;

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
        assert_eq!(completion.sort_text.as_deref(), Some("000000000000000f"));
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
    fn renders_crlf_snippet_and_resolved_import() {
        let line_index = LineIndex::new("HashM\r\n");
        let candidate = completion_candidate(
            CompletionItem {
                label: "HashMap".to_string(),
                filter_text: None,
                kind: CompletionKind::Keyword,
                target: CompletionTarget::Keyword(KeywordCompletion::Use),
                applicability: CompletionApplicability::Known,
                detail: None,
                documentation: None,
                relevance: Default::default(),
                insert_text: CompletionInsertText::Snippet(
                    "HashMap::<${1:K}, ${2:V}>$0\n".to_string(),
                ),
                edit: Some(CompletionEdit {
                    replace: Span { start: 0, end: 5 },
                }),
                import: Some(CompletionImport {
                    path: Path::from_text("std::collections::HashMap", None).expect("known path"),
                    anonymous: false,
                }),
            },
            &line_index,
            16,
        );
        let import = candidate.import.expect("auto-import payload");
        assert_eq!(import.path, "std::collections::HashMap");
        let completion = candidate.item;
        assert_eq!(completion.sort_text.as_deref(), Some("0000000000000010"));
        assert!(completion.additional_text_edits.is_none());

        assert_eq!(
            completion.insert_text_format,
            Some(InsertTextFormat::Snippet)
        );
        let Some(CompletionItemTextEdit::TextEdit(edit)) = completion.text_edit else {
            panic!("snippet completion should use a replacement text edit");
        };
        assert_eq!(edit.new_text, "HashMap::<${1:K}, ${2:V}>$0\r\n");

        let additional_edit = import_edit(
            CompletionAdditionalEdit {
                replace: Span { start: 0, end: 0 },
                new_text: "use std::collections::HashMap;\n".to_string(),
            },
            &line_index,
        );
        assert_eq!(
            additional_edit.new_text,
            "use std::collections::HashMap;\r\n"
        );
    }
}
