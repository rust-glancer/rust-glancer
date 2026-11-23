//! Lowers declarative macro output into an item-tree-shaped generated source.
//!
//! DefMap owns this payload because expansion depends on the target's imports and `$crate` context.
//! It collects the result as item-tree nodes so semantic lowering can read the declarations in
//! the usual shape.

use anyhow::{Context as _, Result};
use rg_arena::Arena;
use rg_ir_model::{CrateRef, FileId, Span};
use rg_item_tree::{
    DeclarationSourceStore, ExpandedItemBodies, ItemLowering, ItemMacroSpans, ItemNode, ItemTreeId,
    ItemTreeRef, LoweredItem, SourceRole,
};
use rg_macro_runtime::ExpansionSyntax;
use rg_syntax::{
    AstNode as _,
    ast::{self, HasModuleItem as _},
};
use rg_tt::{Span as TtSpan, syntax_bridge::ExpansionSpanMap};

use crate::source::{
    GeneratedExpansionLocations, GeneratedLocation, GeneratedLocations, GeneratedSourceData,
};

/// Lowers one parsed macro expansion into retained generated item payloads.
///
/// Record how each declaration occurrence maps to the input. Also save the expanded range of
/// each item with a body. The ranges and tokens let body lowering reopen those items after this
/// parse is dropped.
pub(super) struct GeneratedSourceLowering<'expansion> {
    file_id: FileId,
    dollar_crate: Option<CrateRef>,
    fallback_span: TtSpan,
    span_map: &'expansion ExpansionSpanMap,
    locations: DeclarationSourceStore<GeneratedLocation>,
    items: Arena<ItemTreeId, ItemNode>,
    body_items: Vec<(ItemTreeId, Span)>,
}

impl GeneratedSourceLowering<'_> {
    pub(super) fn lower(
        origin_source: ItemTreeRef,
        file_id: FileId,
        call_site: TtSpan,
        dollar_crate: Option<CrateRef>,
        expansion: ExpansionSyntax,
    ) -> Result<GeneratedSourceData> {
        let ExpansionSyntax {
            parse,
            span_map,
            item_tokens,
            ..
        } = expansion;
        let file = ast::MacroItems::cast(parse.syntax_node())
            .context("while attempting to cast macro expansion syntax root")?;
        // Unmapped tokens belong to the invocation as a whole. Prepare this once so nested
        // calls and definitions can borrow the token map without rebuilding their fallback.
        let fallback_span = call_site;
        let mut lowering = GeneratedSourceLowering {
            file_id,
            dollar_crate,
            fallback_span,
            span_map: &span_map,
            locations: DeclarationSourceStore::default(),
            items: Arena::new(),
            body_items: Vec::new(),
        };
        let top_level = file.items().map(|item| lowering.lower_item(item)).collect();

        Ok(GeneratedSourceData {
            origin_file_id: file_id,
            locations: GeneratedLocations::Expansion(GeneratedExpansionLocations::new(
                lowering.locations,
            )),
            bodies: (!lowering.body_items.is_empty()).then(|| ExpandedItemBodies {
                tokens: item_tokens.expect("item expansion keeps its expanded tokens"),
                call_site,
                dollar_crate,
                items: lowering.body_items,
            }),
            origin_source,
            top_level,
            items: lowering.items,
        })
    }

    fn lower_item(&mut self, item: ast::Item) -> ItemTreeId {
        let span_map = self.span_map;
        let fallback = self.fallback_span;
        // Only syntax copied from this invocation gets its own selectable occurrence. Template
        // names, mixed-origin ranges, and whole declaration containers navigate to the call.
        // Store this decision with the origin so later readers cannot mistake the fallback for
        // an editable declaration name.
        let mut map = |range, role| match (role, span_map.span_for_range(range)) {
            (SourceRole::Syntax, Some(origin))
                if origin.anchor == fallback.anchor && origin.origins.last().is_some() =>
            {
                GeneratedLocation {
                    origin,
                    editable: true,
                }
            }
            _ => GeneratedLocation {
                origin: fallback,
                editable: false,
            },
        };
        let mut recorder = self.locations.start_owner(&mut map);
        let children = ItemLowering::lower_children(&item, |child| Some(self.lower_item(child)));
        let LoweredItem { node, body_range } = ItemLowering {
            file_id: self.file_id,
            dollar_crate: self.dollar_crate,
            macro_spans: Some(ItemMacroSpans {
                token: &|range, _| span_map.span_for_range(range).unwrap_or(fallback),
                call_site: Some(fallback),
            }),
        }
        .lower(&item, children, &mut recorder)
        .expect("generated lowering retains every item kind");
        self.locations.finish_owner(recorder);

        let id = self.items.alloc(node);
        if let Some(range) = body_range {
            self.body_items.push((id, Span::from_text_range(range)));
        }
        id
    }
}
