//! Shared lowering context for expression bodies.

use std::sync::Arc;

use rg_cfg_eval::CfgEvaluator;
use rg_def_map::{
    BodyMacroCallOrigin, BodyMacroExprExpansion, CopiedArgumentFragments, ExpandedBodyMacro,
    GeneratedBodySyntax,
};
use rg_ir_model::{CrateRef, ExprId, LocalDefRef, ModuleRef, ScopeId, SourceId, Span};
use rg_item_tree::{ItemKind, ItemNode, RecordSource, SourceRole};
use rg_syntax::{AstNode, SyntaxNode, TextRange, ast};

use super::{
    arguments::{ArgumentCopies, ExpansionCopies},
    builder::{BodyBuilder, LoweredBodyData},
    macro_expansion::BodyMacroExpansionContext,
    syntax::source_for,
    task::GeneratedBodyTask,
};
use crate::body::{BodyMacroCallData, BodyOwner, BodySource, ExprData, ExprKind};

pub(super) struct BodyLowering<'a> {
    pub(super) owner: BodyOwner,
    owner_module: ModuleRef,
    fallback_module: ModuleRef,
    body_source: BodySource,
    written_text: &'a str,
    pub(super) cfg: CfgEvaluator<'a>,
    pub(super) builder: BodyBuilder,
    pub(super) macro_expansion: &'a mut dyn BodyMacroExpansionContext,
    generated_context: Option<GeneratedBodyMacroContext>,
    argument_copies: &'a mut ArgumentCopies,
}

impl<'a> BodyLowering<'a> {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        owner: BodyOwner,
        owner_module: ModuleRef,
        fallback_module: ModuleRef,
        body_source: BodySource,
        written_text: &'a str,
        cfg: CfgEvaluator<'a>,
        macro_expansion: &'a mut dyn BodyMacroExpansionContext,
        argument_copies: &'a mut ArgumentCopies,
    ) -> Self {
        Self {
            owner,
            owner_module,
            fallback_module,
            body_source,
            written_text,
            cfg,
            builder: BodyBuilder::default(),
            macro_expansion,
            generated_context: None,
            argument_copies,
        }
    }

    pub(super) fn lower_function(
        mut self,
        function: ast::Fn,
        body: ast::BlockExpr,
    ) -> LoweredBodyData {
        // Parameters live in the function's outer lexical scope. The body block gets a child scope
        // so locals do not appear before the function boundary.
        let param_scope = self.builder.alloc_scope(None);
        let function_params = self.lower_params(function.param_list(), param_scope);
        let root_expr = self.lower_block_expr(body, param_scope);

        self.builder.finish(
            self.owner,
            self.owner_module,
            self.fallback_module,
            self.body_source,
            param_scope,
            root_expr,
            function_params,
        )
    }

    pub(super) fn lower_initializer(mut self, expr: ast::Expr) -> LoweredBodyData {
        let root_scope = self.builder.alloc_scope(None);
        // Item initializers are expression bodies without parameters. They still need a root scope
        // so ordinary body path resolution, type paths, and source scans can use the same pipeline
        // as function bodies.
        let root_expr = self.lower_expr(expr, root_scope);

        self.builder.finish(
            self.owner,
            self.owner_module,
            self.fallback_module,
            self.body_source,
            root_scope,
            root_expr,
            Vec::new(),
        )
    }
}

impl BodyLowering<'_> {
    pub(super) fn record_unmodeled_syntax(&mut self, syntax: &SyntaxNode) {
        self.builder
            .unmodeled_syntax
            .push(syntax.text().to_string());
    }

    pub(super) fn alloc_expr(
        &mut self,
        syntax: &SyntaxNode,
        scope: ScopeId,
        kind: ExprKind,
    ) -> ExprId {
        // Expressions can need a whole written range, for example a copied call whose
        // argument positions are shown in the editor. Name locations still use their own IDs.
        let source = self.expression_source(syntax);
        let source = self.builder.sources.record(source);
        self.alloc_expr_at(source, scope, kind)
    }

    pub(super) fn alloc_expr_at(
        &mut self,
        source: SourceId,
        scope: ScopeId,
        kind: ExprKind,
    ) -> ExprId {
        // Name resolution uses this boundary to avoid seeing bindings introduced after the
        // expression, while still allowing earlier bindings in the same lexical scope.
        let visible_bindings = self.builder.bindings.len();
        self.builder.alloc_expr(ExprData {
            source,
            scope,
            visible_bindings,
            kind,
        })
    }

    pub(super) fn source(&self, syntax: &SyntaxNode) -> BodySource {
        if let Some(context) = &self.generated_context {
            // In `($member:ident) => { receiver.$member() }`, the call as a whole maps to
            // the invocation. The member token can have an exact location of its own.
            if let Some(span) = context
                .expanded
                .source_for_exact_syntax(context.source, syntax)
            {
                return BodySource::macro_argument(context.source.file_id, span);
            }
            return BodySource::macro_generated(context.source.file_id, context.source.span);
        }

        source_for(self.body_source.file_id, syntax)
    }

    pub(super) fn expression_source(&self, syntax: &SyntaxNode) -> BodySource {
        if let Some(context) = &self.generated_context
            && let Some(span) = context.fragments.span(syntax)
        {
            return BodySource::macro_argument(context.source.file_id, span);
        }
        self.source(syntax)
    }

    /// Record a range within a node, such as its method name. In an expansion, use the whole
    /// invocation as a navigation fallback when the range has no exact copy in the input.
    pub(super) fn source_location(&mut self, range: TextRange) -> SourceId {
        self.with_sources(|sources| sources.record(range))
    }

    pub(super) fn source_id(&mut self, syntax: &SyntaxNode) -> SourceId {
        let source = self.source(syntax);
        self.builder.sources.record(source)
    }

    /// Record the physical interpretation while syntax and macro provenance are available.
    /// Type and declaration lowering receive the same recorder in ordinary and generated bodies.
    ///
    /// All calls append to this body's occurrence sequence, including type annotations that do
    /// not declare an item. Resolve locations here while the token origins are available.
    pub(super) fn with_sources<T>(&mut self, lower: impl FnOnce(&mut dyn RecordSource) -> T) -> T {
        let context = self.generated_context.as_ref();
        let map = |range, role| match context {
            Some(context) => {
                if matches!(role, SourceRole::Syntax)
                    && let Some((span, token)) =
                        context.expanded.argument_source(context.source, range)
                {
                    // For `"{user}"`, the written range covers `user` and the copy ID
                    // tracks the whole literal that carried it into this expansion.
                    let copy = context
                        .copies
                        .as_ref()
                        .and_then(|copies| copies.get(&token))
                        .copied();
                    return (
                        BodySource::macro_argument(context.source.file_id, span),
                        copy,
                    );
                }
                (
                    BodySource::macro_generated(context.source.file_id, context.source.span),
                    None,
                )
            }
            None => (
                BodySource::written(self.body_source.file_id, Span::from_text_range(range)),
                None,
            ),
        };
        lower(&mut self.builder.sources.recorder(&map))
    }

    pub(super) fn dollar_crate(&self) -> Option<CrateRef> {
        self.generated_context
            .as_ref()
            .map(|context| context.expanded.dollar_crate())
    }

    pub(super) fn dollar_crate_at(&self, token: TextRange) -> Option<CrateRef> {
        self.generated_context
            .as_ref()
            .map(|context| context.expanded.dollar_crate_at(token))
    }

    /// Classify the macro call syntax that is about to be expanded.
    ///
    /// User-written calls have no macro-definition crate. Calls found while recursively lowering
    /// generated syntax keep the generating definition's `$crate` identity and token map.
    pub(super) fn macro_call_origin(&self) -> BodyMacroCallOrigin {
        match &self.generated_context {
            Some(context) => context.expanded.call_origin(),
            None => BodyMacroCallOrigin::Source,
        }
    }

    /// Pick the semantic module used to resolve a body macro call.
    ///
    /// Top-level bodies resolve in their owner module. Body-local generated/nested bodies may have
    /// a synthetic module owner, so they fall back to the surrounding semantic module.
    ///
    /// Example: a macro inside a normal function resolves from that function's module, while a
    /// macro inside a generated nested body still resolves from the nearest real module.
    pub(super) fn macro_resolution_module(&self) -> ModuleRef {
        if self.owner_module.origin.as_crate_ref().is_some() {
            self.owner_module
        } else {
            self.fallback_module
        }
    }

    /// Lower one expanded macro under its source-map and macro-definition crate context.
    pub(super) fn with_expanded_macro<Syntax: AstNode, Output>(
        &mut self,
        call_source: BodySource,
        expanded: ExpandedBodyMacro<Syntax>,
        f: impl FnOnce(&mut Self, Syntax) -> Output,
    ) -> Output {
        let (syntax, expanded_context) = expanded.into_syntax_and_context();
        // A nested expansion uses its own map, then restores the parent before lowering continues.
        let previous = self.generated_context.take();
        // Exact locations can survive parser recovery or an incomplete parent expansion.
        // Keep those locations for navigation and leave copy coverage unknown.
        let copies = (!expanded_context.has_parse_errors()
            && previous
                .as_ref()
                .is_none_or(|context| context.copies.is_some()))
        .then(|| {
            self.argument_copies.expand(
                &expanded_context,
                call_source,
                previous
                    .as_ref()
                    .and_then(|context| context.copies.as_deref()),
            )
        })
        .map(Arc::new);
        let fragments = CopiedArgumentFragments::new(
            &expanded_context,
            syntax.syntax(),
            call_source,
            self.written_text,
        );
        self.generated_context = Some(GeneratedBodyMacroContext {
            source: call_source,
            expanded: expanded_context,
            copies,
            fragments,
        });
        let result = f(self, syntax);
        self.generated_context = previous;
        result
    }

    pub(super) fn set_expansion(&mut self, task: &GeneratedBodyTask) {
        let fragments = CopiedArgumentFragments::new(
            &task.body.expansion,
            &task.body.syntax,
            task.body.source,
            self.written_text,
        );
        self.generated_context = Some(GeneratedBodyMacroContext {
            source: task.body.source,
            expanded: task.body.expansion.clone(),
            copies: task.copies.clone(),
            fragments,
        });
    }

    /// Keep a generated item's syntax until its body-local declaration gets a semantic owner.
    pub(super) fn defer_generated_body(&mut self, item: &ItemNode, syntax: &SyntaxNode) {
        let Some(context) = &self.generated_context else {
            return;
        };
        let has_body = match &item.kind {
            ItemKind::Function(function) => function.has_body,
            ItemKind::Const(konst) => konst.has_value,
            ItemKind::Static(_) => {
                ast::Static::cast(syntax.clone()).is_some_and(|item| item.body().is_some())
            }
            _ => false,
        };
        if has_body {
            self.builder.generated_bodies.insert(
                item.location,
                GeneratedBodyTask {
                    body: GeneratedBodySyntax {
                        syntax: syntax.clone(),
                        expansion: context.expanded.clone(),
                        // The invocation may be handwritten. This generated item's range
                        // is a fallback shared with any siblings from the same call.
                        source: BodySource::macro_generated(
                            context.source.file_id,
                            context.source.span,
                        ),
                    },
                    depth: self.macro_expansion.expansion_depth(),
                    copies: context.copies.clone(),
                },
            );
        }
    }

    /// Record a user-written macro call as a navigation/reference candidate.
    ///
    /// A nested invocation copied from an argument is still user-written syntax, while helper
    /// invocations introduced by the template stay hidden. Keep the definition selected by
    /// expansion so source queries use the same macro as lowering.
    pub(super) fn record_source_macro_call(
        &mut self,
        call_source: BodySource,
        call: &ast::MacroCall,
        origin: BodyMacroCallOrigin,
        definition: LocalDefRef,
    ) {
        let Some(path) = call.path() else {
            return;
        };
        let Some(name_ref) = path.segment().and_then(|segment| segment.name_ref()) else {
            return;
        };

        if !matches!(origin, BodyMacroCallOrigin::Source)
            && self
                .generated_context
                .as_ref()
                .and_then(|context| {
                    context
                        .expanded
                        .source_for_exact_syntax(context.source, name_ref.syntax())
                })
                .is_none()
        {
            return;
        }

        let source = self.builder.sources.record(call_source);
        let name_location = self.source_location(name_ref.syntax().text_range());
        self.builder.push_macro_call(BodyMacroCallData {
            source,
            name_location,
            definition,
        });
    }

    /// Resolve an expression-position macro call from its original source location.
    ///
    /// Example: `let value = make_expr!(input);` expands to syntax and is lowered in the current
    /// lexical scope. `format_args!("hi")` becomes a builtin expression only if no user macro with
    /// that name resolves first. If both paths fail, the caller keeps the original macro
    /// expression.
    pub(super) fn lower_macro_call_from_call_site(
        &mut self,
        call_source: BodySource,
        call: &ast::MacroCall,
        scope: ScopeId,
    ) -> Option<ExprId> {
        let module = self.macro_resolution_module();
        let origin = self.macro_call_origin();

        let _expansion_scope = self.macro_expansion.expansion_scope()?;

        // Expansion is best-effort during mechanical lowering: unresolved, unsupported, or failing
        // macros should leave normal unknown expressions instead of aborting the whole body build.
        let outcome = self
            .macro_expansion
            .expand_expr_call(module, call_source, origin.clone(), call)
            .ok()
            .flatten()?;
        self.record_source_macro_call(call_source, call, origin, outcome.definition);
        let expansion = outcome.expansion?;

        match expansion {
            BodyMacroExprExpansion::Expanded(expanded) => {
                // The macro call remains the fallback range for generated syntax. Leaf syntax that
                // the expansion span map can trace back to invocation input gets a narrower source
                // span during recursive lowering.
                Some(
                    self.with_expanded_macro(call_source, expanded, |this, syntax| {
                        this.lower_expr(syntax, scope)
                    }),
                )
            }
            BodyMacroExprExpansion::Builtin {
                kind,
                arguments,
                format_template,
            } => {
                // Builtin input is parsed separately, so lower it with its own token map just as
                // we lower declarative output. The surrounding builtin keeps its intrinsic type.
                let args = arguments
                    .map(|arguments| {
                        self.with_expanded_macro(call_source, arguments, |this, syntax| {
                            this.lower_builtin_macro_args(syntax, format_template, scope)
                        })
                    })
                    .unwrap_or_default();
                let source = self.builder.sources.record(call_source);
                Some(self.alloc_expr_at(source, scope, ExprKind::BuiltinMacro { kind, args }))
            }
        }
    }
}

/// Temporary context for syntax produced by one body macro expansion.
///
/// The macro call source is the stable fallback for generated syntax. When the expansion span map
/// proves that a small token came from the invocation, the token gets its original argument span
/// instead. For example, `make_expr!(input)` can expose `input` as the source of a generated path,
/// while the surrounding binary expression still belongs to the macro call.
struct GeneratedBodyMacroContext {
    source: BodySource,
    expanded: ExpandedBodyMacro<SyntaxNode>,
    copies: Option<Arc<ExpansionCopies>>,
    fragments: CopiedArgumentFragments,
}
