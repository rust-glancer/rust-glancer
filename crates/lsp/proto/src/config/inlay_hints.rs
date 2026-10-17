use gen_lsp_types::LspAny;
use serde::{Deserialize, Serialize};

use super::section;

/// Query-time presentation settings for inferred type hints.
/// Shortening applies to the inline label; tooltips retain concrete type text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct InlayHintsConfig {
    /// Soft character budget per type label. Whole identifiers and enclosing punctuation can
    /// exceed the budget. Zero disables length-based shortening, keeping iterator simplification.
    pub max_length: usize,
}

impl Default for InlayHintsConfig {
    fn default() -> Self {
        Self { max_length: 25 }
    }
}

impl InlayHintsConfig {
    pub fn from_initialization_options(options: Option<&LspAny>) -> anyhow::Result<Self> {
        let Some(value) = section(options, "inlayHints").and_then(|hints| hints.get("maxLength"))
        else {
            return Ok(Self::default());
        };
        let max_length = value
            .as_u64()
            .and_then(|value| usize::try_from(value).ok())
            .ok_or_else(|| {
                anyhow::anyhow!("rust-glancer inlayHints.maxLength must be a non-negative integer")
            })?;
        Ok(Self { max_length })
    }
}
