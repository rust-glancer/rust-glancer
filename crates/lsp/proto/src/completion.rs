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

#[cfg(test)]
mod tests {
    use ls_types::{
        ClientCapabilities, CompletionClientCapabilities as LspCompletionClientCapabilities,
        CompletionItemCapability, TextDocumentClientCapabilities,
    };

    use super::CompletionClientCapabilities;

    #[test]
    fn reads_completion_snippet_support_from_client_capabilities() {
        let capabilities = ClientCapabilities {
            text_document: Some(TextDocumentClientCapabilities {
                completion: Some(LspCompletionClientCapabilities {
                    completion_item: Some(CompletionItemCapability {
                        snippet_support: Some(true),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        };

        let actual = CompletionClientCapabilities::from_lsp_client_capabilities(&capabilities);

        assert!(actual.snippet_support);
    }
}
