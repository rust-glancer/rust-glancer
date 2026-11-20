use serde::{Deserialize, Serialize};

/// Signature label and active parameter formats supported by the editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
pub struct SignatureHelpClientCapabilities {
    pub label_offset_support: bool,
    pub no_active_parameter_support: bool,
}

impl SignatureHelpClientCapabilities {
    pub(crate) fn from_lsp_client_capabilities(
        capabilities: &gen_lsp_types::ClientCapabilities,
    ) -> Self {
        let information = capabilities
            .text_document
            .as_ref()
            .and_then(|document| document.signature_help.as_ref())
            .and_then(|help| help.signature_information.as_ref());
        Self {
            label_offset_support: information
                .and_then(|information| information.parameter_information)
                .and_then(|parameter| parameter.label_offset_support)
                .unwrap_or_default(),
            no_active_parameter_support: information
                .and_then(|information| information.no_active_parameter_support)
                .unwrap_or_default(),
        }
    }
}
