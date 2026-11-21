use serde::{Deserialize, Serialize};

use crate::{
    CodeActionClientCapabilities, CompletionClientCapabilities, FoldingClientCapabilities,
    SignatureHelpClientCapabilities,
};

/// Client features that engine requests need after LSP initialization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
pub struct ClientCapabilities {
    pub code_action: CodeActionClientCapabilities,
    pub completion: CompletionClientCapabilities,
    pub signature_help: SignatureHelpClientCapabilities,
    /// Workspace edits may use versioned `documentChanges`.
    pub versioned_document_edits: bool,
    pub folding: FoldingClientCapabilities,
}

impl ClientCapabilities {
    /// Eager action edits require both action literals and versioned workspace edits.
    pub const fn supports_eager_actions(self) -> bool {
        self.code_action.literal_support && self.versioned_document_edits
    }

    pub fn from_lsp_client_capabilities(capabilities: &gen_lsp_types::ClientCapabilities) -> Self {
        Self {
            versioned_document_edits: capabilities
                .workspace
                .as_ref()
                .and_then(|workspace| workspace.workspace_edit.as_ref())
                .and_then(|edits| edits.document_changes)
                .unwrap_or(false),
            code_action: CodeActionClientCapabilities::from_lsp_client_capabilities(capabilities),
            completion: CompletionClientCapabilities::from_lsp_client_capabilities(capabilities),
            signature_help: SignatureHelpClientCapabilities::from_lsp_client_capabilities(
                capabilities,
            ),
            folding: FoldingClientCapabilities::from_lsp_client_capabilities(capabilities),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ClientCapabilities;

    #[test]
    fn extracts_engine_client_capabilities_through_the_aggregate() {
        let capabilities: gen_lsp_types::ClientCapabilities =
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
                        "completionItem": {
                            "snippetSupport": true,
                            "labelDetailsSupport": true,
                            "resolveSupport": { "properties": ["additionalTextEdits"] }
                        }
                    },
                    "signatureHelp": {
                        "signatureInformation": {
                            "parameterInformation": { "labelOffsetSupport": true },
                            "noActiveParameterSupport": true
                        }
                    }
                }
            }))
            .expect("client capabilities should deserialize");

        let actual = ClientCapabilities::from_lsp_client_capabilities(&capabilities);

        assert!(actual.code_action.literal_support);
        assert!(actual.versioned_document_edits);
        assert!(actual.code_action.preferred_support);
        assert!(actual.supports_eager_actions());
        assert!(actual.completion.snippet_support);
        assert!(actual.completion.label_details_support);
        assert!(actual.completion.resolve_additional_text_edits);
        assert!(actual.signature_help.label_offset_support);
        assert!(actual.signature_help.no_active_parameter_support);

        let defaults = ClientCapabilities::from_lsp_client_capabilities(
            &gen_lsp_types::ClientCapabilities::default(),
        );
        assert!(!defaults.supports_eager_actions());
        assert!(!defaults.completion.snippet_support);
        assert!(!defaults.completion.label_details_support);
        assert!(!defaults.completion.resolve_additional_text_edits);
    }
}
