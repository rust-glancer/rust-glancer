//! Semantic item traversal shared by signature source queries.
//!
//! The walker visits every item-owned place that can contain a declaration name or type path. It
//! does not decide what those source facts mean; the collector determines whether they become
//! indexed occurrences or a completion site.

use anyhow::Context as _;
use rg_def_map::ItemSourceKind;
use rg_ir_model::{
    ConstRef, DefMapRef, EnumVariantRef, FieldRef, FileId, FunctionRef, GenericDefRef, ImplRef,
    ItemOwner, SemanticItemRef, Span, StaticRef, TraitDefRef, TypeAliasRef, TypeDefId, TypeDefRef,
};
use rg_item_tree::{FieldList, GenericParams, TypeBound, TypeRef, WherePredicate};
use rg_semantic_ir::{
    ConstData, EnumData, FunctionData, ImplData, ItemStoreQuery, SemanticItemView, StaticData,
    StructData, TraitData, TypeAliasData, TypePathContext, UnionData,
};

use super::{SignatureSourceCandidate, SignatureTypePathScope, collector::SignatureScanCollector};
use crate::{
    IndexedViewDb,
    source::scan::{TypeNamePosition, type_path::walk_type_ref_paths},
};

/// Walks the enumerated signature-bearing declarations and sends source facts to `C`.
///
/// Each item first establishes the module and generic owner for its written paths. The walk then
/// descends through fields, parameters, bounds, defaults, and return types. It deliberately knows
/// nothing about cursor selection or occurrence filtering; those policies belong to the collector.
pub(super) struct SignatureItemWalker<'view, 'db, C> {
    db: &'view IndexedViewDb<'db>,
    items: &'view [SemanticItemView<'view>],
    origin: DefMapRef,
    collector: C,
}

impl<'view, 'db, C> SignatureItemWalker<'view, 'db, C>
where
    C: SignatureScanCollector,
{
    pub(super) fn new(
        db: &'view IndexedViewDb<'db>,
        items: &'view [SemanticItemView<'view>],
        origin: DefMapRef,
        collector: C,
    ) -> Self {
        Self {
            db,
            items,
            origin,
            collector,
        }
    }

    #[rg_std::cancelable("signature scan", token = self.db)]
    pub(super) fn scan(mut self) -> anyhow::Result<C> {
        self.scan_items().context("scan signature items")?;
        Ok(self.collector)
    }

    fn scan_items(&mut self) -> anyhow::Result<()> {
        for item in self.items {
            rg_std::check_cancel!(self.db, "signature item scan");
            if !self.file_matches(item.source().file_id) {
                continue;
            }
            match item.item() {
                SemanticItemRef::TypeDef(ty) => {
                    if let Some(data) = item.struct_data() {
                        self.scan_struct(ty, data)?;
                    } else if let Some(data) = item.union_data() {
                        self.scan_union(ty, data)?;
                    } else if let Some(data) = item.enum_data() {
                        self.scan_enum(ty, data)?;
                    }
                }
                SemanticItemRef::Trait(trait_ref) => self.scan_trait(
                    trait_ref,
                    item.trait_data().expect("item kind matches its data"),
                )?,
                SemanticItemRef::Impl(impl_ref) => self.scan_impl(
                    impl_ref,
                    item.impl_data().expect("item kind matches its data"),
                )?,
                SemanticItemRef::Function(function) => self.scan_function(
                    function,
                    item.function_data().expect("item kind matches its data"),
                )?,
                SemanticItemRef::TypeAlias(alias) => self.scan_type_alias(
                    alias,
                    item.type_alias_data().expect("item kind matches its data"),
                )?,
                SemanticItemRef::Const(konst) => self.scan_const(
                    konst,
                    item.const_data().expect("item kind matches its data"),
                )?,
                SemanticItemRef::Static(statik) => self.scan_static(
                    statik,
                    item.static_data().expect("item kind matches its data"),
                )?,
            }
        }
        Ok(())
    }

    fn scan_struct(&mut self, ty: TypeDefRef, data: &StructData) -> anyhow::Result<()> {
        let scope = SignatureTypePathScope {
            context: self.current_context(TypePathContext::module(data.owner))?,
            generic_owner: GenericDefRef::TypeDef(ty),
        };
        self.scan_generic_params(scope, &data.generics, data.source.file_id);
        self.scan_field_list(ty, scope, &data.fields, data.source.file_id);

        Ok(())
    }

    fn scan_union(&mut self, ty: TypeDefRef, data: &UnionData) -> anyhow::Result<()> {
        let scope = SignatureTypePathScope {
            context: self.current_context(TypePathContext::module(data.owner))?,
            generic_owner: GenericDefRef::TypeDef(ty),
        };
        self.scan_generic_params(scope, &data.generics, data.source.file_id);
        for (field_idx, field) in data.fields.iter().enumerate() {
            self.push_field(
                FieldRef {
                    owner: ty,
                    index: field_idx,
                },
                field.span,
            );
            self.push_type_ref(
                scope,
                &field.ty,
                data.source.file_id,
                TypeNamePosition::Type,
            );
        }

        Ok(())
    }

    fn scan_enum(&mut self, ty: TypeDefRef, data: &EnumData) -> anyhow::Result<()> {
        let origin = ty.origin;
        let scope = SignatureTypePathScope {
            context: self.current_context(TypePathContext::module(data.owner))?,
            generic_owner: GenericDefRef::TypeDef(ty),
        };
        self.scan_generic_params(scope, &data.generics, data.source.file_id);
        let TypeDefId::Enum(enum_id) = ty.id else {
            return Ok(());
        };
        for (variant_idx, variant) in data.variants.iter().enumerate() {
            self.push_enum_variant(
                EnumVariantRef {
                    origin,
                    enum_id,
                    index: variant_idx,
                },
                variant.name_span,
            );

            // Variant fields have no type-owned `FieldRef`, but paths in their annotations
            // still belong to the enum's signature and need to remain navigable.
            for field in variant.fields.fields() {
                self.push_type_ref(
                    scope,
                    &field.ty,
                    data.source.file_id,
                    TypeNamePosition::Type,
                );
            }
        }

        Ok(())
    }

    fn scan_trait(&mut self, trait_ref: TraitDefRef, data: &TraitData) -> anyhow::Result<()> {
        let scope = SignatureTypePathScope {
            context: self.current_context(TypePathContext::module(data.owner))?,
            generic_owner: GenericDefRef::Trait(trait_ref),
        };
        self.scan_generic_params(scope, &data.generics, data.source.file_id);
        self.scan_type_bounds(scope, &data.super_traits, data.source.file_id);

        Ok(())
    }

    fn scan_impl(&mut self, impl_ref: ImplRef, data: &ImplData) -> anyhow::Result<()> {
        // A synthesized impl participates in type analysis, but its paths were never written.
        // For `#[derive(Clone)] struct Foo;`, its `Foo` path must not turn `Clone` into a
        // renameable occurrence of the struct. Copied generic bounds are scanned on the type.
        if matches!(data.source.kind, ItemSourceKind::Synthetic(_)) {
            return Ok(());
        }
        let Some(context) = self.owner_context(ItemOwner::Impl(impl_ref.id))? else {
            return Ok(());
        };
        let scope = SignatureTypePathScope {
            context,
            generic_owner: GenericDefRef::Impl(impl_ref),
        };
        self.scan_generic_params(scope, &data.generics, data.source.file_id);

        // The impl's generic parameters are in scope while its trait and self type are being
        // written, but `Self` does not exist until that self type has been established.
        // Associated items keep the full impl context through `owner_context`; only these two
        // paths deliberately start from the containing module.
        let header_scope = SignatureTypePathScope {
            context: TypePathContext::module(scope.context.module),
            ..scope
        };
        if let Some(trait_ref) = &data.trait_ref {
            self.push_type_ref(
                header_scope,
                trait_ref,
                data.source.file_id,
                TypeNamePosition::Type,
            );
        }
        self.push_type_ref(
            header_scope,
            &data.self_ty,
            data.source.file_id,
            TypeNamePosition::Type,
        );

        Ok(())
    }

    fn scan_function(
        &mut self,
        function_ref: FunctionRef,
        data: &FunctionData,
    ) -> anyhow::Result<()> {
        if data.local_def.is_none() {
            let span = data.name_span.unwrap_or(data.span);
            self.push_function(function_ref, span);
        }
        let Some(context) = self.owner_context(data.owner)? else {
            return Ok(());
        };
        let scope = SignatureTypePathScope {
            context,
            generic_owner: GenericDefRef::Function(function_ref),
        };
        if let Some(generics) = data.signature.generics() {
            self.scan_generic_params(scope, generics, data.source.file_id);
        }
        for param in data.signature.params() {
            if let Some(ty) = &param.ty {
                self.push_type_ref(scope, ty, data.source.file_id, TypeNamePosition::Type);
            }
        }
        if let Some(ret_ty) = data.signature.ret_ty() {
            self.push_type_ref(scope, ret_ty, data.source.file_id, TypeNamePosition::Type);
        }

        Ok(())
    }

    fn scan_type_alias(&mut self, alias: TypeAliasRef, data: &TypeAliasData) -> anyhow::Result<()> {
        let Some(context) = self.owner_context(data.owner)? else {
            return Ok(());
        };
        let scope = SignatureTypePathScope {
            context,
            generic_owner: GenericDefRef::TypeAlias(alias),
        };
        if let Some(generics) = data.signature.generics() {
            self.scan_generic_params(scope, generics, data.source.file_id);
        }
        self.scan_type_bounds(scope, data.signature.bounds(), data.source.file_id);
        if let Some(ty) = data.signature.aliased_ty() {
            self.push_type_ref(scope, ty, data.source.file_id, TypeNamePosition::Type);
        }

        Ok(())
    }

    fn scan_const(&mut self, konst: ConstRef, data: &ConstData) -> anyhow::Result<()> {
        let Some(context) = self.owner_context(data.owner)? else {
            return Ok(());
        };
        let scope = SignatureTypePathScope {
            context,
            generic_owner: GenericDefRef::Const(konst),
        };
        if let Some(ty) = data.signature.ty() {
            self.push_type_ref(scope, ty, data.source.file_id, TypeNamePosition::Type);
        }

        Ok(())
    }

    fn scan_static(&mut self, static_ref: StaticRef, data: &StaticData) -> anyhow::Result<()> {
        if let Some(ty) = &data.ty {
            self.push_type_ref(
                SignatureTypePathScope {
                    context: self.current_context(TypePathContext::module(data.owner))?,
                    generic_owner: GenericDefRef::Static(static_ref),
                },
                ty,
                data.source.file_id,
                TypeNamePosition::Type,
            );
        }

        Ok(())
    }

    fn scan_field_list(
        &mut self,
        owner: TypeDefRef,
        scope: SignatureTypePathScope,
        fields: &FieldList,
        file_id: FileId,
    ) {
        for (idx, field) in fields.fields().iter().enumerate() {
            self.push_field(FieldRef { owner, index: idx }, field.span);
            self.push_type_ref(scope, &field.ty, file_id, TypeNamePosition::Type);
        }
    }

    /// Visits type syntax attached to generic declarations, not just the parameter names.
    ///
    /// For `<T: Trait = Default, const N: usize> where T: Other`, the walk reports the paths
    /// `Trait`, `Default`, `usize`, `T`, and `Other`. Lifetimes have no nested type paths to report.
    fn scan_generic_params(
        &mut self,
        scope: SignatureTypePathScope,
        generics: &GenericParams,
        file_id: FileId,
    ) {
        for param in generics.types() {
            self.scan_type_bounds(scope, &param.bounds, file_id);
            if let Some(default) = &param.default {
                self.push_type_ref(scope, default, file_id, TypeNamePosition::Type);
            }
        }
        for param in generics.consts() {
            if let Some(ty) = &param.ty {
                self.push_type_ref(scope, ty, file_id, TypeNamePosition::Type);
            }
        }
        for predicate in &generics.where_predicates {
            match predicate {
                WherePredicate::Type { ty, bounds } => {
                    self.push_type_ref(scope, ty, file_id, TypeNamePosition::Type);
                    self.scan_type_bounds(scope, bounds, file_id);
                }
                WherePredicate::Lifetime { .. } | WherePredicate::Unsupported(_) => {}
            }
        }
    }

    fn scan_type_bounds(
        &mut self,
        scope: SignatureTypePathScope,
        bounds: &[TypeBound],
        file_id: FileId,
    ) {
        for bound in bounds {
            if let Some(ty) = bound.trait_ty() {
                self.push_type_ref(scope, ty, file_id, TypeNamePosition::Type);
            }
        }
    }

    /// Sends every nested path in one type reference through the selected output policy.
    fn push_type_ref(
        &mut self,
        scope: SignatureTypePathScope,
        ty: &TypeRef,
        file_id: FileId,
        position: TypeNamePosition,
    ) {
        let collector = &mut self.collector;
        walk_type_ref_paths(ty, position, &mut |path, position| {
            collector.push_type_path(scope, path, file_id, position);
        });
    }

    fn push_field(&mut self, field: FieldRef, span: Span) {
        self.collector
            .push_candidate(SignatureSourceCandidate::Field { field, span });
    }

    fn push_function(&mut self, function: FunctionRef, span: Span) {
        self.collector
            .push_candidate(SignatureSourceCandidate::Function { function, span });
    }

    fn push_enum_variant(&mut self, variant: EnumVariantRef, span: Span) {
        self.collector
            .push_candidate(SignatureSourceCandidate::EnumVariant { variant, span });
    }

    fn owner_context(&self, owner: ItemOwner) -> anyhow::Result<Option<TypePathContext>> {
        ItemStoreQuery::new(self.db)
            .type_path_context_for_owner(self.origin, owner)?
            .map(|context| self.current_context(context))
            .transpose()
    }

    fn current_context(&self, context: TypePathContext) -> anyhow::Result<TypePathContext> {
        self.db
            .current_signature_context(context)
            .context("resolve current signature context")
    }

    fn file_matches(&self, file_id: FileId) -> bool {
        self.collector
            .selected_file()
            .is_none_or(|selected| selected == file_id)
    }
}
