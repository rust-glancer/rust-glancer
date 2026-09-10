#![allow(
    deprecated,
    reason = "count payloads in both modern and legacy LSP representations"
)]

use std::mem;

use super::{MemoryRecorder, MemorySize};

super::impl_memory_size_leaf!(
    gen_lsp_types::CompletionItemKind,
    gen_lsp_types::CompletionItemTag,
    gen_lsp_types::DiagnosticSeverity,
    gen_lsp_types::DiagnosticTag,
    gen_lsp_types::InlayHintKind,
    gen_lsp_types::InsertTextFormat,
    gen_lsp_types::InsertTextMode,
    gen_lsp_types::MessageType,
    gen_lsp_types::Position,
    gen_lsp_types::SymbolKind,
    gen_lsp_types::SymbolTag,
);

super::impl_memory_size_children! {
    gen_lsp_types::Range => start, end;
    gen_lsp_types::Location => uri, range;
    gen_lsp_types::LocationLink => origin_selection_range, target_uri, target_range,
        target_selection_range;
    gen_lsp_types::Diagnostic => range, severity, code, code_description, source, message,
        related_information, tags, data;
    gen_lsp_types::CodeDescription => href;
    gen_lsp_types::DiagnosticRelatedInformation => location, message;
    gen_lsp_types::Command => title, command, arguments;
    gen_lsp_types::TextEdit => range, new_text;
    gen_lsp_types::DocumentSymbol => name, detail, kind, tags, #[allow(deprecated)] deprecated, range,
        selection_range, children;
    gen_lsp_types::BaseSymbolInformation => name, kind, tags, container_name;
    gen_lsp_types::SymbolInformation => base_symbol_information, #[allow(deprecated)] deprecated,
        location;
    gen_lsp_types::LocationUriOnly => uri;
    gen_lsp_types::WorkspaceSymbol => base_symbol_information, location, data;
    gen_lsp_types::InsertReplaceEdit => new_text, insert, replace;
    gen_lsp_types::CompletionItemLabelDetails => detail, description;
    gen_lsp_types::CompletionItem => label, label_details, kind, detail, documentation, deprecated,
        preselect, sort_text, filter_text, insert_text, insert_text_format, insert_text_mode,
        text_edit, text_edit_text, additional_text_edits, command, commit_characters, data, tags;
    gen_lsp_types::MarkedStringWithLanguage => language, value;
    gen_lsp_types::MarkupContent => kind, value;
    gen_lsp_types::Hover => contents, range;
    gen_lsp_types::InlayHint => position, label, kind, text_edits, tooltip, padding_left, padding_right,
        data;
    gen_lsp_types::InlayHintLabelPart => value, tooltip, location, command;
}

impl MemorySize for gen_lsp_types::WorkspaceSymbolLocation {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            gen_lsp_types::WorkspaceSymbolLocation::Location(value) => {
                recorder.scope("location", |recorder| {
                    value.record_memory_children(recorder)
                });
            }
            gen_lsp_types::WorkspaceSymbolLocation::LocationUriOnly(value) => {
                recorder.scope("uri_only", |recorder| {
                    value.record_memory_children(recorder)
                });
            }
        }
    }
}

impl MemorySize for gen_lsp_types::Code {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            gen_lsp_types::Code::Int(_) => {}
            gen_lsp_types::Code::String(value) => {
                recorder.scope("string", |recorder| value.record_memory_children(recorder));
            }
        }
    }
}

impl MemorySize for gen_lsp_types::MarkupKind {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        // Standard markup kinds carry no text, but an extension may own a custom kind name.
        if let gen_lsp_types::MarkupKind::Custom(value) = self {
            recorder.scope("custom", |recorder| value.record_memory_children(recorder));
        }
    }
}

impl MemorySize for gen_lsp_types::LspAny {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            gen_lsp_types::LspAny::Null
            | gen_lsp_types::LspAny::Bool(_)
            | gen_lsp_types::LspAny::Number(_) => {}
            gen_lsp_types::LspAny::String(value) => {
                recorder.scope("string", |recorder| value.record_memory_children(recorder));
            }
            gen_lsp_types::LspAny::Array(items) => {
                recorder.scope("array", |recorder| items.record_memory_children(recorder));
            }
            gen_lsp_types::LspAny::Object(object) => {
                recorder.scope("object", |recorder| object.record_memory_children(recorder));
            }
        }
    }
}

impl MemorySize for gen_lsp_types::LspObject {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        // serde_json hides whether the map is backed by BTreeMap or IndexMap. Count initialized
        // entries and mark their storage as approximate rather than pretending to know node layout.
        recorder.record_approximate::<gen_lsp_types::LspObject>(self.len().saturating_mul(
            mem::size_of::<String>().saturating_add(mem::size_of::<gen_lsp_types::LspAny>()),
        ));

        recorder.scope("entries", |recorder| {
            for (key, value) in self {
                recorder.scope("key", |recorder| key.record_memory_children(recorder));
                recorder.scope("value", |recorder| value.record_memory_children(recorder));
            }
        });
    }
}

impl MemorySize for gen_lsp_types::Uri {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        recorder.record_approximate::<gen_lsp_types::Uri>(self.as_str().len());
    }
}

impl MemorySize for gen_lsp_types::WorkspaceSymbolResponse {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            gen_lsp_types::WorkspaceSymbolResponse::SymbolInformationList(symbols) => {
                recorder.scope("flat", |recorder| symbols.record_memory_children(recorder));
            }
            gen_lsp_types::WorkspaceSymbolResponse::WorkspaceSymbolList(symbols) => {
                recorder.scope("nested", |recorder| {
                    symbols.record_memory_children(recorder)
                });
            }
        }
    }
}

impl MemorySize for gen_lsp_types::CompletionItemTextEdit {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            gen_lsp_types::CompletionItemTextEdit::TextEdit(edit) => {
                recorder.scope("edit", |recorder| edit.record_memory_children(recorder));
            }
            gen_lsp_types::CompletionItemTextEdit::InsertReplaceEdit(edit) => {
                recorder.scope("insert_replace", |recorder| {
                    edit.record_memory_children(recorder)
                });
            }
        }
    }
}

impl MemorySize for gen_lsp_types::Documentation {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            gen_lsp_types::Documentation::String(value) => {
                recorder.scope("string", |recorder| value.record_memory_children(recorder));
            }
            gen_lsp_types::Documentation::MarkupContent(markup) => {
                recorder.scope("markup", |recorder| markup.record_memory_children(recorder));
            }
        }
    }
}

impl MemorySize for gen_lsp_types::MarkedString {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            gen_lsp_types::MarkedString::String(value) => {
                recorder.scope("string", |recorder| value.record_memory_children(recorder));
            }
            gen_lsp_types::MarkedString::MarkedStringWithLanguage(value) => {
                recorder.scope("language_string", |recorder| {
                    value.record_memory_children(recorder)
                });
            }
        }
    }
}

impl MemorySize for gen_lsp_types::Contents {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            gen_lsp_types::Contents::MarkedString(value) => {
                recorder.scope("scalar", |recorder| value.record_memory_children(recorder));
            }
            gen_lsp_types::Contents::MarkedStringList(values) => {
                recorder.scope("array", |recorder| values.record_memory_children(recorder));
            }
            gen_lsp_types::Contents::MarkupContent(markup) => {
                recorder.scope("markup", |recorder| markup.record_memory_children(recorder));
            }
        }
    }
}

impl MemorySize for gen_lsp_types::Label {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            gen_lsp_types::Label::String(value) => {
                recorder.scope("string", |recorder| value.record_memory_children(recorder));
            }
            gen_lsp_types::Label::InlayHintLabelPartList(parts) => {
                recorder.scope("parts", |recorder| parts.record_memory_children(recorder));
            }
        }
    }
}

impl MemorySize for gen_lsp_types::Tooltip {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            gen_lsp_types::Tooltip::String(value) => {
                recorder.scope("string", |recorder| value.record_memory_children(recorder));
            }
            gen_lsp_types::Tooltip::MarkupContent(markup) => {
                recorder.scope("markup", |recorder| markup.record_memory_children(recorder));
            }
        }
    }
}

impl MemorySize for gen_lsp_types::Message {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            gen_lsp_types::Message::String(value) => {
                recorder.scope("string", |recorder| value.record_memory_children(recorder));
            }
            gen_lsp_types::Message::MarkupContent(markup) => {
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
        let diagnostic = gen_lsp_types::Diagnostic {
            range: gen_lsp_types::Range::new(
                gen_lsp_types::Position::new(1, 2),
                gen_lsp_types::Position::new(1, 5),
            ),
            severity: Some(gen_lsp_types::DiagnosticSeverity::Warning),
            source: Some("cargo check".to_owned()),
            message: "unused variable".to_owned().into(),
            ..gen_lsp_types::Diagnostic::default()
        };

        let mut recorder = MemoryRecorder::new("diagnostic");
        diagnostic.record_memory_size(&mut recorder);
        let totals = recorder.totals_by_path();

        assert!(totals.contains_key("diagnostic"));
        assert!(totals.contains_key("diagnostic.source.some"));
        assert!(totals.contains_key("diagnostic.message.string"));
    }

    #[test]
    fn records_completion_docs_and_label_details() {
        let completion = gen_lsp_types::CompletionItem {
            label: "new".to_owned(),
            label_details: Some(gen_lsp_types::CompletionItemLabelDetails {
                detail: Some("() -> User".to_owned()),
                description: Some("app::User".to_owned()),
            }),
            documentation: Some(gen_lsp_types::Documentation::MarkupContent(
                gen_lsp_types::MarkupContent {
                    kind: gen_lsp_types::MarkupKind::Markdown,
                    value: "Create a user.".to_owned(),
                },
            )),
            ..gen_lsp_types::CompletionItem::default()
        };

        let mut recorder = MemoryRecorder::new("completion");
        completion.record_memory_size(&mut recorder);
        let totals = recorder.totals_by_path();

        assert!(totals.contains_key("completion.label"));
        assert!(totals.contains_key("completion.label_details.some.detail.some"));
        assert!(totals.contains_key("completion.documentation.some.markup.value"));
    }
}
