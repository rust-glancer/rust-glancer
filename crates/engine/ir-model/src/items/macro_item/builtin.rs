//! Source-like builtin macro payloads discovered during item-tree lowering.
//!
//! These are not expanded declarative macros. They are small builtin cases where preserving
//! ordinary source semantics, such as module-file resolution, is more useful than treating the
//! payload as anonymous generated syntax.

use rg_cfg_eval::CfgPredicate;
use rg_parse::FileId;
use rg_std::MemorySize;
use wincode::{SchemaRead, SchemaWrite};

use super::super::ItemTreeId;

/// Source-like builtin payload discovered during item-tree lowering.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub enum BuiltinMacroItem {
    /// Literal `include!("...")` resolves to a real source file.
    Include { file: FileId },
    /// `cfg_select!` stores all lowered arms; def-map later picks one for the target cfg.
    CfgSelect { arms: Vec<CfgSelectArmItem> },
}

impl BuiltinMacroItem {
    pub fn shrink_to_fit(&mut self) {
        match self {
            Self::Include { .. } => {}
            Self::CfgSelect { arms } => {
                arms.shrink_to_fit();
                for arm in arms {
                    arm.shrink_to_fit();
                }
            }
        }
    }
}

/// One item-position arm from a lowered `cfg_select!` call.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
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

    pub fn shrink_to_fit(&mut self) {
        self.predicate.shrink_to_fit();
        self.payload.shrink_to_fit();
    }
}

/// Source-fragment lowering result for one `cfg_select!` arm.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub enum CfgSelectArmPayload {
    /// The arm parsed as ordinary item-position Rust and can be collected if selected.
    Items(Vec<ItemTreeId>),
    /// The arm could not be lowered as source. This matters only if target cfg selects it.
    LoweringFailed,
}

impl CfgSelectArmPayload {
    pub fn shrink_to_fit(&mut self) {
        match self {
            Self::Items(items) => items.shrink_to_fit(),
            Self::LoweringFailed => {}
        }
    }
}
