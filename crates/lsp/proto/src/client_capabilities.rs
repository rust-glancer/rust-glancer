use serde::{Deserialize, Serialize};

use crate::CompletionClientCapabilities;

/// Client features that engine requests need after LSP initialization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
pub struct ClientCapabilities {
    pub completion: CompletionClientCapabilities,
}

impl ClientCapabilities {
    pub fn from_lsp_client_capabilities(capabilities: &ls_types::ClientCapabilities) -> Self {
        Self {
            completion: CompletionClientCapabilities::from_lsp_client_capabilities(capabilities),
        }
    }
}
