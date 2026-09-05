use serde::{Deserialize, Serialize};

use crate::{
    CodeActionClientCapabilities, CompletionClientCapabilities, FoldingClientCapabilities,
};

/// Client features that engine requests need after LSP initialization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
pub struct ClientCapabilities {
    pub code_action: CodeActionClientCapabilities,
    pub completion: CompletionClientCapabilities,
    pub folding: FoldingClientCapabilities,
}

impl ClientCapabilities {
    pub fn from_lsp_client_capabilities(capabilities: &ls_types::ClientCapabilities) -> Self {
        Self {
            code_action: CodeActionClientCapabilities::from_lsp_client_capabilities(capabilities),
            completion: CompletionClientCapabilities::from_lsp_client_capabilities(capabilities),
            folding: FoldingClientCapabilities::from_lsp_client_capabilities(capabilities),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ClientCapabilities;

    #[test]
    fn extracts_engine_client_capabilities_through_the_aggregate() {
        let capabilities: ls_types::ClientCapabilities =
            serde_json::from_value(serde_json::json!({
                "workspace": {
                    "workspaceEdit": { "documentChanges": true }
                },
                "textDocument": {
                    "codeAction": {
                        "codeActionLiteralSupport": {
                            "codeActionKind": {
                                "valueSet": ["quickfix", "refactor.rewrite"]
                            }
                        },
                        "isPreferredSupport": true
                    },
                    "completion": {
                        "completionItem": { "snippetSupport": true }
                    }
                }
            }))
            .expect("client capabilities should deserialize");

        let actual = ClientCapabilities::from_lsp_client_capabilities(&capabilities);

        assert!(actual.code_action.literal_support);
        assert!(actual.code_action.versioned_document_edits);
        assert!(actual.code_action.preferred_support);
        assert!(actual.code_action.supports_eager_actions());
        assert!(actual.completion.snippet_support);

        let defaults = ClientCapabilities::from_lsp_client_capabilities(
            &ls_types::ClientCapabilities::default(),
        );
        assert!(!defaults.code_action.supports_eager_actions());
        assert!(!defaults.completion.snippet_support);
    }
}
