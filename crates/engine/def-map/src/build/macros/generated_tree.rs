//! Lowers declarative macro output into an item-tree-shaped generated source.
//!
//! DefMap owns this payload because expansion depends on the target's imports and `$crate` context.
//! It collects the result as item-tree nodes so semantic lowering can read the declarations in
//! the usual shape.

use std::{cell::RefCell, sync::Arc};

use anyhow::{Context as _, Result};
use rg_arena::Arena;
use rg_ir_model::{CrateRef, FileId, Span};
use rg_item_tree::{
    DeclarationSourceStore, ExpandedItemBodies, ItemKind, ItemLowering, ItemMacroSpans, ItemNode,
    ItemTreeId, ItemTreeRef, LoweredItem, SourceRole,
};
use rg_macro_runtime::ExpansionSyntax;
use rg_syntax::{
    AstNode as _,
    ast::{self, HasModuleItem as _},
};
use rg_tt::{Span as TtSpan, TokenOrigins, TokenOwner, syntax_bridge::ExpansionSpanMap};

use crate::{
    macro_copies::{ForwardedMacroInputs, expansion_tokens, is_argument_origin},
    source::{
        ExpansionTokenCopies, GeneratedExpansionLocations, GeneratedLocation, GeneratedLocations,
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
    copied: &'expansion [(rg_syntax::TextRange, TtSpan)],
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
            mut span_map,
            mut item_tokens,
            transformed_inputs,
        } = expansion;
        let file = ast::MacroItems::cast(parse.syntax_node())
            .context("while attempting to cast macro expansion syntax root")?;
        // Unmapped tokens belong to the invocation as a whole. Prepare this once so nested
        // calls and definitions can borrow the token map without rebuilding their fallback.
        let fallback_span = call_site;
        let mut forwarded = Vec::new();
        let copied: Vec<_> = expansion_tokens(file.syntax(), &span_map)
            .filter_map(|(range, token)| {
                let origin = forwarding.map_or(token, |inputs| inputs.original(token));
                if !is_argument_origin(call_site, origin) {
                    return None;
                }
                forwarded.extend(forwarding.and_then(|inputs| inputs.copy(token)));
                Some((range, origin))
            })
            .collect();
        if let Some(inputs) = forwarding.filter(|inputs| !inputs.is_empty()) {
            // Body preparation reparses these tokens after the nested call has dropped its
            // temporary input table, so restore the original spans in both representations.
            span_map.map_spans(|span| inputs.original(span));
            item_tokens =
                item_tokens.map(|tokens| Arc::new(tokens.map_spans(|span| inputs.original(span))));
        }
        let mut lowering = GeneratedSourceLowering {
            file_id,
            dollar_crate,
            fallback_span,
            span_map: &span_map,
            locations: DeclarationSourceStore::default(),
            items: Arena::new(),
            body_items: Vec::new(),
            copied: &copied,
        };
        let top_level = file.items().map(|item| lowering.lower_item(item)).collect();

        Ok(GeneratedSourceData {
            origin_file_id: file_id,
            locations: GeneratedLocations::Expansion(GeneratedExpansionLocations::new(
                lowering.locations,
            )),
            copies: Some(ExpansionTokenCopies {
                call_site,
                tokens: copied
                    .iter()
                    .map(|(range, origin)| (range.start().into(), *origin))
                    .collect(),
                transformed: transformed_inputs
                    .into_iter()
                    .map(|span| forwarding.map_or(span, |inputs| inputs.original(span)))
                    .filter(|&span| is_argument_origin(call_site, span))
                    .collect(),
                forwarded,
                is_root: forwarding.is_none(),
            }),
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
        let copied = self.copied;
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
                        let mut candidates = copied
                            [copied.partition_point(|(token, _)| token.end() <= range.start())..]
                            .iter()
                            .take_while(|(token, _)| token.start() < range.end());
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
        let mut recorder = self.locations.start_owner(&mut map);
        let children = ItemLowering::lower_children(&item, |child| Some(self.lower_item(child)));
        let is_call = matches!(&item, ast::Item::MacroCall(_));
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
                    if is_call
                        && let Ok(index) =
                            copied.binary_search_by_key(&range.start(), |(range, _)| range.start())
                        && copied[index].0 == range
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
