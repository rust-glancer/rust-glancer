//! Scheduled body lowering.
//!
//! A task opens either a file item or an item from its owning macro expansion. Both sources
//! supply the same function or initializer syntax to the body lowerer.

use std::sync::Arc;

use anyhow::Context as _;
use rg_cfg_eval::CfgEvaluator;
use rg_def_map::GeneratedBodySyntax;
use rg_ir_model::{ConstRef, FileId, FunctionRef, ModuleRef, OuterBodyOwner, Span, StaticRef};
use rg_parse::SourceReader;
use rg_semantic_ir::BodyOwnerData;
use rg_source::QueryContext;
use rg_std::CancellationToken;
use rg_syntax::{AstNode as _, SyntaxKind, SyntaxNode, TextRange, ast};
use rg_text::RustEdition;

use super::{
    LoweredBodies, arguments::ExpansionCopies, body::BodyLowering,
    macro_expansion::BodyMacroExpansionContext, syntax::source_for,
};
use crate::BodyOwner;

/// A function body or item initializer that should become immutable `BodyData`.
#[derive(Debug)]
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
    pub(crate) source: BodyTaskSource,
}

impl BodyLoweringTask {
    pub(crate) fn for_owner(root: BodyOwnerData, source: BodyTaskSource) -> Self {
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
            source,
        }
    }
}

/// Expansion context to restore when a generated function or initializer gets its own body.
///
/// A nested body has its own source IDs. Keep its expansion's copy IDs so uses in siblings
/// still count toward the same macro input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GeneratedBodyTask {
    pub(super) body: GeneratedBodySyntax,
    pub(super) depth: usize,
    pub(super) copies: Option<Arc<ExpansionCopies>>,
}

impl GeneratedBodyTask {
    pub(crate) fn for_owner(body: GeneratedBodySyntax) -> Self {
        // TODO: Share copy provenance across item-scope owners. An outer item expansion can
        // emit syntax outside this worklist.
        Self {
            body,
            depth: 0,
            copies: None,
        }
    }
}

#[derive(Debug)]
pub(crate) enum BodyTaskSource {
    File { file: FileId, span: Span },
    Generated(GeneratedBodyTask),
}

impl BodyTaskSource {
    fn physical_range(&self) -> (FileId, Span) {
        match self {
            Self::File { file, span } => (*file, *span),
            Self::Generated(task) => (task.body.source.file_id, task.body.source.span),
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

    /// File items share the parse for their file. Generated items already carry the syntax
    /// selected inside an expansion, even when several bodies share the same invocation range.
    pub(crate) fn lower_tasks(
        &mut self,
        tasks: &mut [BodyLoweringTask],
        macro_expansion: &mut dyn BodyMacroExpansionContext,
    ) -> anyhow::Result<()> {
        tasks.sort_by_key(|task| {
            let (file, span) = task.source.physical_range();
            (file, span.start, span.end)
        });

        for tasks in tasks.chunk_by(|left, right| {
            left.source.physical_range().0 == right.source.physical_range().0
        }) {
            self.lower_file_tasks(tasks, macro_expansion)
                .context("lower body tasks")?;
        }
        Ok(())
    }

    #[rg_std::cancelable("body lowering task", token = self.cancellation)]
    fn lower_file_tasks(
        &mut self,
        tasks: &[BodyLoweringTask],
        macro_expansion: &mut dyn BodyMacroExpansionContext,
    ) -> anyhow::Result<()> {
        let file = tasks
            .first()
            .expect("file task group is non-empty")
            .source
            .physical_range()
            .0;
        let source_reader = SourceReader::new(self.db);
        let written_text = source_reader.text(file)?;
        let mut parse = None;
        for task in tasks {
            rg_std::check_cancel!(self.cancellation, "body lowering task");
            let (node, source, has_errors) = match &task.source {
                BodyTaskSource::File { file, span } => {
                    if parse.is_none() {
                        parse = Some(source_reader.syntax(*file, self.edition)?);
                    }
                    let parse = parse.as_ref().expect("written body has a file parse");
                    let tree = parse.tree();
                    let range = TextRange::new(span.start.into(), span.end.into());
                    if !tree.syntax().text_range().contains_range(range) {
                        continue;
                    }
                    let kind = match task.owner {
                        BodyOwner::Function(_) => SyntaxKind::FN,
                        BodyOwner::Const(_) => SyntaxKind::CONST,
                        BodyOwner::Static(_) => SyntaxKind::STATIC,
                    };
                    let Some(node) = tree
                        .syntax()
                        .covering_element(range)
                        .ancestors()
                        .find(|node| node.kind() == kind && node.text_range() == range)
                    else {
                        continue;
                    };
                    // Parser recovery can leave meaning outside the lowered nodes. Keep the
                    // exact text for just the owner that intersects an error.
                    let has_errors = parse
                        .parse_errors()
                        .iter()
                        .any(|error| range.intersect(error.range()).is_some());
                    let source = source_for(*file, &node);
                    (node, source, has_errors)
                }
                BodyTaskSource::Generated(task) => (
                    task.body.syntax.clone(),
                    task.body.source,
                    task.body
                        .expansion
                        .has_parse_errors_in(task.body.syntax.text_range()),
                ),
            };
            // A macro can emit `fn next() { recurse!(); }` and defer the next call to a new
            // body task. Resume the enclosing expansion's depth so that call uses the same
            // recursion budget.
            let _expansion_scope = match &task.source {
                BodyTaskSource::Generated(task) => {
                    Some(macro_expansion.resume_expansion(task.depth))
                }
                BodyTaskSource::File { .. } => None,
            };
            let mut lowering = BodyLowering::new(
                task.owner,
                task.owner_module,
                task.fallback_module,
                source,
                &written_text,
                self.cfg,
                macro_expansion,
                &mut self.lowered_bodies.argument_copies,
            );
            if let BodyTaskSource::Generated(task) = &task.source {
                lowering.set_expansion(task);
            }
            if has_errors {
                lowering.record_unmodeled_syntax(&node);
            }
            if let Some(body) = Self::lower_node(lowering, node) {
                self.lowered_bodies.alloc_body(body);
            }
        }
        Ok(())
    }

    fn lower_node(lowering: BodyLowering<'_>, node: SyntaxNode) -> Option<super::LoweredBodyData> {
        // Acquiring the AST is source-specific. From here on, every body lowers the same way.
        match ast::Item::cast(node)? {
            ast::Item::Fn(function) => {
                let body = function.body()?;
                Some(lowering.lower_function(function, body))
            }
            ast::Item::Const(konst) => Some(lowering.lower_initializer(konst.body()?)),
            ast::Item::Static(item) => Some(lowering.lower_initializer(item.body()?)),
            _ => None,
        }
    }
}
