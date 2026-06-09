//! Source-like builtin macro payloads discovered during item-tree lowering.
//!
//! These are not expanded declarative macros. They are small builtin cases where preserving
//! ordinary source semantics, such as module-file resolution, is more useful than treating the
//! payload as anonymous generated syntax.

use rg_cfg_eval::CfgPredicate;
use rg_parse::FileId;
use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

use super::super::ItemTreeId;

/// Source-like builtin payload discovered during item-tree lowering.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub enum BuiltinMacroItem {
    /// Literal `include!("...")` resolves to a real source file.
    Include { file: FileId },
    /// `cfg_select!` stores all lowered arms; def-map later picks one for the target cfg.
    CfgSelect { arms: Vec<CfgSelectArmItem> },
}

/// One item-position arm from a lowered `cfg_select!` call.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct CfgSelectArmItem {
    pub predicate: CfgPredicate,
    pub payload: CfgSelectArmPayload,
}

impl CfgSelectArmItem {
    pub fn lowered(predicate: CfgPredicate, items: Vec<ItemTreeId>) -> Self {
        Self {
            predicate,
            payload: CfgSelectArmPayload::Items(items),
        }
    }

    pub fn lowering_failed(predicate: CfgPredicate) -> Self {
        Self {
            predicate,
            payload: CfgSelectArmPayload::LoweringFailed,
        }
    }
}

/// Source-fragment lowering result for one `cfg_select!` arm.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub enum CfgSelectArmPayload {
    /// The arm parsed as ordinary item-position Rust and can be collected if selected.
    Items(Vec<ItemTreeId>),
    /// The arm could not be lowered as source. This matters only if target cfg selects it.
    LoweringFailed,
}
