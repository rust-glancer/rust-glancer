use rg_std::MemorySize;
use wincode::{SchemaRead, SchemaWrite};
/// User-facing documentation attached to one source declaration.
///
/// The text is already stripped from Rust doc-comment/doc-attribute syntax, but otherwise remains
/// Markdown-like so editor features can render it without re-reading AST.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub struct Documentation {
    pub(crate) text: String,
}

impl Documentation {
    pub fn new(text: impl Into<String>) -> Option<Self> {
        let text = text.into();
        (!text.trim().is_empty()).then_some(Self { text })
    }

    pub fn concat(first: Option<Self>, second: Option<Self>) -> Option<Self> {
        let mut parts = Vec::new();
        if let Some(first) = first {
            parts.push(first.text);
        }
        if let Some(second) = second {
            parts.push(second.text);
        }

        Self::new(parts.join("\n"))
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub fn text(&self) -> String {
        self.text.clone()
    }

    pub fn shrink_to_fit(&mut self) {
        self.text.shrink_to_fit();
    }
}
