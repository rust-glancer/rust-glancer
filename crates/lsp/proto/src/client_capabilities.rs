use serde::{Deserialize, Serialize};

use crate::{CodeActionClientCapabilities, CompletionClientCapabilities};

/// Client features that engine requests need after LSP initialization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
pub struct ClientCapabilities {
    pub code_action: CodeActionClientCapabilities,
    pub completion: CompletionClientCapabilities,
}

impl ClientCapabilities {
    pub fn from_lsp_client_capabilities(capabilities: &ls_types::ClientCapabilities) -> Self {
        Self {
            code_action: CodeActionClientCapabilities::from_lsp_client_capabilities(capabilities),
            completion: CompletionClientCapabilities::from_lsp_client_capabilities(capabilities),
        }
    }
}
