//! Select an item stream for a call already resolved to the `cfg_select!` builtin.
//!
//! Both module and member expansion use the first enabled arm. If file lowering already assigned
//! the arm item IDs, reuse them to keep their source locations. Otherwise parse the selected
//! tokens, as when a macro emits the call or the user calls the builtin through an alias.

use rg_cfg_eval::CfgEvaluator;
use rg_item_tree::{BuiltinMacroItem, CfgSelectArmPayload, ItemTreeId};
use rg_macro_runtime::{CfgSelect, ExpansionParseKind, ExpansionSyntax};
use rg_tt::TopSubtree;

pub(super) enum SelectedCfgArm<'a> {
    Lowered(&'a [ItemTreeId]),
    Generated(ExpansionSyntax),
}

impl<'a> SelectedCfgArm<'a> {
    /// A lowering failure in the first enabled arm ends selection. Generated syntax goes through
    /// the caller's usual parse validation.
    pub(super) fn select(
        builtin: Option<&'a BuiltinMacroItem>,
        args: &TopSubtree,
        cfg: CfgEvaluator<'_>,
    ) -> Option<Self> {
        if let Some(BuiltinMacroItem::CfgSelect { arms }) = builtin {
            let arm = arms
                .iter()
                .find(|arm| cfg.is_predicate_enabled(&arm.predicate))?;
            return match &arm.payload {
                CfgSelectArmPayload::Items(items) => Some(Self::Lowered(items)),
                CfgSelectArmPayload::LoweringFailed => None,
            };
        }
        let selection = CfgSelect::parse(args)?;
        let arm = selection
            .arms()
            .iter()
            .find(|arm| cfg.is_predicate_enabled(&arm.predicate))?;
        Some(Self::Generated(ExpansionSyntax::from_token_tree(
            arm.payload.clone(),
            ExpansionParseKind::Items,
        )))
    }
}
