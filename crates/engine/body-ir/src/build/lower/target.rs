//! Target-level coordination before each expression body is lowered.

use std::collections::HashMap;

use anyhow::Context as _;
use rg_syntax::{AstNode as _, ast};

use rg_def_map::PackageSlot;
use rg_ir_model::{ConstRef, FunctionRef, ImplRef, ItemOwner, ModuleRef, StaticRef, TraitRef};
use rg_ir_storage::ItemStoreQuery;
use rg_parse::{FileId, Span};
use rg_semantic_ir::SemanticIrReadTxn;
use rg_text::NameInterner;

use crate::ir::{BodyOwner, TargetBodies};

use super::{BodyIrLoweringScope, body::BodyLowering, syntax::source_for};

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
        self.lower_selected_bodies_by_file()?;
        Ok(self.target_bodies)
    }

    /// Lowers the target in file-sized batches so syntax is only live for one source file at a time.
    ///
    /// Body IDs are assigned in lowering order, not from Semantic IR item IDs.
    /// Resolve a body by inspecting `BodyData::owner`; never cast item IDs to `BodyId`.
    fn lower_selected_bodies_by_file(&mut self) -> anyhow::Result<()> {
        let mut functions = self
            .functions
            .iter()
            .copied()
            .filter(|(_, file_id, _)| self.scope.should_lower_body_file(self.package, *file_id))
            .collect::<Vec<_>>();
        let mut consts = self
            .consts
            .iter()
            .copied()
            .filter(|(_, file_id, _)| self.scope.should_lower_body_file(self.package, *file_id))
            .collect::<Vec<_>>();
        let mut statics = self
            .statics
            .iter()
            .copied()
            .filter(|(_, file_id, _)| self.scope.should_lower_body_file(self.package, *file_id))
            .collect::<Vec<_>>();

        // Make equal `FileId`s contiguous. Each group below will parse one file, build a function
        // and initializer span lookup for that file, lower every selected body from it, and then
        // drop syntax.
        functions.sort_by_key(|(_, file_id, _)| file_id.0);
        consts.sort_by_key(|(_, file_id, _)| file_id.0);
        statics.sort_by_key(|(_, file_id, _)| file_id.0);

        let mut file_ids = functions
            .iter()
            .map(|(_, file_id, _)| *file_id)
            .chain(consts.iter().map(|(_, file_id, _)| *file_id))
            .chain(statics.iter().map(|(_, file_id, _)| *file_id))
            .collect::<Vec<_>>();
        file_ids.sort_by_key(|file_id| file_id.0);
        file_ids.dedup();

        for file_id in file_ids {
            let function_range = target_range_for_file(&functions, file_id);
            let const_range = target_range_for_file(&consts, file_id);
            let static_range = target_range_for_file(&statics, file_id);
            self.lower_file_bodies(
                file_id,
                &functions[function_range],
                &consts[const_range],
                &statics[static_range],
            )?;
        }

        Ok(())
    }

    fn lower_file_bodies(
        &mut self,
        file_id: FileId,
        functions: &[FunctionLoweringTarget],
        consts: &[ConstLoweringTarget],
        statics: &[StaticLoweringTarget],
    ) -> anyhow::Result<()> {
        let parsed_file = self.parse_package.parsed_file(file_id).with_context(|| {
            format!("while attempting to fetch parsed source file {:?}", file_id)
        })?;
        let line_index = parsed_file
            .line_index()
            .with_context(|| format!("while attempting to load line index for {file_id:?}"))?;
        let syntax = parsed_file.parse_syntax().with_context(|| {
            format!("while attempting to parse syntax for body lowering in {file_id:?}")
        })?;
        let syntax = syntax.tree();

        // Semantic IR stores item spans from item-tree lowering. File-local lookups let us keep
        // syntax alive only for this file while still finding nested body declarations later.
        let mut functions_by_span = HashMap::new();
        for function in syntax.syntax().descendants().filter_map(ast::Fn::cast) {
            let range = function.syntax().text_range();
            functions_by_span.insert((u32::from(range.start()), u32::from(range.end())), function);
        }
        let mut consts_by_span = HashMap::new();
        for konst in syntax.syntax().descendants().filter_map(ast::Const::cast) {
            let range = konst.syntax().text_range();
            consts_by_span.insert((u32::from(range.start()), u32::from(range.end())), konst);
        }
        let mut statics_by_span = HashMap::new();
        for static_item in syntax.syntax().descendants().filter_map(ast::Static::cast) {
            let range = static_item.syntax().text_range();
            statics_by_span.insert(
                (u32::from(range.start()), u32::from(range.end())),
                static_item,
            );
        }

        for &(function_ref, _, span) in functions {
            let Some(owner_module) = Self::owner_module(self.semantic_ir, function_ref)? else {
                continue;
            };
            let Some(ast_fn) = functions_by_span.get(&Self::span_key(span)).cloned() else {
                continue;
            };
            let Some(body_ast) = ast_fn.body() else {
                continue;
            };

            let source = source_for(file_id, ast_fn.syntax());
            let body = BodyLowering::new(
                BodyOwner::Function(function_ref),
                owner_module,
                source,
                line_index,
                self.interner,
            )
            .lower_function(ast_fn, body_ast);
            self.target_bodies.alloc_body(body);
        }

        for &(const_ref, _, span) in consts {
            let Some(owner_module) = Self::const_owner_module(self.semantic_ir, const_ref)? else {
                continue;
            };
            let Some(ast_const) = consts_by_span.get(&Self::span_key(span)).cloned() else {
                continue;
            };
            let Some(body_ast) = ast_const.body() else {
                continue;
            };

            let source = source_for(file_id, ast_const.syntax());
            let body = BodyLowering::new(
                BodyOwner::Const(const_ref),
                owner_module,
                source,
                line_index,
                self.interner,
            )
            .lower_initializer(body_ast);
            self.target_bodies.alloc_body(body);
        }

        for &(static_ref, _, span) in statics {
            let Some(owner_module) = Self::static_owner_module(self.semantic_ir, static_ref)?
            else {
                continue;
            };
            let Some(ast_static) = statics_by_span.get(&Self::span_key(span)).cloned() else {
                continue;
            };
            let Some(body_ast) = ast_static.body() else {
                continue;
            };

            let source = source_for(file_id, ast_static.syntax());
            let body = BodyLowering::new(
                BodyOwner::Static(static_ref),
                owner_module,
                source,
                line_index,
                self.interner,
            )
            .lower_initializer(body_ast);
            self.target_bodies.alloc_body(body);
        }

        Ok(())
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

    fn span_key(span: Span) -> (u32, u32) {
        (span.text.start, span.text.end)
    }
}

fn target_range_for_file<T: Copy>(
    targets: &[(T, FileId, Span)],
    file_id: FileId,
) -> std::ops::Range<usize> {
    let start = targets.partition_point(|(_, target_file, _)| target_file.0 < file_id.0);
    let end = targets.partition_point(|(_, target_file, _)| target_file.0 <= file_id.0);
    start..end
}
