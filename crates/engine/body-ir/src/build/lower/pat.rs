//! Pattern lowering and binding allocation for destructuring syntax.

use rg_syntax::{
    AstNode as _,
    ast::{self, HasName as _, RangeItem as _},
};

use rg_item_tree::{FieldKey, TypeRef};
use rg_text::Name;

use crate::ir::{
    BindingData, BindingId, BindingKind, BodyPath, BodyTy, ExprId, LiteralKind, PatBindingMode,
    PatData, PatId, PatKind, PatMutability, PatRangeKind, RecordPatField, ScopeId,
    path::{BodyPathSegment, BodyPathSegmentKind},
};

use super::function::FunctionBodyLowering;

impl FunctionBodyLowering<'_> {
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
        let source = self.source(pat.syntax());
        let pat_kind = match pat {
            ast::Pat::BoxPat(pat) => {
                let Some(inner) = pat.pat() else {
                    return self.alloc_unsupported_pat(pat.syntax());
                };
                PatKind::Box {
                    pat: self.lower_pat_inner(inner, scope, kind, None, alloc_bindings, bindings),
                }
            }
            ast::Pat::IdentPat(pat) => {
                let Some(name_ast) = pat.name() else {
                    return self.alloc_unsupported_pat(pat.syntax());
                };
                let name_span = self.source(name_ast.syntax()).span;
                let name = self.intern_ast_name(name_ast);
                let mode = PatBindingMode {
                    by_ref: pat.ref_token().is_some(),
                    mutable: pat.mut_token().is_some(),
                };
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

                // Bare identifiers need real pattern resolution to decide between local binding
                // and unit-variant path. Keep existing binding visibility stable while preserving
                // the path-shaped interpretation in the IR.
                let binding = if !alloc_bindings
                    || ambiguous_path.is_some() && is_capitalized(name.as_str())
                {
                    None
                } else {
                    self.push_pat_binding(
                        pat.syntax(),
                        scope,
                        kind,
                        name,
                        annotation.clone(),
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
                    annotation,
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
                        let pat = if let Some(inner) = field.pat() {
                            self.lower_pat_inner(inner, scope, kind, None, alloc_bindings, bindings)
                        } else {
                            self.lower_record_shorthand_pat(
                                field.syntax(),
                                scope,
                                kind,
                                name,
                                alloc_bindings,
                                bindings,
                            )
                        };
                        Some(RecordPatField {
                            key,
                            key_span,
                            source_span,
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
                    mutability: if pat.mut_token().is_some() {
                        PatMutability::Mut
                    } else {
                        PatMutability::Shared
                    },
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
                    .map(LiteralKind::from_ast)
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
                kind: pat.op_kind().map(|kind| match kind {
                    ast::RangeOp::Exclusive => PatRangeKind::Exclusive,
                    ast::RangeOp::Inclusive => PatRangeKind::Inclusive,
                }),
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
        syntax: &rg_syntax::SyntaxNode,
        scope: ScopeId,
        kind: BindingKind,
        name: Name,
        annotation: Option<TypeRef>,
        bindings: &mut Vec<BindingId>,
    ) -> Option<BindingId> {
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

        let binding = self.builder.alloc_binding(BindingData {
            source: self.source(syntax),
            scope,
            kind,
            name: Some(name),
            annotation,
            ty: BodyTy::Unknown,
        });
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
        syntax: &rg_syntax::SyntaxNode,
        scope: ScopeId,
        kind: BindingKind,
        name: Name,
        alloc_bindings: bool,
        bindings: &mut Vec<BindingId>,
    ) -> PatId {
        let binding = alloc_bindings
            .then(|| self.push_pat_binding(syntax, scope, kind, name, None, bindings))
            .flatten();
        self.builder.alloc_pat(PatData {
            source: self.source(syntax),
            kind: PatKind::Binding {
                mode: PatBindingMode::DEFAULT,
                binding,
                subpat: None,
                path: None,
            },
        })
    }

    fn alloc_unsupported_pat(&mut self, syntax: &rg_syntax::SyntaxNode) -> PatId {
        self.builder.alloc_pat(PatData {
            source: self.source(syntax),
            kind: PatKind::Unsupported,
        })
    }
}

fn is_capitalized(name: &str) -> bool {
    name.bytes()
        .next()
        .is_some_and(|byte| byte.is_ascii_uppercase())
}
