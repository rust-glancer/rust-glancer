//! Target-level coordination before each function body is lowered.

use anyhow::Context as _;
use ra_syntax::{AstNode as _, ast};

use rg_def_map::{ModuleRef, PackageSlot};
use rg_parse::{FileId, Span};
use rg_semantic_ir::{FunctionRef, ImplRef, ItemOwner, SemanticIrReadTxn, TraitRef};
use rg_text::NameInterner;

use crate::ir::TargetBodies;

use super::{BodyIrLoweringScope, function::FunctionBodyLowering, syntax::source_for};

pub(super) struct TargetLowering<'a> {
    pub(super) parse_package: &'a rg_parse::Package,
    pub(super) semantic_ir: &'a SemanticIrReadTxn<'a>,
    pub(super) scope: BodyIrLoweringScope<'a>,
    pub(super) package: PackageSlot,
    pub(super) functions: Vec<(FunctionRef, FileId, Span)>,
    pub(super) target_bodies: TargetBodies,
    pub(super) interner: &'a mut NameInterner,
}

impl<'a> TargetLowering<'a> {
    pub(super) fn lower(mut self) -> anyhow::Result<TargetBodies> {
        for &(function_ref, file_id, span) in &self.functions {
            if !self.scope.should_lower_function(self.package, file_id) {
                continue;
            }

            let Some(owner_module) = self.owner_module(function_ref)? else {
                continue;
            };
            let Some(ast_fn) = self.find_function_ast(file_id, span)? else {
                continue;
            };
            let Some(body_ast) = ast_fn.body() else {
                continue;
            };

            let line_index = self
                .parse_package
                .parsed_file(file_id)
                .expect("function source file should exist while lowering body")
                .line_index()
                .with_context(|| format!("while attempting to load line index for {file_id:?}"))?;
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

        Ok(self.target_bodies)
    }

    fn owner_module(&self, function: FunctionRef) -> anyhow::Result<Option<ModuleRef>> {
        let Some(function_data) = self.semantic_ir.function_data(function).with_context(|| {
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
            ItemOwner::Trait(trait_id) => self
                .semantic_ir
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
            ItemOwner::Impl(impl_id) => self
                .semantic_ir
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

    fn find_function_ast(
        &self,
        file_id: FileId,
        expected: Span,
    ) -> anyhow::Result<Option<ast::Fn>> {
        let parsed_file = self.parse_package.parsed_file(file_id).with_context(|| {
            format!("while attempting to fetch parsed source file {:?}", file_id)
        })?;

        let expected = expected.text;
        let syntax = parsed_file.syntax().with_context(|| {
            format!(
                "while attempting to access retained syntax for {:?}",
                file_id
            )
        })?;
        Ok(syntax
            .syntax()
            .descendants()
            .filter_map(ast::Fn::cast)
            .find(|function| {
                let range = function.syntax().text_range();
                u32::from(range.start()) == expected.start && u32::from(range.end()) == expected.end
            }))
    }
}
