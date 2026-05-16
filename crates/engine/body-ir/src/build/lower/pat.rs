//! Pattern lowering and binding allocation for destructuring syntax.

use ra_syntax::{
    AstNode as _,
    ast::{self, HasName as _},
};

use rg_def_map::{Path, PathSegment};
use rg_item_tree::{FieldKey, TypeRef};
use rg_text::Name;

use crate::ir::{
    BindingData, BindingId, BindingKind, BodyPath, BodyTy, PatData, PatId, PatKind, RecordPatField,
    ScopeId,
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
        let pat = self.lower_pat_inner(pat, scope, kind, annotation, &mut bindings);
        (Some(pat), bindings)
    }

    fn lower_pat_inner(
        &mut self,
        pat: ast::Pat,
        scope: ScopeId,
        kind: BindingKind,
        annotation: Option<TypeRef>,
        bindings: &mut Vec<BindingId>,
    ) -> PatId {
        let source = self.source(pat.syntax());
        let pat_kind = match pat {
            ast::Pat::BoxPat(pat) => {
                let Some(inner) = pat.pat() else {
                    return self.alloc_unsupported_pat(pat.syntax());
                };
                PatKind::Box {
                    pat: self.lower_pat_inner(inner, scope, kind, None, bindings),
                }
            }
            ast::Pat::IdentPat(pat) => {
                let Some(name_ast) = pat.name() else {
                    return self.alloc_unsupported_pat(pat.syntax());
                };
                let name_span = self.source(name_ast.syntax()).span;
                let name = self.intern_ast_name(name_ast);
                let subpat = pat
                    .pat()
                    .map(|pat| self.lower_pat_inner(pat, scope, kind, None, bindings));
                if is_capitalized_bare_pat(name.as_str(), &pat, subpat) {
                    PatKind::Path {
                        path: Some(BodyPath::new(
                            name_span,
                            Path {
                                absolute: false,
                                segments: vec![PathSegment::Name(name)],
                            },
                            vec![name_span],
                        )),
                    }
                } else {
                    let binding = self.push_pat_binding(
                        pat.syntax(),
                        scope,
                        kind,
                        name,
                        annotation.clone(),
                        bindings,
                    );
                    PatKind::Binding { binding, subpat }
                }
            }
            ast::Pat::OrPat(pat) => {
                let pats = pat
                    .pats()
                    .map(|inner| self.lower_pat_inner(inner, scope, kind, None, bindings))
                    .collect();
                PatKind::Or { pats }
            }
            ast::Pat::ParenPat(pat) => {
                let Some(inner) = pat.pat() else {
                    return self.alloc_unsupported_pat(pat.syntax());
                };
                return self.lower_pat_inner(inner, scope, kind, annotation, bindings);
            }
            ast::Pat::RecordPat(pat) => {
                let fields = pat
                    .record_pat_field_list()
                    .into_iter()
                    .flat_map(|field_list| field_list.fields())
                    .filter_map(|field| {
                        let field_name = field.field_name()?;
                        let key_span = self.source(field_name.syntax()).span;
                        let name = self.intern_ast_name_or_name_ref(field_name);
                        let key = FieldKey::Named(name.clone());
                        let source_span = self.source(field.syntax()).span;
                        let pat = if let Some(inner) = field.pat() {
                            self.lower_pat_inner(inner, scope, kind, None, bindings)
                        } else {
                            self.lower_record_shorthand_pat(
                                field.syntax(),
                                scope,
                                kind,
                                name,
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
                PatKind::Record {
                    path: pat.path().and_then(|path| self.lower_body_path(path)),
                    field_list_span: pat
                        .record_pat_field_list()
                        .as_ref()
                        .map(|field_list| self.source(field_list.syntax()).span),
                    fields,
                }
            }
            ast::Pat::RefPat(pat) => {
                let Some(inner) = pat.pat() else {
                    return self.alloc_unsupported_pat(pat.syntax());
                };
                PatKind::Ref {
                    pat: self.lower_pat_inner(inner, scope, kind, None, bindings),
                }
            }
            ast::Pat::SlicePat(pat) => {
                let fields = pat
                    .pats()
                    .map(|inner| self.lower_pat_inner(inner, scope, kind, None, bindings))
                    .collect();
                PatKind::Slice { fields }
            }
            ast::Pat::TuplePat(pat) => {
                let fields = pat
                    .fields()
                    .map(|inner| self.lower_pat_inner(inner, scope, kind, None, bindings))
                    .collect();
                PatKind::Tuple { fields }
            }
            ast::Pat::TupleStructPat(pat) => {
                let fields = pat
                    .fields()
                    .map(|inner| self.lower_pat_inner(inner, scope, kind, None, bindings))
                    .collect();
                PatKind::TupleStruct {
                    path: pat.path().and_then(|path| self.lower_body_path(path)),
                    fields,
                }
            }
            ast::Pat::PathPat(pat) => PatKind::Path {
                path: pat.path().and_then(|path| self.lower_body_path(path)),
            },
            ast::Pat::RestPat(_) | ast::Pat::WildcardPat(_) => PatKind::Wildcard,
            ast::Pat::ConstBlockPat(_)
            | ast::Pat::LiteralPat(_)
            | ast::Pat::MacroPat(_)
            | ast::Pat::RangePat(_) => PatKind::Unsupported,
        };

        self.builder.alloc_pat(PatData {
            source,
            kind: pat_kind,
        })
    }

    fn push_pat_binding(
        &mut self,
        syntax: &ra_syntax::SyntaxNode,
        scope: ScopeId,
        kind: BindingKind,
        name: Name,
        annotation: Option<TypeRef>,
        bindings: &mut Vec<BindingId>,
    ) -> Option<BindingId> {
        // Multiple bindings with the same textual name can appear in or-patterns. Keep the first
        // lowered binding so downstream snapshots and type propagation have one stable target.
        if bindings
            .iter()
            .filter_map(|binding| self.builder.bindings.get(*binding))
            .any(|binding| {
                binding
                    .name
                    .as_ref()
                    .is_some_and(|binding_name| binding_name == name.as_str())
            })
        {
            return None;
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

    fn lower_record_shorthand_pat(
        &mut self,
        syntax: &ra_syntax::SyntaxNode,
        scope: ScopeId,
        kind: BindingKind,
        name: Name,
        bindings: &mut Vec<BindingId>,
    ) -> PatId {
        let binding = self.push_pat_binding(syntax, scope, kind, name, None, bindings);
        self.builder.alloc_pat(PatData {
            source: self.source(syntax),
            kind: PatKind::Binding {
                binding,
                subpat: None,
            },
        })
    }

    fn alloc_unsupported_pat(&mut self, syntax: &ra_syntax::SyntaxNode) -> PatId {
        self.builder.alloc_pat(PatData {
            source: self.source(syntax),
            kind: PatKind::Unsupported,
        })
    }
}

fn is_capitalized_bare_pat(name: &str, pat: &ast::IdentPat, subpat: Option<PatId>) -> bool {
    // The syntax tree represents bare unit-variant patterns such as `None` as identifier
    // patterns. Until Body IR has true pattern name resolution, this avoids treating the common
    // capitalized unit-variant shape as a local binding.
    subpat.is_none()
        && pat.ref_token().is_none()
        && pat.mut_token().is_none()
        && name
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_uppercase())
}
