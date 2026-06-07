//! Pattern lowering and binding allocation for destructuring syntax.

use rg_syntax::{
    AstNode as _,
    ast::{self, HasName as _, RangeItem as _},
};

use rg_ir_model::{
    BindingId, BodyPath, BodyPathSegment, BodyPathSegmentKind, ExprId, PatId, ScopeId,
    items::{FieldKey, TypeRef},
};
use rg_item_tree::{FromAst as _, RecordPatFieldAst};
use rg_text::Name;

use crate::ir::{
    BindingData, BindingKind, LiteralKind, PatBindingMode, PatData, PatKind, PatMutability,
    PatRangeKind, PendingBindingResolution, RecordFieldSyntax, RecordPatField,
};

use super::body::BodyLowering;

/// A priori binding information from the syntactic position of an identifier pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IdentPatBinding {
    /// Impossible to know whether this is a binding yet. Body resolution decides later.
    AmbiguousCandidate,
    /// This syntax position guarantees the identifier introduces a binding.
    SyntacticBinding,
}

#[derive(Debug, Clone)]
struct PatLoweringOptions {
    kind: BindingKind,
    annotation: Option<TypeRef>,
    alloc_bindings: bool,
    ident_binding: IdentPatBinding,
}

impl PatLoweringOptions {
    fn without_annotation(&self) -> Self {
        Self {
            kind: self.kind,
            annotation: None,
            alloc_bindings: self.alloc_bindings,
            ident_binding: self.ident_binding,
        }
    }
}

struct PatBindingRequest<'a> {
    syntax: &'a rg_syntax::SyntaxNode,
    name_span: rg_parse::Span,
    scope: ScopeId,
    kind: BindingKind,
    name: Name,
    annotation: Option<TypeRef>,
    resolution: PendingBindingResolution,
}

impl BodyLowering<'_> {
    pub(super) fn lower_pat(
        &mut self,
        pat: ast::Pat,
        scope: ScopeId,
        kind: BindingKind,
        annotation: Option<TypeRef>,
    ) -> (Option<PatId>, Vec<BindingId>) {
        let mut bindings = Vec::new();
        let pat = self.lower_pat_inner(pat, scope, kind, annotation, true, &mut bindings);
        (Some(pat), bindings)
    }

    // Note: some grammar positions are pattern-shaped without binding names, such
    // as range bounds that name constants. `alloc_bindings` preserves that source
    // shape without leaking fake locals into scope resolution.
    fn lower_pat_inner(
        &mut self,
        pat: ast::Pat,
        scope: ScopeId,
        kind: BindingKind,
        annotation: Option<TypeRef>,
        alloc_bindings: bool,
        bindings: &mut Vec<BindingId>,
    ) -> PatId {
        self.lower_pat_inner_with_ident_binding(
            pat,
            scope,
            PatLoweringOptions {
                kind,
                annotation,
                alloc_bindings,
                ident_binding: IdentPatBinding::AmbiguousCandidate,
            },
            bindings,
        )
    }

    fn lower_pat_inner_with_ident_binding(
        &mut self,
        pat: ast::Pat,
        scope: ScopeId,
        options: PatLoweringOptions,
        bindings: &mut Vec<BindingId>,
    ) -> PatId {
        let source = self.source(pat.syntax());
        let kind = options.kind;
        let alloc_bindings = options.alloc_bindings;
        let ident_binding = options.ident_binding;
        let pat_kind = match pat {
            ast::Pat::BoxPat(pat) => {
                let Some(inner) = pat.pat() else {
                    return self.alloc_unsupported_pat(pat.syntax());
                };
                PatKind::Box {
                    pat: self.lower_pat_inner_with_ident_binding(
                        inner,
                        scope,
                        options.without_annotation(),
                        bindings,
                    ),
                }
            }
            ast::Pat::IdentPat(pat) => {
                let Some(name_ast) = pat.name() else {
                    return self.alloc_unsupported_pat(pat.syntax());
                };
                let name_span = self.source(name_ast.syntax()).span;
                let name = self.intern_ast_name(name_ast);
                let mode = PatBindingMode::from_ast(&pat, ());
                let ambiguous_path = pat.is_simple_ident().then(|| {
                    BodyPath::new(
                        name_span,
                        false,
                        vec![BodyPathSegment::new(
                            BodyPathSegmentKind::Name(name.clone()),
                            name_span,
                            None,
                        )],
                    )
                });
                let subpat = pat.pat().map(|pat| {
                    self.lower_pat_inner(pat, scope, kind, None, alloc_bindings, bindings)
                });

                // Bare identifiers keep both meanings until body resolution can check the value
                // namespace. Syntax-known binding sites, such as parameters and record shorthand,
                // skip that ambiguity and materialize as bindings later.
                let binding = if !alloc_bindings {
                    None
                } else {
                    let resolution = if kind == BindingKind::Param
                        || ident_binding == IdentPatBinding::SyntacticBinding
                        || ambiguous_path.is_none()
                    {
                        PendingBindingResolution::AlwaysBinding
                    } else {
                        PendingBindingResolution::AmbiguousPattern
                    };
                    self.push_pat_binding(
                        PatBindingRequest {
                            syntax: pat.syntax(),
                            name_span,
                            scope,
                            kind,
                            name,
                            annotation: options.annotation.clone(),
                            resolution,
                        },
                        bindings,
                    )
                };

                PatKind::Binding {
                    mode,
                    binding,
                    subpat,
                    path: ambiguous_path,
                }
            }
            ast::Pat::OrPat(pat) => {
                let pats = pat
                    .pats()
                    .map(|inner| {
                        self.lower_pat_inner(inner, scope, kind, None, alloc_bindings, bindings)
                    })
                    .collect();
                PatKind::Or { pats }
            }
            ast::Pat::ParenPat(pat) => {
                let Some(inner) = pat.pat() else {
                    return self.alloc_unsupported_pat(pat.syntax());
                };
                return self.lower_pat_inner(
                    inner,
                    scope,
                    kind,
                    options.annotation.clone(),
                    alloc_bindings,
                    bindings,
                );
            }
            ast::Pat::RecordPat(pat) => {
                let field_list = pat.record_pat_field_list();
                let fields = field_list
                    .iter()
                    .flat_map(|field_list| field_list.fields())
                    .filter_map(|field| {
                        let field_name = field.field_name()?;
                        let key_span = self.source(field_name.syntax()).span;
                        let name = self.intern_ast_name_or_name_ref(field_name);
                        let key = FieldKey::Named(name.clone());
                        let source_span = self.source(field.syntax()).span;
                        let syntax = <RecordFieldSyntax as rg_item_tree::FromAst<
                            RecordPatFieldAst,
                        >>::from_ast(&field, RecordPatFieldAst);
                        let pat = if syntax.is_explicit() {
                            match field.pat() {
                                Some(inner) => self.lower_pat_inner(
                                    inner,
                                    scope,
                                    kind,
                                    None,
                                    alloc_bindings,
                                    bindings,
                                ),
                                None => self.alloc_unsupported_pat(field.syntax()),
                            }
                        } else {
                            match field.pat() {
                                Some(inner) => self.lower_record_shorthand_pat(
                                    inner,
                                    scope,
                                    kind,
                                    alloc_bindings,
                                    bindings,
                                ),
                                None => self.alloc_unsupported_pat(field.syntax()),
                            }
                        };
                        Some(RecordPatField {
                            key,
                            key_span,
                            source_span,
                            syntax,
                            pat,
                        })
                    })
                    .collect();
                let rest = field_list
                    .as_ref()
                    .and_then(|field_list| field_list.rest_pat())
                    .map(|rest| {
                        self.lower_pat_inner(
                            ast::Pat::RestPat(rest),
                            scope,
                            kind,
                            None,
                            alloc_bindings,
                            bindings,
                        )
                    });
                PatKind::Record {
                    path: pat.path().and_then(|path| self.lower_body_path(path)),
                    field_list_span: field_list
                        .as_ref()
                        .map(|field_list| self.source(field_list.syntax()).span),
                    fields,
                    rest,
                }
            }
            ast::Pat::RefPat(pat) => {
                let Some(inner) = pat.pat() else {
                    return self.alloc_unsupported_pat(pat.syntax());
                };
                PatKind::Ref {
                    mutability: PatMutability::from_ast(&pat, ()),
                    pat: self.lower_pat_inner(inner, scope, kind, None, alloc_bindings, bindings),
                }
            }
            ast::Pat::SlicePat(pat) => {
                let fields = pat
                    .pats()
                    .map(|inner| {
                        self.lower_pat_inner(inner, scope, kind, None, alloc_bindings, bindings)
                    })
                    .collect();
                PatKind::Slice { fields }
            }
            ast::Pat::TuplePat(pat) => {
                let fields = pat
                    .fields()
                    .map(|inner| {
                        self.lower_pat_inner(inner, scope, kind, None, alloc_bindings, bindings)
                    })
                    .collect();
                PatKind::Tuple { fields }
            }
            ast::Pat::TupleStructPat(pat) => {
                let fields = pat
                    .fields()
                    .map(|inner| {
                        self.lower_pat_inner(inner, scope, kind, None, alloc_bindings, bindings)
                    })
                    .collect();
                PatKind::TupleStruct {
                    path: pat.path().and_then(|path| self.lower_body_path(path)),
                    fields,
                }
            }
            ast::Pat::PathPat(pat) => PatKind::Path {
                path: pat.path().and_then(|path| self.lower_body_path(path)),
            },
            ast::Pat::RestPat(_) => PatKind::Rest,
            ast::Pat::LiteralPat(pat) => PatKind::Literal {
                kind: pat
                    .literal()
                    .as_ref()
                    .map(Self::literal_kind_from_ast)
                    .unwrap_or(LiteralKind::Unknown),
                negated: pat.minus_token().is_some(),
            },
            ast::Pat::RangePat(pat) => PatKind::Range {
                start: pat
                    .start()
                    .map(|start| self.lower_pat_inner(start, scope, kind, None, false, bindings)),
                end: pat
                    .end()
                    .map(|end| self.lower_pat_inner(end, scope, kind, None, false, bindings)),
                kind: pat.op_kind().map(|kind| PatRangeKind::from_ast(&kind, ())),
            },
            ast::Pat::ConstBlockPat(pat) => PatKind::ConstBlock {
                expr: pat
                    .block_expr()
                    .map(|block| self.lower_const_block_pat_expr(block)),
            },
            ast::Pat::WildcardPat(_) => PatKind::Wildcard,
            ast::Pat::MacroPat(_) => PatKind::Unsupported,
        };

        self.builder.alloc_pat(PatData {
            source,
            kind: pat_kind,
        })
    }

    fn push_pat_binding(
        &mut self,
        request: PatBindingRequest<'_>,
        bindings: &mut Vec<BindingId>,
    ) -> Option<BindingId> {
        let PatBindingRequest {
            syntax,
            name_span,
            scope,
            kind,
            name,
            annotation,
            resolution,
        } = request;

        // Multiple bindings with the same textual name can appear in or-patterns. Reuse the first
        // lowered binding so later occurrences do not look like value paths.
        if let Some(binding) = bindings.iter().copied().find(|binding| {
            self.builder.bindings.get(*binding).is_some_and(|binding| {
                binding
                    .name
                    .as_ref()
                    .is_some_and(|binding_name| binding_name == name.as_str())
            })
        }) {
            return Some(binding);
        }

        let binding = self.builder.alloc_pending_binding(
            BindingData {
                source: self.source(syntax),
                name_span: Some(name_span),
                scope,
                kind,
                name: Some(name),
                annotation,
            },
            resolution,
        );
        bindings.push(binding);
        Some(binding)
    }

    fn lower_const_block_pat_expr(&mut self, block: ast::BlockExpr) -> ExprId {
        // Const-pattern expressions resolve as constant/value lookups, not as uses of bindings
        // introduced by the surrounding pattern. Give the block an isolated parent scope so its
        // own locals still work without inheriting pattern bindings.
        let const_scope = self.builder.alloc_scope(None);
        self.lower_block_expr(block, const_scope)
    }

    fn lower_record_shorthand_pat(
        &mut self,
        pat: ast::Pat,
        scope: ScopeId,
        kind: BindingKind,
        alloc_bindings: bool,
        bindings: &mut Vec<BindingId>,
    ) -> PatId {
        // A colonless record field is still a real binding pattern: `User { ref name }` binds by
        // reference, and `User { mut name }` creates a mutable binding. The shorthand-specific rule
        // is that `User { Name }` names a field binding even though a standalone `Name` pattern may
        // resolve as a path-like unit variant.
        self.lower_pat_inner_with_ident_binding(
            pat,
            scope,
            PatLoweringOptions {
                kind,
                annotation: None,
                alloc_bindings,
                ident_binding: IdentPatBinding::SyntacticBinding,
            },
            bindings,
        )
    }

    fn alloc_unsupported_pat(&mut self, syntax: &rg_syntax::SyntaxNode) -> PatId {
        self.builder.alloc_pat(PatData {
            source: self.source(syntax),
            kind: PatKind::Unsupported,
        })
    }
}
