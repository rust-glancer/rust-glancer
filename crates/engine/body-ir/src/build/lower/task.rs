//! Scheduled body lowering.
//!
//! By the time we decide that a body should be lowered, its owner usually comes from an item
//! store and is identified by source span rather than by a syntax node. This module is the bridge:
//! it groups those scheduled owners by file, opens each parsed file once, finds the matching AST
//! item by span, and then delegates the actual expression lowering to `BodyLowering`.

use anyhow::Context as _;
use rg_cfg_eval::CfgEvaluator;
use rg_ir_model::{ConstRef, FileId, FunctionRef, ModuleRef, OuterBodyOwner, Span, StaticRef};
use rg_parse::SourceReader;
use rg_semantic_ir::BodyOwnerData;
use rg_source::QueryContext;
use rg_std::CancellationToken;
use rg_syntax::{AstNode as _, SyntaxKind, TextRange, ast};
use rg_text::RustEdition;

use super::{
    LoweredBodies, body::BodyLowering, macro_expansion::BodyMacroExpansionContext,
    syntax::source_for,
};
use crate::BodyOwner;

/// A function body or item initializer that should become immutable `BodyData`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BodyLoweringTask {
    pub(crate) owner: BodyOwner,
    /// Module used for body-local lookup inside this body.
    ///
    /// Top-level semantic bodies use their normal module. Nested body-local owners use the
    /// synthetic module for the scope where the item was declared.
    pub(crate) owner_module: ModuleRef,
    /// Ordinary module context to try after body-local lookup fails.
    ///
    /// This is enough for nested bodies to see both parent body-local items and surrounding
    /// semantic items. If we decide to support arbitrary fn-in-fn-in-fn ancestor lookup, this
    /// should become an ordered fallback chain instead of one module.
    pub(crate) fallback_module: ModuleRef,
    pub(crate) file_id: FileId,
    pub(crate) span: Span,
}

impl BodyLoweringTask {
    pub(crate) fn for_owner(root: BodyOwnerData, span: Span) -> Self {
        let store = root.store_ref();
        let owner = match root.owner {
            OuterBodyOwner::Function { id, .. } => BodyOwner::Function(FunctionRef::new(store, id)),
            OuterBodyOwner::Const { id, .. } => BodyOwner::Const(ConstRef::new(store, id)),
            OuterBodyOwner::Static(id) => BodyOwner::Static(StaticRef::new(store.origin, id)),
        };
        Self {
            owner,
            owner_module: root.module,
            fallback_module: root.module,
            file_id: root.file,
            span,
        }
    }
}

pub(crate) struct BodyTaskLowering<'a> {
    db: &'a QueryContext<'a>,
    edition: RustEdition,
    lowered_bodies: &'a mut LoweredBodies,
    cfg: CfgEvaluator<'a>,
    cancellation: &'a CancellationToken,
}

impl<'a> BodyTaskLowering<'a> {
    pub(crate) fn new(
        db: &'a QueryContext<'a>,
        edition: RustEdition,
        lowered_bodies: &'a mut LoweredBodies,
        cfg: CfgEvaluator<'a>,
        cancellation: &'a CancellationToken,
    ) -> Self {
        Self {
            db,
            edition,
            lowered_bodies,
            cfg,
            cancellation,
        }
    }

    /// Lowers scheduled bodies in file-sized batches.
    ///
    /// Parsing a file gives us all syntax nodes in that file, so lowering all matching tasks while
    /// the syntax tree is alive keeps memory use predictable and gives deterministic `BodyId`s.
    pub(crate) fn lower_tasks(
        &mut self,
        tasks: &[BodyLoweringTask],
        macro_expansion: &mut dyn BodyMacroExpansionContext,
    ) -> anyhow::Result<()> {
        let mut tasks = tasks.to_vec();
        tasks.sort_by_key(|task| task.file_id);

        for tasks in tasks.chunk_by(|left, right| left.file_id == right.file_id) {
            self.lower_file_tasks(tasks[0].file_id, tasks, &mut *macro_expansion)
                .context("lower file body tasks")?;
        }

        Ok(())
    }

    #[rg_std::cancelable("body lowering task", token = self.cancellation)]
    fn lower_file_tasks(
        &mut self,
        file_id: FileId,
        tasks: &[BodyLoweringTask],
        macro_expansion: &mut dyn BodyMacroExpansionContext,
    ) -> anyhow::Result<()> {
        let parse = SourceReader::new(self.db).syntax(file_id, self.edition)?;
        let syntax = parse.tree();
        // Source readers resolve owner occurrences before scheduling. Every task now carries
        // a byte range in exactly the syntax snapshot opened above.
        for task in tasks {
            rg_std::check_cancel!(self.cancellation, "body lowering task");
            let span = task.span;
            let range = TextRange::new(span.start.into(), span.end.into());
            if !syntax.syntax().text_range().contains_range(range) {
                continue;
            }
            let kind = match task.owner {
                BodyOwner::Function(_) => SyntaxKind::FN,
                BodyOwner::Const(_) => SyntaxKind::CONST,
                BodyOwner::Static(_) => SyntaxKind::STATIC,
            };
            let Some(node) = syntax
                .syntax()
                .covering_element(range)
                .ancestors()
                .find(|node| node.kind() == kind && node.text_range() == range)
            else {
                continue;
            };
            let mut lowering = BodyLowering::new(
                task.owner,
                task.owner_module,
                task.fallback_module,
                source_for(file_id, &node),
                self.cfg,
                &mut *macro_expansion,
            );
            // Parser recovery can leave meaning outside the nodes we lower. Keep exact text for
            // that owner only. parse_errors() reads the shared parser result without validating
            // the whole file again for every function in a broad hint request.
            if parse
                .parse_errors()
                .iter()
                .any(|error| range.intersect(error.range()).is_some())
            {
                lowering.record_unmodeled_syntax(&node);
            }
            match task.owner {
                BodyOwner::Function(_) => {
                    let ast_fn = ast::Fn::cast(node).expect("body task kind matches syntax");
                    let Some(body_ast) = ast_fn.body() else {
                        continue;
                    };

                    let body = lowering.lower_function(ast_fn, body_ast);
                    self.lowered_bodies.alloc_body(body);
                }
                BodyOwner::Const(_) => {
                    let ast_const = ast::Const::cast(node).expect("body task kind matches syntax");
                    let Some(body_ast) = ast_const.body() else {
                        continue;
                    };

                    let body = lowering.lower_initializer(body_ast);
                    self.lowered_bodies.alloc_body(body);
                }
                BodyOwner::Static(_) => {
                    let ast_static =
                        ast::Static::cast(node).expect("body task kind matches syntax");
                    let Some(body_ast) = ast_static.body() else {
                        continue;
                    };

                    let body = lowering.lower_initializer(body_ast);
                    self.lowered_bodies.alloc_body(body);
                }
            }
        }

        Ok(())
    }
}
