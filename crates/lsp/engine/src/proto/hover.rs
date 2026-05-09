use ls_types::{Hover, HoverContents, MarkupContent, MarkupKind};
use rg_analysis::HoverInfo;
use rg_parse::LineIndex;

use crate::proto::position;

pub(crate) fn hover(info: HoverInfo, line_index: &LineIndex) -> Option<Hover> {
    let range = info.range.map(|span| position::range(line_index, span));
    let value = HoverMarkdown::from_info(info).finish()?;
    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value,
        }),
        range,
    })
}

struct HoverMarkdown {
    sections: Vec<String>,
}

impl HoverMarkdown {
    fn from_info(info: HoverInfo) -> Self {
        let sections = info
            .blocks
            .into_iter()
            .filter_map(|block| {
                let mut block_sections = Vec::new();

                if let Some(path) = block.path {
                    block_sections.push(format!("```rust\n{path}\n```"));
                }

                if let Some(signature) = block.signature {
                    block_sections.push(format!("```rust\n{signature}\n```"));
                }
                if let Some(ty) = block.ty {
                    block_sections.push(format!("```text\nType: {ty}\n```"));
                }

                if let Some(docs) = block.docs {
                    let docs = docs.trim();
                    if !docs.is_empty() {
                        block_sections.push(docs.to_string());
                    }
                }

                (!block_sections.is_empty()).then(|| block_sections.join("\n\n"))
            })
            .collect();

        Self { sections }
    }

    fn finish(self) -> Option<String> {
        (!self.sections.is_empty()).then(|| self.sections.join("\n\n---\n\n"))
    }
}

#[cfg(test)]
mod tests {
    use rg_analysis::{HoverBlock, HoverInfo, SymbolKind};

    use super::HoverMarkdown;

    #[test]
    fn renders_signature_and_docs_as_markdown() {
        let markdown = HoverMarkdown::from_info(HoverInfo {
            range: None,
            blocks: vec![HoverBlock {
                kind: SymbolKind::Struct,
                path: Some("app::User".to_string()),
                signature: Some("pub struct User".to_string()),
                ty: None,
                docs: Some("User account.".to_string()),
            }],
        })
        .finish();

        assert_eq!(
            markdown.as_deref(),
            Some("```rust\napp::User\n```\n\n```rust\npub struct User\n```\n\nUser account.")
        );
    }
}
