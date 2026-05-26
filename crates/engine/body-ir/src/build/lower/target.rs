//! Target-level coordination before each function body is lowered.

use std::collections::HashMap;

use anyhow::Context as _;
use rg_syntax::{AstNode as _, ast};

use rg_def_map::PackageSlot;
use rg_ir_model::{FunctionRef, ImplRef, ItemOwner, ModuleRef, TraitRef};
use rg_parse::{FileId, Span};
use rg_semantic_ir::SemanticIrReadTxn;
use rg_text::NameInterner;

use crate::ir::TargetBodies;

use super::{BodyIrLoweringScope, function::FunctionBodyLowering, syntax::source_for};

type FunctionLoweringTarget = (FunctionRef, FileId, Span);

pub(super) struct TargetLowering<'a> {
    pub(super) parse_package: &'a rg_parse::Package,
    pub(super) semantic_ir: &'a SemanticIrReadTxn<'a>,
    pub(super) scope: BodyIrLoweringScope<'a>,
    pub(super) package: PackageSlot,
    pub(super) functions: Vec<FunctionLoweringTarget>,
    pub(super) target_bodies: TargetBodies,
    pub(super) interner: &'a mut NameInterner,
}

impl<'a> TargetLowering<'a> {
    pub(super) fn lower(mut self) -> anyhow::Result<TargetBodies> {
        self.lower_selected_functions_by_file()?;
        Ok(self.target_bodies)
    }

    /// Lowers the target in file-sized batches so syntax is only live for one source file at a time.
    ///
    /// Semantic IR already gives us stable function slots, so this temporary work list can be
    /// reordered freely: lowered bodies are written back through `FunctionRef`, not through the
    /// iteration order.
    fn lower_selected_functions_by_file(&mut self) -> anyhow::Result<()> {
        let mut functions = self
            .functions
            .iter()
            .copied()
            .filter(|(_, file_id, _)| self.scope.should_lower_function(self.package, *file_id))
            .collect::<Vec<_>>();

        // Make equal `FileId`s contiguous. Each group below will parse one file, build a function
        // span lookup for that file, lower every selected function from it, and then drop syntax.
        functions.sort_by_key(|(_, file_id, _)| file_id.0);

        let mut start = 0;
        while start < functions.len() {
            let file_id = functions[start].1;

            // Walk the contiguous run by index so `lower_file_functions` can borrow a slice
            // without allocating another per-file Vec.
            let mut end = start + 1;
            while end < functions.len() && functions[end].1 == file_id {
                end += 1;
            }

            self.lower_file_functions(file_id, &functions[start..end])?;
            start = end;
        }

        Ok(())
    }

    fn lower_file_functions(
        &mut self,
        file_id: FileId,
        functions: &[FunctionLoweringTarget],
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

        // Semantic IR stores function spans from item-tree lowering. A file-local lookup lets us
        // keep syntax alive only for this file while still finding nested body declarations later.
        let mut functions_by_span = HashMap::new();
        for function in syntax.syntax().descendants().filter_map(ast::Fn::cast) {
            let range = function.syntax().text_range();
            functions_by_span.insert((u32::from(range.start()), u32::from(range.end())), function);
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
            let body = FunctionBodyLowering::new(
                function_ref,
                owner_module,
                source,
                line_index,
                self.interner,
            )
            .lower(ast_fn, body_ast);
            let body_id = self.target_bodies.alloc_body(body);
            self.target_bodies
                .set_function_body(function_ref.id, body_id);
        }

        Ok(())
    }

    fn owner_module(
        semantic_ir: &SemanticIrReadTxn<'_>,
        function: FunctionRef,
    ) -> anyhow::Result<Option<ModuleRef>> {
        let Some(function_data) = semantic_ir.function_data(function).with_context(|| {
            format!(
                "while attempting to fetch semantic IR function {:?}",
                function.id
            )
        })?
        else {
            return Ok(None);
        };

        let module = match function_data.owner {
            ItemOwner::Module(module_ref) => Some(module_ref),
            ItemOwner::Trait(trait_id) => semantic_ir
                .trait_data(TraitRef {
                    target: function.target,
                    id: trait_id,
                })
                .with_context(|| {
                    format!(
                        "while attempting to fetch semantic IR trait owner {:?}",
                        trait_id
                    )
                })?
                .map(|data| data.owner),
            ItemOwner::Impl(impl_id) => semantic_ir
                .impl_data(ImplRef {
                    target: function.target,
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
