//! Reopen an item body from the packed tokens saved during declaration lowering.
//!
//! An expansion may produce several functions or initializers at one written call site. Each item
//! has its own range in the expansion parse, which lets body lowering reopen just that item.
//! Pre-lowered `cfg_select!` arms use the same entry point for their saved tokens.

use std::sync::Arc;

use rg_ir_model::{BodySource, CrateRef, FileId, PackageSlot};
use rg_item_tree::{ItemTreeId, ItemTreeRef, file_declarations};
use rg_macro_runtime::{ExpansionParseKind, ExpansionSyntax};
use rg_source::QueryContext;
use rg_syntax::{AstNode as _, SyntaxNode, TextRange, ast};

use super::expansion::{SavedItemExpansion, parsed_item_expansion};
use crate::{ExpandedBodyMacro, MacroExpansionRef, body_macros::BodyMacroInputSources};

/// The selected item and the expansion context needed to lower it.
///
/// `syntax` selects one function or initializer. `expansion` maps its tokens back to the input.
/// `source` gives the invocation range to use when a generated node has no exact written range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedBodySyntax {
    pub syntax: SyntaxNode,
    pub expansion: ExpandedBodyMacro<SyntaxNode>,
    pub source: BodySource,
    /// Bodies from the same invocation use this ID to report copies against the same expansion.
    pub invocation: Option<MacroExpansionRef>,
}

impl GeneratedBodySyntax {
    /// A `cfg_select!` arm keeps its declarations in the file's item tree. The ordinary file
    /// parse sees the arm as tokens, so parse the saved arm tokens when preparing one of its bodies.
    pub fn for_file_item(
        db: &QueryContext<'_>,
        krate: CrateRef,
        item: ItemTreeRef,
    ) -> ajika::Result<Option<Self>> {
        let declarations = file_declarations(db, krate.package, item.file_id)?;
        let bodies = declarations.cfg_select_bodies.value_ref(db)?;
        let Some(output) = bodies.get(item.item) else {
            return Ok(None);
        };
        // TODO: Publish copy coverage for pre-lowered arms through the file source output.
        Self::open(
            db,
            krate,
            SavedItemExpansion::File(output.clone()),
            item.item,
            None,
        )
    }

    pub(super) fn open(
        db: &QueryContext<'_>,
        krate: CrateRef,
        output: SavedItemExpansion,
        item: ItemTreeId,
        invocation: Option<MacroExpansionRef>,
    ) -> ajika::Result<Option<Self>> {
        let (data, _) = output.load(db)?;
        let Some((_, range)) = data.body_items.iter().find(|(id, _)| *id == item) else {
            return Ok(None);
        };
        let parsed = body_expansion(db, krate, output)?;
        // An invocation can generate several owners at the same physical range. These offsets
        // select the item in the reproduced expansion, where each owner has a distinct range.
        let range = TextRange::new(range.start.into(), range.end.into());
        let syntax = parsed
            .root
            .covering_element(range)
            .ancestors()
            .find(|node| node.text_range() == range && ast::Item::can_cast(node.kind()))
            .ok_or_else(|| ajika::Error::contract("generated body is absent from its expansion"))?;
        Ok(Some(Self {
            syntax,
            expansion: parsed.expansion.clone(),
            source: parsed.source,
            invocation,
        }))
    }
}

struct ParsedBodyExpansion {
    root: SyntaxNode,
    source: BodySource,
    expansion: ExpandedBodyMacro<SyntaxNode>,
}

/// Reparse the packed tokens once per operation so sibling bodies can borrow the same tree.
/// The saved token IDs resolve through this snapshot's declaration source table. Moving the
/// invocation can shift every written range while leaving the packed tokens unchanged.
#[ajika::query(transient, returns(ref))]
fn body_expansion(
    db: &QueryContext<'_>,
    krate: CrateRef,
    output: SavedItemExpansion,
) -> ajika::Result<ParsedBodyExpansion> {
    let (data, forwarding) = output.load(db)?;
    let (root, spans) = if let Some(inputs) = forwarding.filter(|inputs| !inputs.is_empty()) {
        // Restore before parsing. A lifetime such as `'a` joins two token-tree spans into
        // one syntax token, and the parser needs the written origins when it joins them.
        // Change only the spans so body ranges and copy IDs still use the saved offsets.
        let tokens = data.tokens.map_spans(|span| inputs.original(span));
        let (parse, spans) = ExpansionSyntax::parse_token_tree(&tokens, ExpansionParseKind::Items);
        (parse.syntax_node(), Arc::new(spans))
    } else {
        let parsed = parsed_item_expansion(db, output)?;
        (parsed.root.clone(), Arc::clone(&parsed.spans))
    };
    let package = PackageSlot(data.call_site.anchor.package as usize);
    let file = FileId::from_raw(data.call_site.anchor.file_id.raw_file_id());
    let inputs = file_declarations(db, package, file)?.sources.value(db)?;
    let span = inputs
        .token_span(data.call_site)
        .ok_or_else(|| ajika::Error::contract("generated body invocation range is absent"))?;
    let expansion = ExpandedBodyMacro::new(
        root.clone(),
        spans,
        BodyMacroInputSources::Declaration(inputs),
        data.call_site,
        data.dollar_crate.unwrap_or(krate),
    );
    Ok(ParsedBodyExpansion {
        root,
        source: BodySource::macro_generated(file, span),
        expansion,
    })
}
