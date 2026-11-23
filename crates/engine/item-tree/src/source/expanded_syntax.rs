//! Token input and item ranges for bodies in parsed item expansions.

use std::sync::Arc;

use rg_ir_model::{CrateRef, Span};
use rg_std::{MemorySize, Shrink};
use rg_tt::{Span as TtSpan, TopSubtree};
use wincode::{SchemaRead, SchemaWrite};

use crate::ItemTreeId;

/// Saved tokens and body locations for one item expansion.
///
/// If `make!()` emits two functions, both can have the same invocation range in the file.
/// Each function has its own item ID and byte range in the expanded syntax. Save those ranges
/// with the tokens so body lowering can find the right item when it parses the expansion again.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct ExpandedItemSyntax {
    #[shrink(skip)]
    pub tokens: Arc<TopSubtree>,
    #[memsize(skip)]
    #[shrink(skip)]
    pub call_site: TtSpan,
    pub dollar_crate: Option<CrateRef>,
    pub body_items: Vec<(ItemTreeId, Span)>,
}
