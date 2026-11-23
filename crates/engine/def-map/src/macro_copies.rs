//! Copy coverage belongs to the expansion that produced the tokens.

use std::sync::Arc;

use rg_ir_model::{BodyKey, CrateRef, FileId, Span};
use rg_std::{MemorySize, Shrink};
use rg_syntax::{SyntaxNode, TextRange};
use rg_tt::{Span as TtSpan, TokenOrigins, TokenOwner, syntax_bridge::ExpansionSpanMap};
use wincode::{SchemaRead, SchemaWrite};

use crate::{GeneratedSourceId, GeneratedSourceStoreId, ItemSource, ItemSourceKind};

/// A complete origin in the invocation's input table. The call itself can be used as a
/// fallback by the expander, so sharing its anchor alone is insufficient.
pub(crate) fn is_argument_origin(call: TtSpan, token: TtSpan) -> bool {
    token.anchor == call.anchor && token.origins.last().is_some() && token.origins != call.origins
}

/// Forwarding can account for one parent copy only when the output has one input ID.
pub(crate) fn forwarded_input_id(token: TtSpan) -> Option<u32> {
    let first = token.origins.first();
    (token.origins.last() == Some(first)).then_some(first)
}

pub(crate) fn expansion_tokens<'a>(
    syntax: &SyntaxNode,
    map: &'a ExpansionSpanMap,
) -> impl Iterator<Item = (TextRange, TtSpan)> + use<'a> {
    syntax
        .descendants_with_tokens()
        .filter_map(|element| element.into_token())
        .filter(|token| !token.kind().is_trivia())
        .filter_map(move |token| {
            let range = token.text_range();
            Some((range, map.span_for_range(range)?))
        })
}

/// The producer of one expansion within a configured source route.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub enum MacroExpansionRef {
    Item {
        krate: CrateRef,
        store: GeneratedSourceStoreId,
        source: GeneratedSourceId,
    },
    Body {
        owner: BodyKey,
        expansion: u32,
    },
}

impl MacroExpansionRef {
    pub fn krate(self) -> CrateRef {
        match self {
            Self::Item { krate, .. } => krate,
            Self::Body { owner, .. } => owner.krate,
        }
    }
}

/// The copy ID is its token's start in the expansion parse. Two copies of the same input
/// use distinct output offsets even when both navigate to the same byte range in the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct MacroCopyRef {
    pub expansion: MacroExpansionRef,
    pub token: u32,
}

/// Physical inputs and forwarding edges published by one expansion producer. Output offsets
/// identify copies only inside this record. Transformed inputs have no editable output token.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct MacroCopyCoverage {
    pub file: FileId,
    pub copies: Vec<(u32, Span)>,
    pub transformed: Vec<Span>,
    pub forwarded: Vec<MacroCopyRef>,
}

/// A nested item macro receives distinct parent-copy IDs even when several inputs
/// share one written origin. Its call retains the information to restore those origins.
#[derive(Debug, Clone)]
pub(crate) struct ForwardedMacroInputs {
    parent: MacroExpansionRef,
    inputs: Arc<[(u32, TtSpan)]>,
}

impl ForwardedMacroInputs {
    pub(crate) fn new(
        source: ItemSource,
        krate: CrateRef,
        inputs: &[(u32, TtSpan)],
    ) -> Option<Self> {
        let ItemSourceKind::Generated(source) = source.kind else {
            return None;
        };
        Some(Self {
            parent: source.expansion(krate),
            inputs: Arc::from(inputs),
        })
    }

    pub(crate) fn copy(&self, span: TtSpan) -> Option<MacroCopyRef> {
        if span.anchor.owner != TokenOwner::Forwarded {
            return None;
        }
        Some(MacroCopyRef {
            expansion: self.parent,
            token: forwarded_input_id(span)?,
        })
    }

    pub(crate) fn original(&self, span: TtSpan) -> TtSpan {
        if span.anchor.owner != TokenOwner::Forwarded {
            return span;
        }
        let original = |id| {
            self.inputs
                .binary_search_by_key(&id, |(id, _)| *id)
                .ok()
                .map(|index| self.inputs[index].1)
        };
        let first =
            original(span.origins.first()).expect("forwarded token belongs to this invocation");
        let origins = span
            .origins
            .last()
            .and_then(original)
            .filter(|last| last.anchor == first.anchor)
            .map_or_else(
                || TokenOrigins::synthetic(first.origins.first()),
                |last| first.origins.join(last.origins),
            );
        TtSpan {
            origins,
            anchor: first.anchor,
            ctx: span.ctx,
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.inputs.is_empty()
    }
}
