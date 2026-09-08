//! Markdown adaptation for item links in editor documentation.
//!
//! The parser identifies actual links, including rustdoc's shortcuts without reference
//! definitions. Resolution and source navigation remain in their existing semantic layers.
//! This module joins those steps: it pairs link ranges with source targets while leaving the
//! original Markdown alone. The LSP renderer can then replace the links without rebuilding
//! paragraphs, code examples, or other formatting from parser events.

use std::ops::Range;

use anyhow::Context as _;
use pulldown_cmark::{BrokenLink, CowStr, Event, LinkType, Options, Parser, Tag, TagEnd};
use rg_ir_model::identity::DeclarationRef;
use rg_ir_view::{
    IndexedViewDb,
    item::documentation::{DocumentationLinkResolution, DocumentationView},
};

use crate::{DocumentationLink, query::navigation::NavigationTargetProjection};

/// Prepares source links for the documentation attached to a declaration.
///
/// For `[profile](super::Profile)`, look up `super::Profile` where the comment was written,
/// even if the hover is shown on an imported name in another module. Then turn that declaration
/// into the same navigation target a goto request would use. File URLs come later, when the
/// renderer can check which source positions are usable in the editor.
pub(crate) struct DocumentationLinkResolver<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> DocumentationLinkResolver<'a, 'db> {
    pub(crate) fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self { db }
    }

    /// Return the link replacements in document order, keeping each label's original Markdown.
    ///
    /// Ordinary Markdown links need no replacement. An item link with no usable source target
    /// still gets an entry: the renderer needs its ranges to show just the label instead of a
    /// broken link. Finding a declaration alone does not guarantee it has a navigation target.
    pub(crate) fn resolve(
        &self,
        owner: DeclarationRef,
        markdown: &str,
    ) -> anyhow::Result<Vec<DocumentationLink>> {
        let view = DocumentationView::new(self.db);
        let navigation = NavigationTargetProjection::new(self.db);
        let mut links = Vec::new();
        for link in MarkdownLink::extract(markdown) {
            // Module docs can combine outer and inner comments. The link's offset tells the
            // semantic view which comment scope to use within that combined text.
            let target = match view
                .resolve_link(owner, link.range.start, &link.destination)
                .context("resolve documentation link")?
            {
                DocumentationLinkResolution::NotAnItemLink => continue,
                DocumentationLinkResolution::Unresolved => None,
                DocumentationLinkResolution::Declaration(declaration) => navigation
                    .target_for_declaration(declaration)
                    .context("project documentation navigation target")?,
            };
            links.push(DocumentationLink {
                range: link.range,
                label: link.label,
                target,
            });
        }
        Ok(links)
    }
}

/// The authored ranges and parsed destination of one Markdown link.
///
/// ```text
/// [**profile**][account]
///
/// [account]: super::Profile
/// ```
///
/// Here `range` covers the first line, `label` covers `**profile**`, and `destination` is
/// `super::Profile`. Both ranges are byte offsets in the input, so replacing the destination
/// can preserve the label's Markdown without reconstructing it from rendered text.
struct MarkdownLink<'a> {
    range: Range<usize>,
    label: Range<usize>,
    destination: CowStr<'a>,
}

impl<'a> MarkdownLink<'a> {
    /// Let Markdown distinguish links from bracketed text inside code examples.
    /// Each link is ready at its end event, after its label events have supplied their ranges.
    fn extract(markdown: &'a str) -> impl Iterator<Item = Self> + 'a {
        // Markdown normally leaves references without a definition as plain text. Rustdoc
        // uses that spelling as an item path: both [Profile] and [profile][Profile] can name
        // `Profile`. Feeding the reference back as a destination gives us ordinary link
        // events for those spellings as well as explicit links such as [profile](Profile).
        let callback = |link: BrokenLink<'a>| Some((link.reference, CowStr::Borrowed("")));
        let parser = Parser::new_with_broken_link_callback(
            markdown,
            Options::ENABLE_TABLES | Options::ENABLE_FOOTNOTES | Options::ENABLE_TASKLISTS,
            Some(callback),
        );
        // A link's start and end surround the events for its label. Markdown links cannot
        // contain other links, so only one unfinished link needs to be kept at a time.
        let mut current: Option<Self> = None;
        parser.into_offset_iter().filter_map(move |(event, range)| {
            match event {
                // Automatic URL and email links already have non-Rust destinations.
                Event::Start(Tag::Link {
                    link_type,
                    dest_url,
                    ..
                }) if !matches!(link_type, LinkType::Autolink | LinkType::Email) => {
                    current = Some(Self {
                        label: range.start + 1..range.start + 1,
                        range,
                        destination: dest_url,
                    });
                }
                Event::End(TagEnd::Link) => return current.take(),
                _ => {
                    if let Some(link) = &mut current {
                        // Child event ranges include Markdown delimiters for code and emphasis.
                        // Their union preserves the authored label without reparsing brackets.
                        link.label.end = link.label.end.max(range.end);
                    }
                }
            }
            None
        })
    }
}

#[cfg(test)]
mod tests {
    use super::MarkdownLink;

    #[test]
    fn extracts_item_links_and_preserves_their_labels() {
        let cases = [
            ("[`Profile`]", "`Profile`", "`Profile`"),
            (
                "[**public** profile](super::Profile)",
                "**public** profile",
                "super::Profile",
            ),
            ("[profile][Profile]", "profile", "Profile"),
            ("[`Profile`][]", "`Profile`", "`Profile`"),
            (
                "[profile][id]\n\n[id]: crate::Profile",
                "profile",
                "crate::Profile",
            ),
        ];
        for (markdown, label, destination) in cases {
            let links = MarkdownLink::extract(markdown).collect::<Vec<_>>();
            assert_eq!(links.len(), 1, "{markdown}");
            assert_eq!(&markdown[links[0].label.clone()], label, "{markdown}");
            assert_eq!(links[0].destination.as_ref(), destination, "{markdown}");
        }
    }
}
