//! Token input and item ranges for bodies in parsed item expansions.

use std::sync::Arc;

use rg_ir_model::{CrateRef, Span};
use rg_std::{MemorySize, Shrink};
use rg_syntax::{AstNode as _, ast};
use rg_tt::{Span as TtSpan, TopSubtree};
use wincode::{SchemaRead, SchemaWrite};

use crate::ItemTreeId;

/// Saved tokens and body locations for one item expansion.
///
/// If `make!()` emits two functions, both can have the same invocation range in the file.
/// Each function has its own item ID and byte range in the expanded syntax. Save those ranges
/// with the tokens so body lowering can find the right item when it parses the expansion again.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct ExpandedItemBodies {
    #[shrink(skip)]
    pub tokens: Arc<TopSubtree>,
    #[memsize(skip)]
    #[shrink(skip)]
    pub call_site: TtSpan,
    pub dollar_crate: Option<CrateRef>,
    pub items: Vec<(ItemTreeId, Span)>,
}

impl ExpandedItemBodies {
    /// Keep the full item range. A function needs its parameters along with its block.
    pub fn item_range(item: &ast::Item) -> Option<Span> {
        match item {
            ast::Item::Fn(item) => item.body().map(|_| item.syntax().text_range()),
            ast::Item::Const(item) => item.body().map(|_| item.syntax().text_range()),
            ast::Item::Static(item) => item.body().map(|_| item.syntax().text_range()),
            _ => None,
        }
        .map(Span::from_text_range)
    }
}
