//! Target-level body selection.
//!
//! This module starts from Semantic IR target items and turns the selected functions, consts, and
//! statics into lowering tasks. Nested body-local tasks are discovered later, after each parent
//! body's local item store exists.

use anyhow::Context as _;

use rg_def_map::PackageSlot;
use rg_ir_model::{ConstRef, FunctionRef, ImplRef, ItemOwner, ModuleRef, StaticRef, TraitRef};
use rg_ir_storage::ItemStoreQuery;
use rg_parse::{FileId, Span};
use rg_semantic_ir::SemanticIrReadTxn;
use rg_text::NameInterner;

use crate::{BodyOwner, TargetBodies};

use super::{BodyIrLoweringScope, task::BodyLoweringTask, task::BodyTaskLowering};

type FunctionLoweringTarget = (FunctionRef, FileId, Span);
type ConstLoweringTarget = (ConstRef, FileId, Span);
type StaticLoweringTarget = (StaticRef, FileId, Span);

pub(super) struct TargetLowering<'a> {
    pub(super) parse_package: &'a rg_parse::Package,
    pub(super) semantic_ir: &'a SemanticIrReadTxn<'a>,
    pub(super) scope: BodyIrLoweringScope<'a>,
    pub(super) package: PackageSlot,
    pub(super) functions: Vec<FunctionLoweringTarget>,
    pub(super) consts: Vec<ConstLoweringTarget>,
    pub(super) statics: Vec<StaticLoweringTarget>,
    pub(super) target_bodies: TargetBodies,
    pub(super) interner: &'a mut NameInterner,
}

impl<'a> TargetLowering<'a> {
    pub(super) fn lower(mut self) -> anyhow::Result<TargetBodies> {
        let tasks = self.selected_body_tasks()?;
        BodyTaskLowering::new(self.parse_package, &mut self.target_bodies, self.interner)
            .lower_tasks(&tasks)?;
        Ok(self.target_bodies)
    }

    /// Converts target semantic items into the same task shape used by nested body discovery.
    ///
    /// Body IDs are assigned in lowering order, not from Semantic IR item IDs. Resolve a body by
    /// inspecting `ResolvedBodyData::owner`; never cast item IDs to `BodyId`.
    fn selected_body_tasks(&self) -> anyhow::Result<Vec<BodyLoweringTask>> {
        let mut tasks = Vec::new();

        for &(function_ref, file_id, span) in &self.functions {
            if !self.scope.should_lower_body_file(self.package, file_id) {
                continue;
            }
            let Some(owner_module) = Self::owner_module(self.semantic_ir, function_ref)? else {
                continue;
            };
            tasks.push(BodyLoweringTask {
                owner: BodyOwner::Function(function_ref),
                owner_module,
                fallback_module: owner_module,
                file_id,
                span,
            });
        }

        for &(const_ref, file_id, span) in &self.consts {
            if !self.scope.should_lower_body_file(self.package, file_id) {
                continue;
            }
            let Some(owner_module) = Self::const_owner_module(self.semantic_ir, const_ref)? else {
                continue;
            };
            tasks.push(BodyLoweringTask {
                owner: BodyOwner::Const(const_ref),
                owner_module,
                fallback_module: owner_module,
                file_id,
                span,
            });
        }

        for &(static_ref, file_id, span) in &self.statics {
            if !self.scope.should_lower_body_file(self.package, file_id) {
                continue;
            }
            let Some(owner_module) = Self::static_owner_module(self.semantic_ir, static_ref)?
            else {
                continue;
            };
            tasks.push(BodyLoweringTask {
                owner: BodyOwner::Static(static_ref),
                owner_module,
                fallback_module: owner_module,
                file_id,
                span,
            });
        }

        Ok(tasks)
    }

    fn owner_module(
        semantic_ir: &SemanticIrReadTxn<'_>,
        function: FunctionRef,
    ) -> anyhow::Result<Option<ModuleRef>> {
        let item_query = ItemStoreQuery::new(semantic_ir);
        let Some(function_data) = item_query.function_data(function).with_context(|| {
            format!(
                "while attempting to fetch semantic IR function {:?}",
                function.id
            )
        })?
        else {
            return Ok(None);
        };

        Self::owner_module_for_item_owner(semantic_ir, function.origin, function_data.owner)
    }

    fn const_owner_module(
        semantic_ir: &SemanticIrReadTxn<'_>,
        konst: ConstRef,
    ) -> anyhow::Result<Option<ModuleRef>> {
        let item_query = ItemStoreQuery::new(semantic_ir);
        let Some(const_data) = item_query.const_data(konst).with_context(|| {
            format!("while attempting to fetch semantic IR const {:?}", konst.id)
        })?
        else {
            return Ok(None);
        };

        Self::owner_module_for_item_owner(semantic_ir, konst.origin, const_data.owner)
    }

    fn static_owner_module(
        semantic_ir: &SemanticIrReadTxn<'_>,
        static_ref: StaticRef,
    ) -> anyhow::Result<Option<ModuleRef>> {
        let item_query = ItemStoreQuery::new(semantic_ir);
        Ok(item_query.static_data(static_ref)?.map(|data| data.owner))
    }

    fn owner_module_for_item_owner(
        semantic_ir: &SemanticIrReadTxn<'_>,
        origin: rg_ir_model::DefMapRef,
        owner: ItemOwner,
    ) -> anyhow::Result<Option<ModuleRef>> {
        let item_query = ItemStoreQuery::new(semantic_ir);
        let module = match owner {
            ItemOwner::Module(module_ref) => Some(module_ref),
            ItemOwner::Trait(trait_id) => item_query
                .trait_data(TraitRef {
                    origin,
                    id: trait_id,
                })
                .with_context(|| {
                    format!(
                        "while attempting to fetch semantic IR trait owner {:?}",
                        trait_id
                    )
                })?
                .map(|data| data.owner),
            ItemOwner::Impl(impl_id) => item_query
                .impl_data(ImplRef {
                    origin,
                    id: impl_id,
                })
                .with_context(|| {
                    format!(
                        "while attempting to fetch semantic IR impl owner {:?}",
                        impl_id
                    )
                })?
                .map(|data| data.owner),
        };

        Ok(module)
    }
}
