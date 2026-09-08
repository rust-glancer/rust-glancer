use std::ops::Range;

use rg_ir_model::Span;
use rg_ir_view::SymbolKind;

use super::NavigationTarget;

/// Markdown-ready hover payload independent from LSP transport types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoverInfo {
    pub range: Option<Span>,
    pub blocks: Vec<HoverBlock>,
}

/// One independently rendered hover section.
///
/// A single cursor position can resolve to several useful facts, such as a field shorthand that
/// refers both to a local variable and a field declaration. Keeping blocks separate lets clients
/// render those facts with clear separators without losing their individual symbol categories.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoverBlock {
    pub kind: SymbolKind,
    pub path: Option<String>,
    pub signature: Option<String>,
    pub ty: Option<String>,
    pub docs: Option<String>,
    /// Link replacements in source order, with byte ranges into the unmodified `docs`.
    pub doc_links: Vec<DocumentationLink>,
}

/// One item link in the original documentation Markdown.
///
/// Both ranges refer to the unmodified docs string. Keeping the label's source preserves
/// inline code, emphasis, and escapes when the transport inserts the destination URI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentationLink {
    /// The whole link to replace, including its destination or reference, as in `[profile][id]`.
    pub range: Range<usize>,
    /// The label inside that link, including any inline-code or emphasis delimiters.
    pub label: Range<usize>,
    /// An unresolved item link renders as its label without a clickable destination.
    pub target: Option<NavigationTarget>,
}
