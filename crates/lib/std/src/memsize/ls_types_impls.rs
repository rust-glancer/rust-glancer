use std::mem;

use super::{MemoryRecorder, MemorySize};

super::impl_memory_size_leaf!(
    ls_types::CompletionItemKind,
    ls_types::CompletionItemTag,
    ls_types::DiagnosticSeverity,
    ls_types::DiagnosticTag,
    ls_types::InlayHintKind,
    ls_types::InsertTextFormat,
    ls_types::InsertTextMode,
    ls_types::MarkupKind,
    ls_types::MessageType,
    ls_types::Position,
    ls_types::SymbolKind,
    ls_types::SymbolTag,
);

super::impl_memory_size_children! {
    ls_types::Range => start, end;
    ls_types::Location => uri, range;
    ls_types::LocationLink => origin_selection_range, target_uri, target_range,
        target_selection_range;
    ls_types::Diagnostic => range, severity, code, code_description, source, message,
        related_information, tags, data;
    ls_types::CodeDescription => href;
    ls_types::DiagnosticRelatedInformation => location, message;
    ls_types::Command => title, command, arguments;
    ls_types::TextEdit => range, new_text;
    ls_types::DocumentSymbol => name, detail, kind, tags, #[allow(deprecated)] deprecated, range,
        selection_range, children;
    ls_types::SymbolInformation => name, kind, tags, #[allow(deprecated)] deprecated, location,
        container_name;
    ls_types::WorkspaceLocation => uri;
    ls_types::WorkspaceSymbol => name, kind, tags, container_name, location, data;
    ls_types::InsertReplaceEdit => new_text, insert, replace;
    ls_types::CompletionItemLabelDetails => detail, description;
    ls_types::CompletionItem => label, label_details, kind, detail, documentation, deprecated,
        preselect, sort_text, filter_text, insert_text, insert_text_format, insert_text_mode,
        text_edit, additional_text_edits, command, commit_characters, data, tags;
    ls_types::LanguageString => language, value;
    ls_types::MarkupContent => kind, value;
    ls_types::Hover => contents, range;
    ls_types::InlayHint => position, label, kind, text_edits, tooltip, padding_left, padding_right,
        data;
    ls_types::InlayHintLabelPart => value, tooltip, location, command;
}

impl<A, B> MemorySize for ls_types::OneOf<A, B>
where
    A: MemorySize,
    B: MemorySize,
{
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            ls_types::OneOf::Left(value) => {
                recorder.scope("left", |recorder| value.record_memory_children(recorder));
            }
            ls_types::OneOf::Right(value) => {
                recorder.scope("right", |recorder| value.record_memory_children(recorder));
            }
        }
    }
}

impl MemorySize for ls_types::NumberOrString {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            ls_types::NumberOrString::Number(_) => {}
            ls_types::NumberOrString::String(value) => {
                recorder.scope("string", |recorder| value.record_memory_children(recorder));
            }
        }
    }
}

impl MemorySize for ls_types::LSPAny {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            ls_types::LSPAny::Null | ls_types::LSPAny::Bool(_) | ls_types::LSPAny::Number(_) => {}
            ls_types::LSPAny::String(value) => {
                recorder.scope("string", |recorder| value.record_memory_children(recorder));
            }
            ls_types::LSPAny::Array(items) => {
                recorder.scope("array", |recorder| items.record_memory_children(recorder));
            }
            ls_types::LSPAny::Object(object) => {
                recorder.scope("object", |recorder| object.record_memory_children(recorder));
            }
        }
    }
}

impl MemorySize for ls_types::LSPObject {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        // serde_json hides whether the map is backed by BTreeMap or IndexMap. Count initialized
        // entries and mark their storage as approximate rather than pretending to know node layout.
        recorder.record_approximate::<ls_types::LSPObject>(self.len().saturating_mul(
            mem::size_of::<String>().saturating_add(mem::size_of::<ls_types::LSPAny>()),
        ));

        recorder.scope("entries", |recorder| {
            for (key, value) in self {
                recorder.scope("key", |recorder| key.record_memory_children(recorder));
                recorder.scope("value", |recorder| value.record_memory_children(recorder));
            }
        });
    }
}

impl MemorySize for ls_types::Uri {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        recorder.record_approximate::<ls_types::Uri>(self.as_str().len());
    }
}

impl MemorySize for ls_types::WorkspaceSymbolResponse {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            ls_types::WorkspaceSymbolResponse::Flat(symbols) => {
                recorder.scope("flat", |recorder| symbols.record_memory_children(recorder));
            }
            ls_types::WorkspaceSymbolResponse::Nested(symbols) => {
                recorder.scope("nested", |recorder| {
                    symbols.record_memory_children(recorder)
                });
            }
        }
    }
}

impl MemorySize for ls_types::CompletionTextEdit {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            ls_types::CompletionTextEdit::Edit(edit) => {
                recorder.scope("edit", |recorder| edit.record_memory_children(recorder));
            }
            ls_types::CompletionTextEdit::InsertAndReplace(edit) => {
                recorder.scope("insert_replace", |recorder| {
                    edit.record_memory_children(recorder)
                });
            }
        }
    }
}

impl MemorySize for ls_types::Documentation {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            ls_types::Documentation::String(value) => {
                recorder.scope("string", |recorder| value.record_memory_children(recorder));
            }
            ls_types::Documentation::MarkupContent(markup) => {
                recorder.scope("markup", |recorder| markup.record_memory_children(recorder));
            }
        }
    }
}

impl MemorySize for ls_types::MarkedString {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            ls_types::MarkedString::String(value) => {
                recorder.scope("string", |recorder| value.record_memory_children(recorder));
            }
            ls_types::MarkedString::LanguageString(value) => {
                recorder.scope("language_string", |recorder| {
                    value.record_memory_children(recorder)
                });
            }
        }
    }
}

impl MemorySize for ls_types::HoverContents {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            ls_types::HoverContents::Scalar(value) => {
                recorder.scope("scalar", |recorder| value.record_memory_children(recorder));
            }
            ls_types::HoverContents::Array(values) => {
                recorder.scope("array", |recorder| values.record_memory_children(recorder));
            }
            ls_types::HoverContents::Markup(markup) => {
                recorder.scope("markup", |recorder| markup.record_memory_children(recorder));
            }
        }
    }
}

impl MemorySize for ls_types::InlayHintLabel {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            ls_types::InlayHintLabel::String(value) => {
                recorder.scope("string", |recorder| value.record_memory_children(recorder));
            }
            ls_types::InlayHintLabel::LabelParts(parts) => {
                recorder.scope("parts", |recorder| parts.record_memory_children(recorder));
            }
        }
    }
}

impl MemorySize for ls_types::InlayHintTooltip {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            ls_types::InlayHintTooltip::String(value) => {
                recorder.scope("string", |recorder| value.record_memory_children(recorder));
            }
            ls_types::InlayHintTooltip::MarkupContent(markup) => {
                recorder.scope("markup", |recorder| markup.record_memory_children(recorder));
            }
        }
    }
}

impl MemorySize for ls_types::InlayHintLabelPartTooltip {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            ls_types::InlayHintLabelPartTooltip::String(value) => {
                recorder.scope("string", |recorder| value.record_memory_children(recorder));
            }
            ls_types::InlayHintLabelPartTooltip::MarkupContent(markup) => {
                recorder.scope("markup", |recorder| markup.record_memory_children(recorder));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::memsize::{MemoryRecorder, MemorySize};

    #[test]
    fn records_diagnostic_owned_payloads() {
        let diagnostic = ls_types::Diagnostic {
            range: ls_types::Range::new(
                ls_types::Position::new(1, 2),
                ls_types::Position::new(1, 5),
            ),
            severity: Some(ls_types::DiagnosticSeverity::WARNING),
            source: Some("cargo check".to_owned()),
            message: "unused variable".to_owned(),
            ..ls_types::Diagnostic::default()
        };

        let mut recorder = MemoryRecorder::new("diagnostic");
        diagnostic.record_memory_size(&mut recorder);
        let totals = recorder.totals_by_path();

        assert!(totals.contains_key("diagnostic"));
        assert!(totals.contains_key("diagnostic.source.some"));
        assert!(totals.contains_key("diagnostic.message"));
    }

    #[test]
    fn records_completion_docs_and_label_details() {
        let completion = ls_types::CompletionItem {
            label: "new".to_owned(),
            label_details: Some(ls_types::CompletionItemLabelDetails {
                detail: Some("() -> User".to_owned()),
                description: Some("app::User".to_owned()),
            }),
            documentation: Some(ls_types::Documentation::MarkupContent(
                ls_types::MarkupContent {
                    kind: ls_types::MarkupKind::Markdown,
                    value: "Create a user.".to_owned(),
                },
            )),
            ..ls_types::CompletionItem::default()
        };

        let mut recorder = MemoryRecorder::new("completion");
        completion.record_memory_size(&mut recorder);
        let totals = recorder.totals_by_path();

        assert!(totals.contains_key("completion.label"));
        assert!(totals.contains_key("completion.label_details.some.detail.some"));
        assert!(totals.contains_key("completion.documentation.some.markup.value"));
    }
}
