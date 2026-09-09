use std::fmt::Write as _;

use anyhow::Context as _;
use ls_types::{Hover, HoverContents, Location, MarkupContent, MarkupKind};
use rg_analysis::{DocumentationLink, HoverInfo, NavigationTarget};
use rg_parse::LineIndex;

use crate::proto::{markdown, position};

/// Render the hover text and attach editor locations to its item links.
///
/// `line_index` belongs to the hovered document. The `location` callback handles each link's
/// destination document, which may have its own unsaved edits, and can decline a target whose
/// source position is no longer usable.
pub(crate) fn hover(
    info: HoverInfo,
    line_index: &LineIndex,
    mut location: impl FnMut(&NavigationTarget) -> anyhow::Result<Option<Location>>,
) -> anyhow::Result<Option<Hover>> {
    let range = info.range.map(|span| position::range(line_index, span));
    let Some(value) = HoverMarkdown::from_info(info, &mut location)
        .context("render hover blocks")?
        .finish()
    else {
        return Ok(None);
    };
    Ok(Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value,
        }),
        range,
    }))
}

/// Keeps each resolved declaration's path, signature, and docs together as one hover section.
struct HoverMarkdown {
    sections: Vec<String>,
}

impl HoverMarkdown {
    fn from_info(
        info: HoverInfo,
        location: &mut impl FnMut(&NavigationTarget) -> anyhow::Result<Option<Location>>,
    ) -> anyhow::Result<Self> {
        let mut sections = Vec::new();
        for block in info.blocks {
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
                // Link ranges belong to the original docs. Rewrite them before normalizing
                // rustdoc fences, which can remove lines and change every following offset.
                let docs = Self::linked_docs(&docs, &block.doc_links, location)
                    .context("render hover source links")?;
                if let Some(docs) = markdown::render_rustdoc_markdown(&docs) {
                    block_sections.push(docs);
                }
            }
            if !block_sections.is_empty() {
                sections.push(block_sections.join("\n\n"));
            }
        }
        Ok(Self { sections })
    }

    /// Replace the discovered item links while copying the rest of the Markdown unchanged.
    ///
    /// For example, `[profile][id]` becomes `[profile](<file:///...#L12,5>)` when its target has
    /// a usable editor location. Otherwise only `profile` remains, without a broken destination.
    fn linked_docs(
        docs: &str,
        links: &[DocumentationLink],
        location: &mut impl FnMut(&NavigationTarget) -> anyhow::Result<Option<Location>>,
    ) -> anyhow::Result<String> {
        let mut rendered = String::with_capacity(docs.len());
        let mut copied_until = 0;
        // The parser supplies non-overlapping links in document order. Copy each gap exactly
        // as written; only the link itself needs rebuilding from its label and destination.
        for link in links {
            rendered.push_str(&docs[copied_until..link.range.start]);
            let label = &docs[link.label.clone()];
            let destination = match &link.target {
                Some(target) => location(target).context("locate documentation declaration")?,
                None => None,
            };
            if let Some(destination) = destination {
                // Both clients open file links with a one-based line fragment. VS Code also
                // consumes the UTF-16 column; Zed uses the line. Angle brackets keep valid URI
                // characters such as parentheses from becoming Markdown delimiters.
                write!(
                    rendered,
                    "[{label}](<{}#L{},{}>)",
                    destination.uri.as_str(),
                    destination.range.start.line + 1,
                    destination.range.start.character + 1
                )
                .expect("string writes should not fail");
            } else {
                rendered.push_str(label);
            }
            copied_until = link.range.end;
        }
        rendered.push_str(&docs[copied_until..]);
        Ok(rendered)
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
        let markdown = HoverMarkdown::from_info(
            HoverInfo {
                range: None,
                blocks: vec![HoverBlock {
                    kind: SymbolKind::Struct,
                    path: Some("app::User".to_string()),
                    signature: Some("pub struct User".to_string()),
                    ty: None,
                    doc_links: Vec::new(),
                    docs: Some("User account.".to_string()),
                }],
            },
            &mut |_| Ok(None),
        )
        .expect("hover should render")
        .finish();

        assert_eq!(
            markdown.as_deref(),
            Some("```rust\napp::User\n```\n\n```rust\npub struct User\n```\n\nUser account.")
        );
    }

    #[test]
    fn normalizes_rustdoc_docs() {
        let markdown = HoverMarkdown::from_info(
            HoverInfo {
                range: None,
                blocks: vec![HoverBlock {
                    kind: SymbolKind::Function,
                    path: None,
                    signature: Some("pub fn make_user()".to_string()),
                    ty: None,
                    doc_links: Vec::new(),
                    docs: Some("```rust,no_run\n# use app::User;\nUser::new();\n```".to_string()),
                }],
            },
            &mut |_| Ok(None),
        )
        .expect("hover should render")
        .finish();

        assert_eq!(
            markdown.as_deref(),
            Some("```rust\npub fn make_user()\n```\n\n```rust\nUser::new();\n```")
        );
    }
}
