//! Lowers declarative macro output into an item-tree-shaped generated source.
//!
//! DefMap owns this payload because expansion depends on the target's imports and `$crate` context.
//! It collects the result as item-tree nodes so semantic lowering can read the declarations in
//! the usual shape.

use std::cell::RefCell;

use anyhow::{Context as _, Result};
use rg_arena::Arena;
use rg_ir_model::{CrateRef, FileId, Span};
use rg_item_tree::{
    DeclarationSourceStore, ExpandedItemSyntax, ItemKind, ItemLowering, ItemMacroSpans, ItemNode,
    ItemTreeId, ItemTreeRef, LoweredItem, SourceRole, dollar_crate_from_span,
};
use rg_macro_runtime::ExpansionSyntax;
use rg_syntax::{
    AstNode as _,
    ast::{self, HasModuleItem as _},
};
use rg_tt::{Span as TtSpan, TokenOrigins, TokenOwner, syntax_bridge::ExpansionSpanMap};

use crate::{
    macro_copies::{ForwardedMacroInputs, expansion_argument_origin, is_argument_origin},
    source::{
        GeneratedExpansionLocations, GeneratedItemExpansion, GeneratedLocation, GeneratedLocations,
        GeneratedSourceData,
    },
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
    raw_spans: &'expansion ExpansionSpanMap,
    forwarding: Option<&'expansion ForwardedMacroInputs>,
}

impl GeneratedSourceLowering<'_> {
    pub(super) fn lower(
        origin_source: ItemTreeRef,
        file_id: FileId,
        call_site: TtSpan,
        dollar_crate: Option<CrateRef>,
        expansion: ExpansionSyntax,
        forwarding: Option<&ForwardedMacroInputs>,
    ) -> Result<GeneratedSourceData> {
        let ExpansionSyntax {
            parse,
            span_map,
            item_tokens,
            transformed_inputs,
        } = expansion;
        let file = ast::MacroItems::cast(parse.syntax_node())
            .context("while attempting to cast macro expansion syntax root")?;
        // Recorded locations need written origins. Keep the raw map too when a nested
        // expansion uses parent copy IDs, so copy detection can use the same spans as replay.
        let restored_spans = forwarding
            .filter(|inputs| !inputs.is_empty())
            .map(|inputs| inputs.restore_spans(&span_map));
        let mut lowering = GeneratedSourceLowering {
            file_id,
            dollar_crate,
            fallback_span: call_site,
            span_map: restored_spans.as_ref().unwrap_or(&span_map),
            locations: DeclarationSourceStore::default(),
            items: Arena::new(),
            body_items: Vec::new(),
            raw_spans: &span_map,
            forwarding,
        };
        let top_level = file.items().map(|item| lowering.lower_item(item)).collect();

        Ok(GeneratedSourceData {
            origin_file_id: file_id,
            locations: GeneratedLocations::Expansion(GeneratedExpansionLocations::new(
                lowering.locations,
            )),
            expansion: Some(GeneratedItemExpansion {
                syntax: ExpandedItemSyntax {
                    tokens: item_tokens.expect("item expansion keeps its expanded tokens"),
                    call_site,
                    dollar_crate,
                    body_items: lowering.body_items,
                },
                transformed_inputs,
                forwarding: forwarding.cloned(),
            }),
            origin_source,
            top_level,
            items: lowering.items,
        })
    }

    fn lower_item(&mut self, item: ast::Item) -> ItemTreeId {
        let span_map = self.span_map;
        let fallback = self.fallback_span;
        let raw_spans = self.raw_spans;
        let forwarding = self.forwarding;
        // Only syntax copied from this invocation gets its own selectable occurrence. Template
        // names, mixed-origin ranges, and whole declaration containers navigate to the call.
        // Store this decision with the origin so later readers cannot mistake the fallback for
        // an editable declaration name.
        let mut map = |range, role| match (role, span_map.span_for_range(range)) {
            (SourceRole::Syntax, Some(origin)) if is_argument_origin(fallback, origin) => {
                GeneratedLocation {
                    origin,
                    editable: true,
                    copy: {
                        // Give a location a copy ID only if it identifies one whole copied
                        // token with the same origin. Rename will look for that token's start
                        // when it opens the full coverage for this expansion.
                        let mut candidates =
                            raw_spans.token_spans(range).filter_map(|(range, raw)| {
                                Some((range, expansion_argument_origin(fallback, raw, forwarding)?))
                            });
                        let copy = candidates.next().filter(|(token, input)| {
                            range.contains_range(*token) && *input == origin
                        });
                        copy.filter(|_| candidates.next().is_none())
                            .map(|(token, _)| token.start().into())
                    },
                }
            }
            _ => GeneratedLocation {
                origin: fallback,
                editable: false,
                copy: None,
            },
        };
        let default_crate = self.dollar_crate;
        let dollar_crate_at = |range| {
            raw_spans
                .span_for_range(range)
                .and_then(dollar_crate_from_span)
                .or(default_crate)
        };
        let mut recorder = self.locations.start_owner(&mut map, Some(&dollar_crate_at));
        let children = ItemLowering::lower_children(&item, |child| Some(self.lower_item(child)));
        let call_input = match &item {
            ast::Item::MacroCall(call) => call.token_tree().map(|tree| tree.syntax().text_range()),
            _ => None,
        };
        let forwarded = RefCell::new(Vec::new());
        let LoweredItem {
            mut node,
            body_range,
        } = ItemLowering {
            file_id: self.file_id,
            dollar_crate: self.dollar_crate,
            macro_spans: Some(ItemMacroSpans {
                token: &|range, _| {
                    let origin = span_map.span_for_range(range).unwrap_or(fallback);
                    // Forward an exact parser token as one parent copy. Some callback ranges
                    // cover only part of a token, such as punctuation split by the bridge.
                    if call_input.is_some_and(|input| input.contains_range(range))
                        && let Some((token, raw)) = raw_spans.token_spans(range).next()
                        && token == range
                        && expansion_argument_origin(fallback, raw, forwarding).is_some()
                    {
                        let token = u32::from(range.start());
                        forwarded.borrow_mut().push((token, origin));
                        let mut tagged = origin;
                        tagged.anchor.owner = TokenOwner::Forwarded;
                        tagged.origins = TokenOrigins::single(token);
                        return tagged;
                    }
                    origin
                },
                call_site: Some(fallback),
            }),
        }
        .lower(&item, children, &mut recorder)
        .expect("generated lowering retains every item kind");
        self.locations.finish_owner(recorder);

        if let ItemKind::MacroCall(call) = &mut node.kind
            && let Some(input) = &mut call.input
        {
            let mut forwarded = forwarded.into_inner();
            forwarded.sort_unstable_by_key(|(token, _)| *token);
            forwarded.dedup_by_key(|(token, _)| *token);
            input.forwarded = forwarded;
        }
        let id = self.items.alloc(node);
        if let Some(range) = body_range {
            self.body_items.push((id, Span::from_text_range(range)));
        }
        id
    }
}
