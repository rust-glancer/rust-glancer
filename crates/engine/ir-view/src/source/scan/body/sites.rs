//! Shared source-site discovery for body source scanners.
//!
//! Source queries interpret the same lowered syntax in several different ways: one scanner wants
//! navigable value paths, another wants completion owners, and another wants record field lists.
//! This module keeps the structural walk in one place while leaving those query-specific meanings
//! with the callers. Pattern walking includes function parameters, and type sites retain a
//! body-local declaration owner when its generics differ from the enclosing body.
//!
//! In particular, type syntax is not limited to a `let` annotation. The same path walk must find
//! all of these spellings while retaining the scope that owns them:
//!
//! ```text
//! let value: Option<Item> = input;
//! let closure = |item: Item| -> Output { item };
//! value.cast::<Target>()
//! fn local<T>(value: T) -> Option<T> { ... }
//! ```

use rg_body_ir::{BodyPath, BodyView, ExprData, ExprKind, PatKind, StmtKind};
use rg_ir_model::{BodyRef, BodySource, ExprId, FileId, PatId, ScopeId, SourceId};
use rg_item_tree::{
    FieldItem, FieldList, FunctionItem, GenericParams, ImplItem, ItemKind, ItemNode, ItemTreeId,
    ModuleItem, ModuleSource, TypeBound, TypePath, TypeRef, WherePredicate,
};

use super::{
    BodySourceCandidate, RecordFieldKeySurface,
    walk::{
        PatWalkSite, walk_body_path_type_refs as walk_embedded_body_path_type_refs,
        walk_generic_args_type_refs, walk_pat,
    },
};
use crate::source::scan::{TypeNamePosition, type_path::walk_type_ref_paths};

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
    /// Declaration whose generic scope owns this type reference.
    ///
    /// Ordinary body annotations have no declaration owner and use the enclosing body's generic
    /// scope. A type written in a body-local item signature must instead resolve parameters from
    /// that item, even though its source payload is retained by the parent body.
    owner_item: Option<ItemTreeId>,
    ty: &'body TypeRef,
}

/// Scope metadata carried by one written type reference.
#[derive(Debug, Clone, Copy)]
struct TypeRefContext {
    scope: ScopeId,
    visible_bindings: usize,
    source: BodySource,
    owner_item: Option<ItemTreeId>,
}

impl TypeRefContext {
    fn new(scope: ScopeId, visible_bindings: usize, source: BodySource) -> Self {
        Self {
            scope,
            visible_bindings,
            source,
            owner_item: None,
        }
    }

    fn for_item(
        scope: ScopeId,
        visible_bindings: usize,
        source: BodySource,
        owner_item: ItemTreeId,
    ) -> Self {
        Self {
            scope,
            visible_bindings,
            source,
            owner_item: Some(owner_item),
        }
    }
}

/// One type path reached from a body-local type reference.
#[derive(Debug, Clone, Copy)]
pub(super) struct TypePathSite<'body> {
    pub(super) scope: ScopeId,
    pub(super) visible_bindings: usize,
    pub(super) source: BodySource,
    pub(super) owner_item: Option<ItemTreeId>,
    pub(super) path: &'body TypePath,
    /// Whether this path may name a const because it fills one whole generic argument.
    pub(super) position: TypeNamePosition,
}

/// Structural views over lowered body syntax used by source scans.
///
/// Shares syntax traversal and exact name selection. Each query decides which occurrences to
/// keep, such as preferring the binding at a handwritten record pattern's shorthand field.
pub(super) struct BodyScanSites<'body> {
    body: BodyView<'body>,
    include_expansions: bool,
}

impl<'body> BodyScanSites<'body> {
    pub(super) fn written(body: BodyView<'body>) -> Self {
        Self {
            body,
            include_expansions: false,
        }
    }

    /// Also walk generated syntax roots. A generated root can mix copied names with
    /// template tokens, so the caller still checks each name's location.
    pub(super) fn including_expansions(body: BodyView<'body>) -> Self {
        Self {
            body,
            include_expansions: true,
        }
    }

    fn includes(&self, source: BodySource, file_id: Option<FileId>) -> bool {
        file_id.is_none_or(|file| source.file_id == file)
            && (source.is_written() || self.include_expansions)
    }

    /// The name may come from the invocation even when the enclosing expression came from
    /// the macro template, as in `($method:ident) => { receiver.$method() }`.
    pub(super) fn expression_name(expr: &ExprData) -> Option<SourceId> {
        let name = match &expr.kind {
            ExprKind::Path { path }
            | ExprKind::Record {
                path: Some(path), ..
            } => path.last_segment_location(),
            ExprKind::MethodCall {
                method_name_location,
                ..
            } => *method_name_location,
            ExprKind::Field { field_location, .. } => *field_location,
            _ => return None,
        };
        Some(name.unwrap_or(expr.source))
    }

    pub(super) fn walk_record_expr_keys(
        &self,
        body: BodyRef,
        expr: &ExprData,
        offset: Option<u32>,
        mut visit: impl FnMut(BodySourceCandidate),
    ) {
        let ExprKind::Record {
            path: Some(owner),
            fields,
            ..
        } = &expr.kind
        else {
            return;
        };
        let Some(owner) = owner.as_def_map_path() else {
            return;
        };
        for field in fields {
            let key = self.body.source_at(field.key_location);
            let location = key.location();
            if !location.is_exact() || offset.is_some_and(|offset| !key.span.touches(offset)) {
                continue;
            }
            visit(BodySourceCandidate::RecordFieldKey {
                body,
                scope: expr.scope,
                name: field.key_location,
                owner: owner.clone(),
                key: field.key.clone(),
                file_id: key.file_id,
                location,
                surface: if field.syntax.is_explicit() {
                    RecordFieldKeySurface::Explicit
                } else {
                    RecordFieldKeySurface::RecordExprShorthand {
                        field_span: self.body.span(field.location),
                    }
                },
            });
        }
    }

    pub(super) fn walk_record_pat_keys(
        &self,
        body: BodyRef,
        file_id: Option<FileId>,
        offset: Option<u32>,
        mut visit: impl FnMut(BodySourceCandidate),
    ) {
        self.walk_pats(file_id, offset, |site| {
            let PatKind::Record {
                path: Some(owner),
                fields,
                ..
            } = &site.data.kind
            else {
                return;
            };
            let Some(owner) = owner.as_def_map_path() else {
                return;
            };
            for field in fields {
                let key = self.body.source_at(field.key_location);
                let location = key.location();
                if !location.is_exact() || offset.is_some_and(|offset| !key.span.touches(offset)) {
                    continue;
                }
                visit(BodySourceCandidate::RecordFieldKey {
                    body,
                    scope: site.scope,
                    name: field.key_location,
                    owner: owner.clone(),
                    key: field.key.clone(),
                    file_id: key.file_id,
                    location,
                    surface: if field.syntax.is_explicit() {
                        RecordFieldKeySurface::Explicit
                    } else {
                        let field_span = self.body.span(field.location);
                        let pat_span = self
                            .body
                            .pat(field.pat)
                            .map_or(field_span, |pat| self.body.span(pat.source));
                        RecordFieldKeySurface::RecordPatShorthand {
                            field_span,
                            pat_span,
                        }
                    },
                });
            }
        });
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
            if !self.includes(site.source, file_id) {
                return;
            }
            if offset.is_some_and(|offset| !site.source.span.touches(offset)) {
                return;
            }

            walk_pat(self.body, site.scope, site.pat, &mut |walk_site| {
                if self.includes(self.body.source_at(walk_site.data.source), file_id) {
                    visit(walk_site);
                }
            });
        });
    }

    /// Walks type paths that belong to body-local type syntax.
    ///
    /// Type references can hide paths inside tuples, pointers, function pointers, and generic
    /// arguments. Callers receive every nested path with the body scope that owns the annotation.
    /// They also receive the generic-argument distinction needed to treat `Array<N$0>` differently
    /// from an ordinary type spelling such as `let value: N$0`.
    pub(super) fn walk_type_paths(
        &self,
        file_id: Option<FileId>,
        mut visit: impl FnMut(TypePathSite<'body>),
    ) {
        self.for_each_type_ref_site(|site| {
            if !self.includes(site.source, file_id) {
                return;
            }

            walk_type_ref_paths(site.ty, TypeNamePosition::Type, &mut |path, position| {
                visit(TypePathSite {
                    scope: site.scope,
                    visible_bindings: site.visible_bindings,
                    source: site.source,
                    owner_item: site.owner_item,
                    path,
                    position,
                });
            });
        });
    }

    /// Returns expression ids used as the implicit values of record-expression shorthand fields.
    ///
    /// Those expressions are represented again as source-level shorthand occurrences, so ordinary
    /// expression scanning skips them to avoid exposing the same token through two surfaces.
    pub(super) fn record_expr_shorthand_value_ids(&self) -> Vec<ExprId> {
        let mut values = Vec::new();
        for expr in self.body.exprs() {
            let ExprKind::Record { fields, .. } = &expr.kind else {
                continue;
            };
            for field in fields {
                if !field.syntax.is_explicit()
                    && let Some(value) = field.value
                {
                    values.push(value);
                }
            }
        }
        values
    }

    fn for_each_pattern_site(&self, mut visit: impl FnMut(PatternSite)) {
        // Function parameter patterns are stored outside the expression arena, but they use the
        // body's parameter scope and participate in the same pattern completion rules.
        for param in self.body.function_params() {
            if let Some(pat) = param.pat {
                self.visit_pattern_site(&mut visit, self.body.param_scope(), pat);
            }
        }

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
            source: self.body.source_at(data.source),
        });
    }
}

/// Collects written type references while keeping scope policy close to each source form.
///
/// Besides ordinary annotations, bodies can contain type references in closures, casts,
/// turbofish arguments, pattern paths, and body-local item signatures. The walker carries the
/// lexical scope and source location of each spelling so downstream point queries do not have to
/// reconstruct that context from the finalized arenas.
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

            let context = TypeRefContext::new(
                *scope,
                self.body_visible_bindings,
                body.source_at(statement.source),
            );
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
                            let context = TypeRefContext::new(
                                *scope,
                                expr.visible_bindings,
                                body.source_at(param.source),
                            );
                            self.emit_type_ref(context, annotation);
                        }
                    }

                    if let Some(ret_ty) = ret_ty {
                        let context = TypeRefContext::new(
                            *scope,
                            expr.visible_bindings,
                            body.source_at(expr.source),
                        );
                        self.emit_type_ref(context, ret_ty);
                    }
                }
                ExprKind::Cast { ty: Some(ty), .. } => {
                    let context = TypeRefContext::new(
                        expr.scope,
                        expr.visible_bindings,
                        body.source_at(expr.source),
                    );
                    self.emit_type_ref(context, ty);
                }
                ExprKind::Path { path } => {
                    self.walk_expr_body_path_type_refs(
                        path,
                        expr.scope,
                        expr.visible_bindings,
                        body.source_at(expr.source),
                    );
                }
                ExprKind::Record {
                    path: Some(path), ..
                } => {
                    self.walk_expr_body_path_type_refs(
                        path,
                        expr.scope,
                        expr.visible_bindings,
                        body.source_at(expr.source),
                    );
                }
                ExprKind::MethodCall { generic_args, .. } => {
                    let context = TypeRefContext::new(
                        expr.scope,
                        expr.visible_bindings,
                        body.source_at(expr.source),
                    );
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
            walk_pat(sites.body, site.scope, site.pat, &mut |walk_site| {
                if let Some(path) = walk_site.data.kind.path() {
                    self.walk_decl_body_path_type_refs(
                        path,
                        walk_site.scope,
                        sites.body.source_at(walk_site.data.source),
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

        let context = self.decl_context(scope, source, item_id);
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
            ItemKind::Function(item) => self.walk_fn_def_type_refs(context, item),
            ItemKind::Impl(item) => self.walk_impl_item_type_refs(context, item),
            ItemKind::Module(item) => self.walk_module_item_type_refs(context, item),
            ItemKind::ExternBlock(item) => {
                for child in &item.items {
                    self.walk_source_item_id_type_refs(context.scope, *child);
                }
            }
            ItemKind::AsmExpr
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

    fn walk_fn_def_type_refs(&mut self, context: TypeRefContext, item: &'body FunctionItem) {
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
        for param in generics.types() {
            self.walk_type_bounds_type_refs(context, &param.bounds);
            if let Some(ty) = &param.default {
                self.emit_type_ref(context, ty);
            }
        }
        for param in generics.consts() {
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
            if let Some(ty) = bound.trait_ty() {
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
        (self.visit)(TypeRefSite {
            scope: context.scope,
            visible_bindings: context.visible_bindings,
            source: context.source,
            owner_item: context.owner_item,
            ty,
        });
    }

    fn decl_context(
        &self,
        scope: ScopeId,
        source: BodySource,
        owner_item: ItemTreeId,
    ) -> TypeRefContext {
        TypeRefContext::for_item(scope, self.body_visible_bindings, source, owner_item)
    }
}
