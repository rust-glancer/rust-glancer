//! Scheduled body lowering.
//!
//! By the time we decide that a body should be lowered, its owner usually comes from an item
//! store and is identified by source span rather than by a syntax node. This module is the bridge:
//! it groups those scheduled owners by file, opens each parsed file once, finds the matching AST
//! item by span, and then delegates the actual expression lowering to `BodyLowering`.

use std::sync::Arc;

use anyhow::Context as _;
use rg_cfg_eval::CfgEvaluator;
use rg_ir_model::{
    BodyId, ConstRef, FileId, FunctionRef, ModuleRef, SavedBodyOwner, Span, StaticRef,
};
use rg_parse::{CurrentSource, Package, saved_syntax};
use rg_semantic_ir::BodyOwnerData;
use rg_source::QueryContext;
use rg_std::CancellationToken;
use rg_syntax::{AstNode as _, SourceFile, SyntaxError, SyntaxKind, TextRange, ast};
use rg_text::RustEdition;

use super::{
    CurrentRootItems, LoweredBodies, body::BodyLowering,
    macro_expansion::BodyMacroExpansionContext, syntax::source_for,
};
use crate::BodyOwner;

/// A function body or item initializer that should become immutable `BodyData`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BodyLoweringTask {
    pub(crate) owner: BodyOwner,
    pub(crate) current_root_items: CurrentRootItems,
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
    pub(crate) fn for_saved_owner(root: BodyOwnerData, span: Span) -> Self {
        let store = root.store_ref();
        let owner = match root.owner {
            SavedBodyOwner::Function { id, .. } => BodyOwner::Function(FunctionRef::new(store, id)),
            SavedBodyOwner::Const { id, .. } => BodyOwner::Const(ConstRef::new(store, id)),
            SavedBodyOwner::Static(id) => BodyOwner::Static(StaticRef::new(store.origin, id)),
        };
        Self {
            owner,
            current_root_items: CurrentRootItems::None,
            owner_module: root.module,
            fallback_module: root.module,
            file_id: root.file,
            span,
        }
    }
}

/// Which immutable syntax tree scheduled body tasks must use.
///
/// Saved builds share tracked parses. A current request substitutes its one
/// captured document while retaining the saved package for any other source reached by lowering.
#[derive(Clone, Copy)]
pub(crate) enum BodyTaskSource<'a> {
    Tracked {
        db: &'a QueryContext<'a>,
        edition: RustEdition,
    },
    Current {
        package: &'a Package,
        file: FileId,
        source: &'a CurrentSource,
    },
}

impl<'a> BodyTaskSource<'a> {
    pub(crate) fn edition(self) -> RustEdition {
        match self {
            Self::Current { package, .. } => package.edition(),
            Self::Tracked { edition, .. } => edition,
        }
    }

    pub(crate) fn check_cancelled(self) -> anyhow::Result<()> {
        if let Self::Tracked { db, .. } = self {
            db.check_cancelled()?;
        }
        Ok(())
    }
}

/// One task together with the worklist slot allocated for its lowered body.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LoweredBodyTask {
    pub(crate) body: BodyId,
    pub(crate) task: BodyLoweringTask,
}

pub(crate) struct BodyTaskLowering<'a> {
    source: BodyTaskSource<'a>,
    lowered_bodies: &'a mut LoweredBodies,
    cfg: CfgEvaluator<'a>,
    cancellation: &'a CancellationToken,
}

impl<'a> BodyTaskLowering<'a> {
    pub(crate) fn new(
        source: BodyTaskSource<'a>,
        lowered_bodies: &'a mut LoweredBodies,
        cfg: CfgEvaluator<'a>,
        cancellation: &'a CancellationToken,
    ) -> Self {
        Self {
            source,
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
    ) -> anyhow::Result<Vec<LoweredBodyTask>> {
        let mut tasks = tasks.to_vec();
        tasks.sort_by_key(|task| task.file_id);

        let mut file_ids = tasks.iter().map(|task| task.file_id).collect::<Vec<_>>();
        file_ids.sort_unstable();
        file_ids.dedup();

        let mut lowered = Vec::new();
        for file_id in file_ids {
            let range = Self::task_range_for_file(&tasks, file_id);
            self.lower_file_tasks(file_id, &tasks[range], &mut lowered, &mut *macro_expansion)
                .context("lower file body tasks")?;
        }

        Ok(lowered)
    }

    #[rg_std::cancelable("body lowering task", token = self.cancellation)]
    fn lower_file_tasks(
        &mut self,
        file_id: FileId,
        tasks: &[BodyLoweringTask],
        lowered: &mut Vec<LoweredBodyTask>,
        macro_expansion: &mut dyn BodyMacroExpansionContext,
    ) -> anyhow::Result<()> {
        if let BodyTaskSource::Current { file, source, .. } = self.source
            && file == file_id
        {
            let parse = source.parse();
            let syntax = parse.tree();
            return self.lower_file_tasks_from_syntax(
                file_id,
                tasks,
                lowered,
                macro_expansion,
                &syntax,
                parse.parse_errors(),
            );
        }

        let parse = match self.source {
            BodyTaskSource::Tracked { db, edition } => saved_syntax(db, file_id, edition)?,
            BodyTaskSource::Current { package, .. } => Arc::new(
                package
                    .parsed_file(file_id)
                    .with_context(|| format!("fetch parsed source file {file_id:?}"))?
                    .parse_syntax()
                    .with_context(|| format!("parse body source {file_id:?}"))?,
            ),
        };
        self.lower_file_tasks_from_syntax(
            file_id,
            tasks,
            lowered,
            macro_expansion,
            &parse.tree(),
            parse.parse_errors(),
        )
    }

    fn lower_file_tasks_from_syntax(
        &mut self,
        file_id: FileId,
        tasks: &[BodyLoweringTask],
        lowered: &mut Vec<LoweredBodyTask>,
        macro_expansion: &mut dyn BodyMacroExpansionContext,
        syntax: &SourceFile,
        parse_errors: &[SyntaxError],
    ) -> anyhow::Result<()> {
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
                self.cancellation,
            );
            // Parser recovery can leave meaning outside the nodes we lower. Keep exact text for
            // that owner only. parse_errors() reads the shared parser result without validating
            // the whole file again for every function in a broad hint request.
            if parse_errors
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

                    let body = lowering
                        .lower_function(ast_fn, body_ast, task.current_root_items)
                        .context("lower function body")?;
                    lowered.push(LoweredBodyTask {
                        body: self.lowered_bodies.alloc_body(body),
                        task: *task,
                    });
                }
                BodyOwner::Const(_) => {
                    let ast_const = ast::Const::cast(node).expect("body task kind matches syntax");
                    let Some(body_ast) = ast_const.body() else {
                        continue;
                    };

                    let body = lowering
                        .lower_const(ast_const, body_ast, task.current_root_items)
                        .context("lower const body")?;
                    lowered.push(LoweredBodyTask {
                        body: self.lowered_bodies.alloc_body(body),
                        task: *task,
                    });
                }
                BodyOwner::Static(_) => {
                    let ast_static =
                        ast::Static::cast(node).expect("body task kind matches syntax");
                    let Some(body_ast) = ast_static.body() else {
                        continue;
                    };

                    let body = lowering
                        .lower_static(ast_static, body_ast, task.current_root_items)
                        .context("lower static body")?;
                    lowered.push(LoweredBodyTask {
                        body: self.lowered_bodies.alloc_body(body),
                        task: *task,
                    });
                }
            }
        }

        Ok(())
    }

    fn task_range_for_file(tasks: &[BodyLoweringTask], file_id: FileId) -> std::ops::Range<usize> {
        let start = tasks.partition_point(|task| task.file_id < file_id);
        let end = tasks.partition_point(|task| task.file_id <= file_id);
        start..end
    }
}
