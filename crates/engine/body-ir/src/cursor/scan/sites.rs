//! Shared source-site discovery for body cursor scanners.
//!
//! Cursor queries interpret the same lowered syntax in several different ways: one scanner wants
//! navigable value paths, another wants completion owners, and another wants record field lists.
//! This module keeps the structural walk in one place while leaving those query-specific meanings
//! with the callers.

use rg_item_tree::{GenericArg, TypeBound, TypePath, TypeRef};
use rg_parse::{FileId, Span};

use crate::{BodyData, ExprKind, PatData, PatId, PatKind, ScopeId, StmtKind};

/// A source-owned pattern root together with the scope where its bindings live.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PatternSite {
    scope: ScopeId,
    pat: PatId,
    file_id: FileId,
    source_span: Span,
}

/// A type reference written inside a body-local source form.
#[derive(Debug, Clone, Copy)]
struct TypeRefSite<'body> {
    scope: ScopeId,
    visible_bindings: usize,
    file_id: FileId,
    ty: &'body TypeRef,
}

/// One pattern node reached from a body-local pattern root.
///
/// Nested nodes inherit the root pattern scope because all bindings in one pattern are introduced
/// into the same local scope.
#[derive(Debug, Clone, Copy)]
pub(super) struct PatternWalkSite<'body> {
    pub(super) scope: ScopeId,
    pub(super) data: &'body PatData,
}

/// One type path reached from a body-local type reference.
#[derive(Debug, Clone, Copy)]
pub(super) struct TypePathSite<'body> {
    pub(super) scope: ScopeId,
    pub(super) visible_bindings: usize,
    pub(super) file_id: FileId,
    pub(super) path: &'body TypePath,
}

/// Structural views over lowered body syntax used by cursor scans.
pub(super) struct BodyScanSites<'body> {
    body: &'body BodyData,
}

impl<'body> BodyScanSites<'body> {
    pub(super) fn new(body: &'body BodyData) -> Self {
        Self { body }
    }

    /// Walks pattern nodes that belong to body-local pattern syntax.
    ///
    /// The file filter applies to both roots and visited nodes. The offset filter only decides
    /// whether a root pattern should be explored; callers still interpret each node's own spans.
    pub(super) fn walk_pats(
        &self,
        file_id: Option<FileId>,
        offset: Option<u32>,
        mut visit: impl FnMut(PatternWalkSite<'body>),
    ) {
        self.for_each_pattern_site(|site| {
            if !Self::file_matches(file_id, site.file_id) {
                return;
            }
            if offset.is_some_and(|offset| !site.source_span.touches(offset)) {
                return;
            }

            self.walk_pat_inner(site.scope, site.pat, file_id, &mut visit);
        });
    }

    /// Walks type paths that belong to body-local type syntax.
    ///
    /// Type references can hide paths inside tuples, pointers, function pointers, and generic
    /// arguments. Callers receive every nested path with the body scope that owns the annotation.
    pub(super) fn walk_type_paths(
        &self,
        file_id: Option<FileId>,
        mut visit: impl FnMut(TypePathSite<'body>),
    ) {
        self.for_each_type_ref_site(|site| {
            if !Self::file_matches(file_id, site.file_id) {
                return;
            }

            self.walk_type_ref(site.ty, &mut |path| {
                visit(TypePathSite {
                    scope: site.scope,
                    visible_bindings: site.visible_bindings,
                    file_id: site.file_id,
                    path,
                });
            });
        });
    }

    fn for_each_pattern_site(&self, mut visit: impl FnMut(PatternSite)) {
        for statement in self.body.statements.iter() {
            let StmtKind::Let {
                scope,
                pat: Some(pat),
                ..
            } = &statement.kind
            else {
                continue;
            };
            self.visit_pattern_site(&mut visit, *scope, *pat);
        }

        for expr in self.body.exprs.iter() {
            match &expr.kind {
                ExprKind::Match { arms, .. } => {
                    for arm in arms {
                        if let Some(pat) = arm.pat {
                            self.visit_pattern_site(&mut visit, arm.scope, pat);
                        }
                    }
                }
                ExprKind::Let {
                    scope,
                    pat: Some(pat),
                    ..
                }
                | ExprKind::For {
                    scope,
                    pat: Some(pat),
                    ..
                } => self.visit_pattern_site(&mut visit, *scope, *pat),
                ExprKind::Closure { scope, params, .. } => {
                    for param in params {
                        if let Some(pat) = param.pat {
                            self.visit_pattern_site(&mut visit, *scope, pat);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn for_each_type_ref_site(&self, mut visit: impl FnMut(TypeRefSite<'body>)) {
        let visible_bindings = self.body.bindings().len();

        for statement in self.body.statements.iter() {
            let StmtKind::Let {
                scope,
                annotation: Some(annotation),
                ..
            } = &statement.kind
            else {
                continue;
            };

            visit(TypeRefSite {
                scope: *scope,
                visible_bindings,
                file_id: statement.source.file_id,
                ty: annotation,
            });
        }

        for expr in self.body.exprs.iter() {
            match &expr.kind {
                ExprKind::Closure {
                    scope,
                    params,
                    ret_ty,
                    ..
                } => {
                    for param in params {
                        if let Some(annotation) = &param.annotation {
                            visit(TypeRefSite {
                                scope: *scope,
                                visible_bindings,
                                file_id: param.source.file_id,
                                ty: annotation,
                            });
                        }
                    }

                    if let Some(ret_ty) = ret_ty {
                        visit(TypeRefSite {
                            scope: *scope,
                            visible_bindings,
                            file_id: expr.source.file_id,
                            ty: ret_ty,
                        });
                    }
                }
                ExprKind::Cast { ty: Some(ty), .. } => {
                    visit(TypeRefSite {
                        scope: expr.scope,
                        visible_bindings,
                        file_id: expr.source.file_id,
                        ty,
                    });
                }
                _ => {}
            }
        }
    }

    fn walk_type_ref(&self, ty: &'body TypeRef, visit: &mut impl FnMut(&'body TypePath)) {
        match ty {
            TypeRef::Path(path) => {
                visit(path);

                for segment in &path.segments {
                    for arg in &segment.args {
                        self.walk_generic_arg(arg, visit);
                    }
                }
            }
            TypeRef::Tuple(types) => {
                for ty in types {
                    self.walk_type_ref(ty, visit);
                }
            }
            TypeRef::Reference { inner, .. }
            | TypeRef::RawPointer { inner, .. }
            | TypeRef::Slice(inner) => self.walk_type_ref(inner, visit),
            TypeRef::Array { inner, .. } => self.walk_type_ref(inner, visit),
            TypeRef::FnPointer { params, ret } => {
                for param in params {
                    self.walk_type_ref(param, visit);
                }
                self.walk_type_ref(ret, visit);
            }
            TypeRef::ImplTrait(bounds) | TypeRef::DynTrait(bounds) => {
                for bound in bounds {
                    if let TypeBound::Trait(ty) = bound {
                        self.walk_type_ref(ty, visit);
                    }
                }
            }
            TypeRef::Unknown(_) | TypeRef::Never | TypeRef::Unit | TypeRef::Infer => {}
        }
    }

    fn visit_pattern_site(&self, visit: &mut impl FnMut(PatternSite), scope: ScopeId, pat: PatId) {
        let Some(data) = self.body.pat(pat) else {
            return;
        };

        visit(PatternSite {
            scope,
            pat,
            file_id: data.source.file_id,
            source_span: data.source.span,
        });
    }

    fn walk_pat_inner(
        &self,
        scope: ScopeId,
        pat: PatId,
        file_id: Option<FileId>,
        visit: &mut impl FnMut(PatternWalkSite<'body>),
    ) {
        let Some(data) = self.body.pat(pat) else {
            return;
        };

        if Self::file_matches(file_id, data.source.file_id) {
            visit(PatternWalkSite { scope, data });
        }

        match &data.kind {
            PatKind::TupleStruct { fields, .. }
            | PatKind::Tuple { fields }
            | PatKind::Or { pats: fields }
            | PatKind::Slice { fields } => {
                for field in fields {
                    self.walk_pat_inner(scope, *field, file_id, visit);
                }
            }
            PatKind::Record { fields, rest, .. } => {
                for field in fields {
                    self.walk_pat_inner(scope, field.pat, file_id, visit);
                }
                if let Some(rest) = rest {
                    self.walk_pat_inner(scope, *rest, file_id, visit);
                }
            }
            PatKind::Binding {
                subpat: Some(subpat),
                ..
            }
            | PatKind::Ref { pat: subpat, .. }
            | PatKind::Box { pat: subpat } => {
                self.walk_pat_inner(scope, *subpat, file_id, visit);
            }
            PatKind::Range { start, end, .. } => {
                if let Some(start) = start {
                    self.walk_pat_inner(scope, *start, file_id, visit);
                }
                if let Some(end) = end {
                    self.walk_pat_inner(scope, *end, file_id, visit);
                }
            }
            PatKind::Binding { subpat: None, .. }
            | PatKind::Path { .. }
            | PatKind::Rest
            | PatKind::Literal { .. }
            | PatKind::ConstBlock { .. }
            | PatKind::Wildcard
            | PatKind::Unsupported => {}
        }
    }

    fn walk_generic_arg(&self, arg: &'body GenericArg, visit: &mut impl FnMut(&'body TypePath)) {
        match arg {
            GenericArg::Type(ty) => self.walk_type_ref(ty, visit),
            GenericArg::AssocType { ty: Some(ty), .. } => self.walk_type_ref(ty, visit),
            GenericArg::Lifetime(_)
            | GenericArg::Const(_)
            | GenericArg::AssocType { ty: None, .. }
            | GenericArg::Unsupported(_) => {}
        }
    }

    fn file_matches(selected: Option<FileId>, file_id: FileId) -> bool {
        selected.is_none_or(|selected| selected == file_id)
    }
}
