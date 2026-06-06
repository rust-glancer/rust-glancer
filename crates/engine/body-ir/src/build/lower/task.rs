//! Scheduled body lowering.
//!
//! By the time we decide that a body should be lowered, its owner usually comes from an item
//! store and is identified by source span rather than by a syntax node. This module is the bridge:
//! it groups those scheduled owners by file, opens each parsed file once, finds the matching AST
//! item by span, and then delegates the actual expression lowering to `BodyLowering`.

use std::collections::HashMap;

use anyhow::Context as _;
use rg_syntax::{AstNode as _, ast};

use rg_ir_model::{BodyId, ModuleRef};
use rg_parse::{FileId, Span};
use rg_text::NameInterner;

use crate::ir::{BodyOwner, TargetBodies};

use super::{body::BodyLowering, syntax::source_for};

/// A function body or item initializer that should become a `BodyData`.
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

pub(crate) struct BodyTaskLowering<'a> {
    parse_package: &'a rg_parse::Package,
    target_bodies: &'a mut TargetBodies,
    interner: &'a mut NameInterner,
}

impl<'a> BodyTaskLowering<'a> {
    pub(crate) fn new(
        parse_package: &'a rg_parse::Package,
        target_bodies: &'a mut TargetBodies,
        interner: &'a mut NameInterner,
    ) -> Self {
        Self {
            parse_package,
            target_bodies,
            interner,
        }
    }

    /// Lowers scheduled bodies in file-sized batches.
    ///
    /// Parsing a file gives us all syntax nodes in that file, so lowering all matching tasks while
    /// the syntax tree is alive keeps memory use predictable and gives deterministic `BodyId`s.
    pub(crate) fn lower_tasks(
        &mut self,
        tasks: &[BodyLoweringTask],
    ) -> anyhow::Result<Vec<BodyId>> {
        let mut tasks = tasks.to_vec();
        tasks.sort_by_key(|task| task.file_id.0);

        let mut file_ids = tasks.iter().map(|task| task.file_id).collect::<Vec<_>>();
        file_ids.sort_by_key(|file_id| file_id.0);
        file_ids.dedup();

        let mut lowered = Vec::new();
        for file_id in file_ids {
            let range = task_range_for_file(&tasks, file_id);
            self.lower_file_tasks(file_id, &tasks[range], &mut lowered)?;
        }

        Ok(lowered)
    }

    fn lower_file_tasks(
        &mut self,
        file_id: FileId,
        tasks: &[BodyLoweringTask],
        lowered: &mut Vec<BodyId>,
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

        // Tasks are span-based so both top-level Semantic IR items and body-local items can use
        // the same lowering path. Build small file-local lookup maps once and reuse them below.
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

        for task in tasks {
            match task.owner {
                BodyOwner::Function(_) => {
                    let Some(ast_fn) = functions_by_span.get(&Self::span_key(task.span)).cloned()
                    else {
                        continue;
                    };
                    let Some(body_ast) = ast_fn.body() else {
                        continue;
                    };

                    let source = source_for(file_id, ast_fn.syntax());
                    let body = BodyLowering::new(
                        task.owner,
                        task.owner_module,
                        task.fallback_module,
                        source,
                        line_index,
                        self.interner,
                    )
                    .lower_function(ast_fn, body_ast);
                    lowered.push(self.target_bodies.alloc_body(body));
                }
                BodyOwner::Const(_) => {
                    let Some(ast_const) = consts_by_span.get(&Self::span_key(task.span)).cloned()
                    else {
                        continue;
                    };
                    let Some(body_ast) = ast_const.body() else {
                        continue;
                    };

                    let source = source_for(file_id, ast_const.syntax());
                    let body = BodyLowering::new(
                        task.owner,
                        task.owner_module,
                        task.fallback_module,
                        source,
                        line_index,
                        self.interner,
                    )
                    .lower_initializer(body_ast);
                    lowered.push(self.target_bodies.alloc_body(body));
                }
                BodyOwner::Static(_) => {
                    let Some(ast_static) = statics_by_span.get(&Self::span_key(task.span)).cloned()
                    else {
                        continue;
                    };
                    let Some(body_ast) = ast_static.body() else {
                        continue;
                    };

                    let source = source_for(file_id, ast_static.syntax());
                    let body = BodyLowering::new(
                        task.owner,
                        task.owner_module,
                        task.fallback_module,
                        source,
                        line_index,
                        self.interner,
                    )
                    .lower_initializer(body_ast);
                    lowered.push(self.target_bodies.alloc_body(body));
                }
            }
        }

        Ok(())
    }

    fn span_key(span: Span) -> (u32, u32) {
        (span.text.start, span.text.end)
    }
}

fn task_range_for_file(tasks: &[BodyLoweringTask], file_id: FileId) -> std::ops::Range<usize> {
    let start = tasks.partition_point(|task| task.file_id.0 < file_id.0);
    let end = tasks.partition_point(|task| task.file_id.0 <= file_id.0);
    start..end
}
