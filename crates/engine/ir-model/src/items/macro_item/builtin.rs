//! Source-like builtin macro payloads discovered during item-tree lowering.
//!
//! These are not expanded declarative macros. They are small builtin cases where preserving
//! ordinary source semantics, such as module-file resolution, is more useful than treating the
//! payload as anonymous generated syntax.

use rg_cfg_eval::CfgPredicate;
use rg_parse::FileId;
use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

use crate::BuiltinMacroExprKind;

use super::super::ItemTreeId;

/// Compiler-provided macro definition selected through normal macro resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[memsize(leaf)]
#[shrink(leaf)]
pub enum BuiltinMacroKind {
    /// Builtin expression macro that Body IR can represent without declarative expansion.
    Expr(BuiltinMacroExprKind),
    /// Builtin that selects one source stream from cfg predicates.
    CfgSelect,
    /// Builtin that splices another source file into the caller.
    Include,
    /// Builtin that does not contribute item definitions to def-map.
    IgnoredByDefMap,
    /// Builtin that needs dedicated support before it can be expanded.
    Unsupported,
}

impl BuiltinMacroKind {
    /// Classify a known compiler builtin name.
    pub fn from_known_name(name: &str) -> Option<Self> {
        if let Some(kind) = BuiltinMacroExprKind::from_macro_name(name) {
            return Some(Self::Expr(kind));
        }

        match name {
            "asm" | "compile_error" | "global_asm" | "llvm_asm" => Some(Self::IgnoredByDefMap),
            "cfg_select" => Some(Self::CfgSelect),
            "include" => Some(Self::Include),
            "concat_idents" => Some(Self::Unsupported),
            _ => None,
        }
    }

    /// Classify a definition explicitly marked by rustc as compiler-provided.
    pub fn from_rustc_builtin_macro_name(name: &str) -> Self {
        Self::from_known_name(name).unwrap_or(Self::Unsupported)
    }
}

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
