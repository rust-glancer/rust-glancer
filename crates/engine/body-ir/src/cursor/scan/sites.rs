//! Shared source-site discovery for body cursor scanners.
//!
//! Cursor queries interpret the same lowered syntax in several different ways: one scanner wants
//! navigable value paths, another wants completion owners, and another wants record field lists.
//! This module keeps the structural walk in one place while leaving those query-specific meanings
//! with the callers.

use rg_ir_model::{BodySource, PatId, ScopeId, items::ItemTreeId};
use rg_item_tree::{
    FieldItem, FieldList, FunctionItem, GenericParams, ImplItem, ItemKind, ItemNode, ModuleItem,
    ModuleSource, TypeBound, TypePath, TypeRef, WherePredicate,
};
use rg_parse::FileId;

use crate::{
    BodyPath, ExprKind, ResolvedBodyData, StmtKind,
    walk::{
        PatWalkSite, walk_body_path_type_refs as walk_embedded_body_path_type_refs,
        walk_generic_args_type_refs, walk_pat, walk_type_ref_paths,
    },
};

/// A source-owned pattern root together with the scope where its bindings live.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PatternSite {
    scope: ScopeId,
    pat: PatId,
    source: BodySource,
}

/// A type reference written inside a body-local source form.
#[derive(Debug, Clone, Copy)]
struct TypeRefSite<'body> {
    scope: ScopeId,
    visible_bindings: usize,
    source: BodySource,
    ty: &'body TypeRef,
}

/// Scope metadata carried by one written type reference.
#[derive(Debug, Clone, Copy)]
struct TypeRefContext {
    scope: ScopeId,
    visible_bindings: usize,
    source: BodySource,
}

impl TypeRefContext {
    fn new(scope: ScopeId, visible_bindings: usize, source: BodySource) -> Self {
        Self {
            scope,
            visible_bindings,
            source,
        }
    }
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
    body: &'body ResolvedBodyData,
}

impl<'body> BodyScanSites<'body> {
    pub(super) fn new(body: &'body ResolvedBodyData) -> Self {
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
        mut visit: impl FnMut(PatWalkSite<'body>),
    ) {
        self.for_each_pattern_site(|site| {
            if !site.source.is_written_in_selected_file(file_id) {
                return;
            }
            if offset.is_some_and(|offset| !site.source.span.touches(offset)) {
                return;
            }

            walk_pat(self.body, site.scope, site.pat, &mut |walk_site| {
                if walk_site.data.source.is_written_in_selected_file(file_id) {
                    visit(walk_site);
                }
            });
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
            if !site.source.is_written_in_selected_file(file_id) {
                return;
            }

            walk_type_ref_paths(site.ty, &mut |path| {
                visit(TypePathSite {
                    scope: site.scope,
                    visible_bindings: site.visible_bindings,
                    file_id: site.source.file_id,
                    path,
                });
            });
        });
    }

    fn for_each_pattern_site(&self, mut visit: impl FnMut(PatternSite)) {
        for statement in self.body.statements().iter() {
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

        for expr in self.body.exprs().iter() {
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
        TypeRefSiteWalker::new(self, &mut visit).walk();
    }

    fn visit_pattern_site(&self, visit: &mut impl FnMut(PatternSite), scope: ScopeId, pat: PatId) {
        let Some(data) = self.body.pat(pat) else {
            return;
        };

        visit(PatternSite {
            scope,
            pat,
            source: data.source,
        });
    }

}

/// Collects written type references while keeping scope policy close to each source form.
struct TypeRefSiteWalker<'scan, 'body, V>
where
    V: FnMut(TypeRefSite<'body>),
{
    sites: &'scan BodyScanSites<'body>,
    body_visible_bindings: usize,
    visit: &'scan mut V,
}

impl<'scan, 'body, V> TypeRefSiteWalker<'scan, 'body, V>
where
    V: FnMut(TypeRefSite<'body>),
{
    fn new(sites: &'scan BodyScanSites<'body>, visit: &'scan mut V) -> Self {
        Self {
            sites,
            body_visible_bindings: sites.body.bindings().len(),
            visit,
        }
    }

    fn walk(&mut self) {
        self.walk_let_annotations();
        self.walk_source_item_declarations();
        self.walk_expression_type_refs();
        self.walk_pattern_path_type_refs();
    }

    fn walk_let_annotations(&mut self) {
        let body = self.sites.body;
        for statement in body.statements().iter() {
            let StmtKind::Let {
                scope,
                annotation: Some(annotation),
                ..
            } = &statement.kind
            else {
                continue;
            };

            let context =
                TypeRefContext::new(*scope, self.body_visible_bindings, statement.source);
            self.emit_type_ref(context, annotation);
        }
    }

    fn walk_source_item_declarations(&mut self) {
        for (scope_idx, scope_data) in self.sites.body.scopes().iter().enumerate() {
            let scope = ScopeId(scope_idx);
            for item in &scope_data.source_items {
                self.walk_source_item_id_type_refs(scope, *item);
            }
        }
    }

    fn walk_expression_type_refs(&mut self) {
        let body = self.sites.body;
        for expr in body.exprs().iter() {
            match &expr.kind {
                ExprKind::Closure {
                    scope,
                    params,
                    ret_ty,
                    ..
                } => {
                    for param in params {
                        if let Some(annotation) = &param.annotation {
                            let context =
                                TypeRefContext::new(*scope, expr.visible_bindings, param.source);
                            self.emit_type_ref(context, annotation);
                        }
                    }

                    if let Some(ret_ty) = ret_ty {
                        let context =
                            TypeRefContext::new(*scope, expr.visible_bindings, expr.source);
                        self.emit_type_ref(context, ret_ty);
                    }
                }
                ExprKind::Cast { ty: Some(ty), .. } => {
                    let context =
                        TypeRefContext::new(expr.scope, expr.visible_bindings, expr.source);
                    self.emit_type_ref(context, ty);
                }
                ExprKind::Path { path } => {
                    self.walk_expr_body_path_type_refs(
                        path,
                        expr.scope,
                        expr.visible_bindings,
                        expr.source,
                    );
                }
                ExprKind::Record {
                    path: Some(path), ..
                } => {
                    self.walk_expr_body_path_type_refs(
                        path,
                        expr.scope,
                        expr.visible_bindings,
                        expr.source,
                    );
                }
                ExprKind::MethodCall { generic_args, .. } => {
                    let context =
                        TypeRefContext::new(expr.scope, expr.visible_bindings, expr.source);
                    walk_generic_args_type_refs(generic_args, &mut |ty| {
                        self.emit_type_ref(context, ty);
                    });
                }
                _ => {}
            }
        }
    }

    fn walk_pattern_path_type_refs(&mut self) {
        let sites = self.sites;
        sites.for_each_pattern_site(|site| {
            if !site.source.is_written() {
                return;
            }
            walk_pat(sites.body, site.scope, site.pat, &mut |walk_site| {
                if !walk_site.data.source.is_written() {
                    return;
                }
                if let Some(path) = walk_site.data.kind.path() {
                    self.walk_decl_body_path_type_refs(
                        path,
                        walk_site.scope,
                        walk_site.data.source,
                    );
                }
            });
        });
    }

    fn walk_source_item_id_type_refs(&mut self, scope: ScopeId, item_id: ItemTreeId) {
        let Some(item) = self.sites.body.source_item(item_id) else {
            return;
        };
        let Some(source) = self.sites.body.source_item_source(item_id) else {
            return;
        };

        let context = self.decl_context(scope, source);
        self.walk_source_item_type_refs(context, item);
    }

    fn walk_source_item_type_refs(&mut self, context: TypeRefContext, item: &'body ItemNode) {
        match &item.kind {
            ItemKind::Struct(item) => {
                self.walk_generic_params_type_refs(context, &item.generics);
                self.walk_field_list_type_refs(context, &item.fields);
            }
            ItemKind::Enum(item) => {
                self.walk_generic_params_type_refs(context, &item.generics);
                for variant in &item.variants {
                    self.walk_field_list_type_refs(context, &variant.fields);
                }
            }
            ItemKind::Union(item) => {
                self.walk_generic_params_type_refs(context, &item.generics);
                self.walk_field_type_refs(context, &item.fields);
            }
            ItemKind::TypeAlias(item) => {
                self.walk_generic_params_type_refs(context, &item.generics);
                self.walk_type_bounds_type_refs(context, &item.bounds);
                if let Some(ty) = &item.aliased_ty {
                    self.emit_type_ref(context, ty);
                }
            }
            ItemKind::Trait(item) => {
                self.walk_generic_params_type_refs(context, &item.generics);
                self.walk_type_bounds_type_refs(context, &item.super_traits);
                for assoc_item in &item.items {
                    self.walk_source_item_id_type_refs(context.scope, *assoc_item);
                }
            }
            ItemKind::Const(item) => {
                self.walk_generic_params_type_refs(context, &item.generics);
                if let Some(ty) = &item.ty {
                    self.emit_type_ref(context, ty);
                }
            }
            ItemKind::Static(item) => {
                if let Some(ty) = &item.ty {
                    self.emit_type_ref(context, ty);
                }
            }
            ItemKind::Function(item) => self.walk_function_item_type_refs(context, item),
            ItemKind::Impl(item) => self.walk_impl_item_type_refs(context, item),
            ItemKind::Module(item) => self.walk_module_item_type_refs(context, item),
            ItemKind::AsmExpr
            | ItemKind::ExternBlock
            | ItemKind::ExternCrate(_)
            | ItemKind::MacroCall(_)
            | ItemKind::MacroDefinition(_)
            | ItemKind::Use(_) => {}
        }
    }

    fn walk_impl_item_type_refs(&mut self, context: TypeRefContext, item: &'body ImplItem) {
        self.walk_generic_params_type_refs(context, &item.generics);
        if let Some(trait_ref) = &item.trait_ref {
            self.emit_type_ref(context, trait_ref);
        }
        self.emit_type_ref(context, &item.self_ty);
        for assoc_item in &item.items {
            self.walk_source_item_id_type_refs(context.scope, *assoc_item);
        }
    }

    fn walk_module_item_type_refs(&mut self, context: TypeRefContext, item: &'body ModuleItem) {
        let ModuleSource::Inline { items } = &item.source else {
            return;
        };
        for item in items {
            self.walk_source_item_id_type_refs(context.scope, *item);
        }
    }

    fn walk_function_item_type_refs(&mut self, context: TypeRefContext, item: &'body FunctionItem) {
        self.walk_generic_params_type_refs(context, &item.generics);
        for param in &item.params {
            if let Some(ty) = &param.ty {
                self.emit_type_ref(context, ty);
            }
        }
        if let Some(ty) = &item.ret_ty {
            self.emit_type_ref(context, ty);
        }
    }

    fn walk_generic_params_type_refs(
        &mut self,
        context: TypeRefContext,
        generics: &'body GenericParams,
    ) {
        for param in &generics.types {
            self.walk_type_bounds_type_refs(context, &param.bounds);
            if let Some(ty) = &param.default {
                self.emit_type_ref(context, ty);
            }
        }
        for param in &generics.consts {
            if let Some(ty) = &param.ty {
                self.emit_type_ref(context, ty);
            }
        }
        for predicate in &generics.where_predicates {
            match predicate {
                WherePredicate::Type { ty, bounds } => {
                    self.emit_type_ref(context, ty);
                    self.walk_type_bounds_type_refs(context, bounds);
                }
                WherePredicate::Lifetime { .. } | WherePredicate::Unsupported(_) => {}
            }
        }
    }

    fn walk_type_bounds_type_refs(&mut self, context: TypeRefContext, bounds: &'body [TypeBound]) {
        for bound in bounds {
            if let TypeBound::Trait(ty) = bound {
                self.emit_type_ref(context, ty);
            }
        }
    }

    fn walk_field_list_type_refs(&mut self, context: TypeRefContext, fields: &'body FieldList) {
        self.walk_field_type_refs(context, fields.fields());
    }

    fn walk_field_type_refs(&mut self, context: TypeRefContext, fields: &'body [FieldItem]) {
        for field in fields {
            self.emit_type_ref(context, &field.ty);
        }
    }

    fn walk_decl_body_path_type_refs(
        &mut self,
        path: &'body BodyPath,
        scope: ScopeId,
        source: BodySource,
    ) {
        let context = TypeRefContext::new(scope, self.body_visible_bindings, source);
        self.walk_body_path_type_refs(context, path);
    }

    fn walk_expr_body_path_type_refs(
        &mut self,
        path: &'body BodyPath,
        scope: ScopeId,
        visible_bindings: usize,
        source: BodySource,
    ) {
        // Type arguments inside a value path belong to a concrete expression, so they inherit that
        // expression's binding cutoff rather than the body-wide declaration cutoff.
        let context = TypeRefContext::new(scope, visible_bindings, source);
        self.walk_body_path_type_refs(context, path);
    }

    fn walk_body_path_type_refs(&mut self, context: TypeRefContext, path: &'body BodyPath) {
        walk_embedded_body_path_type_refs(path, &mut |ty| {
            self.emit_type_ref(context, ty);
        });
    }

    fn emit_type_ref(&mut self, context: TypeRefContext, ty: &'body TypeRef) {
        if !context.source.is_written() {
            return;
        }

        (self.visit)(TypeRefSite {
            scope: context.scope,
            visible_bindings: context.visible_bindings,
            source: context.source,
            ty,
        });
    }

    fn decl_context(&self, scope: ScopeId, source: BodySource) -> TypeRefContext {
        TypeRefContext::new(scope, self.body_visible_bindings, source)
    }
}
