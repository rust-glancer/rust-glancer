use serde::{Deserialize, Serialize};

/// Folding behavior that the engine must adapt to the requesting editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
pub struct FoldingClientCapabilities {
    /// The client ignores character offsets and collapses complete lines.
    pub line_folding_only: bool,
}

impl FoldingClientCapabilities {
    pub fn from_lsp_client_capabilities(capabilities: &ls_types::ClientCapabilities) -> Self {
        let line_folding_only = capabilities
            .text_document
            .as_ref()
            .and_then(|text_document| text_document.folding_range.as_ref())
            .and_then(|folding| folding.line_folding_only)
            .unwrap_or(false);

        Self { line_folding_only }
    }
}

#[cfg(test)]
mod tests {
    use ls_types::{
        ClientCapabilities, FoldingRangeClientCapabilities as LspFoldingRangeClientCapabilities,
        TextDocumentClientCapabilities,
    };

    use super::FoldingClientCapabilities;

    #[test]
    fn reads_line_only_support_from_client_capabilities() {
        let capabilities = ClientCapabilities {
            text_document: Some(TextDocumentClientCapabilities {
                folding_range: Some(LspFoldingRangeClientCapabilities {
                    line_folding_only: Some(true),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        };

        let actual = FoldingClientCapabilities::from_lsp_client_capabilities(&capabilities);

        assert!(actual.line_folding_only);
    }
}
