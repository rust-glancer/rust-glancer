//! Crate-level body selection.
//!
//! This module starts from Semantic IR crate items and turns the selected functions, consts, and
//! statics into lowering tasks. Nested body-local tasks are discovered later, after each parent
//! body's local item store exists.

use rg_cfg_eval::CfgEvaluator;
use rg_def_map::{DefMap, DefinitionReader};
use rg_ir_model::SavedBodyOwner;
use rg_semantic_ir::{BodyOwnerData, ItemStore};

use super::{
    BodyMacroExpansion, LoweredCrateBodies,
    task::{BodyLoweringTask, BodyTaskLowering, BodyTaskSource},
};

pub(crate) struct CrateLowering<'a> {
    pub(crate) parse_package: &'a rg_parse::Package,
    pub(crate) def_map: &'a DefinitionReader<'a>,
    pub(crate) definitions: &'a DefMap,
    pub(crate) items: &'a ItemStore,
    pub(crate) include_functions: bool,
    pub(crate) crate_bodies: LoweredCrateBodies,
    pub(crate) cfg: CfgEvaluator<'a>,
    pub(crate) cancellation: &'a rg_std::CancellationToken,
}

impl CrateLowering<'_> {
    pub(crate) fn lower(mut self) -> anyhow::Result<LoweredCrateBodies> {
        let mut tasks = Vec::new();
        for root in BodyOwnerData::for_crate(self.items, self.definitions) {
            rg_std::check_cancel!(self.cancellation, "select body task");
            if self.include_functions || !matches!(root.owner, SavedBodyOwner::Function(_)) {
                tasks.push(BodyLoweringTask::for_saved_owner(
                    self.items.crate_ref(),
                    root,
                ));
            }
        }
        let mut macro_expansion = BodyMacroExpansion::new(
            self.parse_package.edition(),
            self.def_map,
            self.cfg,
            self.cancellation.clone(),
        );
        BodyTaskLowering::new(
            BodyTaskSource::Parsed(self.parse_package),
            &mut self.crate_bodies,
            self.cfg,
            self.cancellation,
        )
        .lower_tasks(&tasks, &mut macro_expansion)?;
        Ok(self.crate_bodies)
    }
}
