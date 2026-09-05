use serde::{Deserialize, Serialize};

/// Completion client features that the analysis engine needs while rendering items.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
pub struct CompletionClientCapabilities {
    pub snippet_support: bool,
}

impl CompletionClientCapabilities {
    pub fn from_lsp_client_capabilities(capabilities: &ls_types::ClientCapabilities) -> Self {
        let snippet_support = capabilities
            .text_document
            .as_ref()
            .and_then(|text_document| text_document.completion.as_ref())
            .and_then(|completion| completion.completion_item.as_ref())
            .and_then(|completion_item| completion_item.snippet_support)
            .unwrap_or(false);

        Self { snippet_support }
    }
}
