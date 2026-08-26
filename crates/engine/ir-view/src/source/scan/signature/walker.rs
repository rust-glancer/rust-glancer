//! Semantic item traversal shared by signature source queries.
//!
//! The walker visits every item-owned place that can contain a declaration name or type path. It
//! does not decide what those source facts mean; the collector determines whether they become
//! indexed occurrences or a completion site.

use rg_ir_model::{
    ConstRef, DefMapRef, EnumVariantRef, FieldRef, FunctionRef, GenericDefRef, ItemOwner,
    StaticRef, TypeAliasRef, TypeDefId, TypeDefRef,
};
use rg_item_tree::{FieldList, GenericParams, TypeBound, TypeRef, WherePredicate};
use rg_package_store::PackageStoreError;
use rg_parse::{FileId, Span};
use rg_semantic_ir::{ItemStore, ItemStoreQuery, TypePathContext};

use super::{SignatureSourceCandidate, SignatureTypePathScope, collector::SignatureScanCollector};
use crate::IndexedViewDb;
use crate::source::scan::{TypeNamePosition, type_path::walk_type_ref_paths};

/// Walks every signature-bearing item in one semantic store and sends source facts to `C`.
///
/// Each item first establishes the module and generic owner for its written paths. The walk then
/// descends through fields, parameters, bounds, defaults, and return types. It deliberately knows
/// nothing about cursor selection or occurrence filtering; those policies belong to the collector.
pub(super) struct SignatureItemWalker<'view, 'db, C> {
    db: &'view IndexedViewDb<'db>,
    items: &'view ItemStore,
    origin: DefMapRef,
    collector: C,
}

impl<'view, 'db, C> SignatureItemWalker<'view, 'db, C>
where
    C: SignatureScanCollector,
{
    pub(super) fn new(
        db: &'view IndexedViewDb<'db>,
        items: &'view ItemStore,
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

    pub(super) fn scan(mut self) -> Result<C, PackageStoreError> {
        self.scan_items()?;
        Ok(self.collector)
    }

    fn scan_items(&mut self) -> Result<(), PackageStoreError> {
        self.scan_structs()?;
        self.scan_unions()?;
        self.scan_enums()?;
        self.scan_traits()?;
        self.scan_impls()?;
        self.scan_functions()?;
        self.scan_type_aliases()?;
        self.scan_consts()?;
        self.scan_statics()?;
        Ok(())
    }

    fn scan_structs(&mut self) -> Result<(), PackageStoreError> {
        let origin = self.origin;
        for (id, data) in self.items.structs().iter_with_ids() {
            if !self.file_matches(data.source.file_id) {
                continue;
            }
            let ty = TypeDefRef {
                origin,
                id: TypeDefId::Struct(id),
            };
            let scope = SignatureTypePathScope {
                context: self.current_context(TypePathContext::module(data.owner))?,
                generic_owner: GenericDefRef::TypeDef(ty),
            };
            self.scan_generic_params(scope, &data.generics, data.source.file_id);
            self.scan_field_list(ty, scope, &data.fields, data.source.file_id);
        }

        Ok(())
    }

    fn scan_unions(&mut self) -> Result<(), PackageStoreError> {
        let origin = self.origin;
        for (id, data) in self.items.unions().iter_with_ids() {
            if !self.file_matches(data.source.file_id) {
                continue;
            }
            let ty = TypeDefRef {
                origin,
                id: TypeDefId::Union(id),
            };
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
        }

        Ok(())
    }

    fn scan_enums(&mut self) -> Result<(), PackageStoreError> {
        let origin = self.origin;
        for (id, data) in self.items.enums().iter_with_ids() {
            if !self.file_matches(data.source.file_id) {
                continue;
            }
            let ty = TypeDefRef {
                origin,
                id: TypeDefId::Enum(id),
            };
            let scope = SignatureTypePathScope {
                context: self.current_context(TypePathContext::module(data.owner))?,
                generic_owner: GenericDefRef::TypeDef(ty),
            };
            self.scan_generic_params(scope, &data.generics, data.source.file_id);
            let TypeDefId::Enum(enum_id) = ty.id else {
                continue;
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
        }

        Ok(())
    }

    fn scan_traits(&mut self) -> Result<(), PackageStoreError> {
        for (trait_ref, data) in self.items.traits_with_refs() {
            if !self.file_matches(data.source.file_id) {
                continue;
            }
            let scope = SignatureTypePathScope {
                context: self.current_context(TypePathContext::module(data.owner))?,
                generic_owner: GenericDefRef::Trait(trait_ref),
            };
            self.scan_generic_params(scope, &data.generics, data.source.file_id);
            self.scan_type_bounds(scope, &data.super_traits, data.source.file_id);
        }

        Ok(())
    }

    fn scan_impls(&mut self) -> Result<(), PackageStoreError> {
        for (impl_ref, data) in self.items.impls_with_refs() {
            if !self.file_matches(data.source.file_id) {
                continue;
            }
            let Some(context) = self.owner_context(ItemOwner::Impl(impl_ref.id))? else {
                continue;
            };
            let scope = SignatureTypePathScope {
                context,
                generic_owner: GenericDefRef::Impl(impl_ref),
            };
            self.scan_generic_params(scope, &data.generics, data.source.file_id);
            if let Some(trait_ref) = &data.trait_ref {
                self.push_type_ref(
                    scope,
                    trait_ref,
                    data.source.file_id,
                    TypeNamePosition::Type,
                );
            }
            self.push_type_ref(
                scope,
                &data.self_ty,
                data.source.file_id,
                TypeNamePosition::Type,
            );
        }

        Ok(())
    }

    fn scan_functions(&mut self) -> Result<(), PackageStoreError> {
        for (function_ref, data) in self.items.functions_with_refs() {
            if !self.file_matches(data.source.file_id) {
                continue;
            }
            if data.local_def.is_none() {
                let span = data.name_span.unwrap_or(data.span);
                self.push_function(function_ref, span);
            }
            let Some(context) = self.owner_context(data.owner)? else {
                continue;
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
        }

        Ok(())
    }

    fn scan_type_aliases(&mut self) -> Result<(), PackageStoreError> {
        let origin = self.origin;
        for (id, data) in self.items.type_aliases().iter_with_ids() {
            if !self.file_matches(data.source.file_id) {
                continue;
            }
            let Some(context) = self.owner_context(data.owner)? else {
                continue;
            };
            let scope = SignatureTypePathScope {
                context,
                generic_owner: GenericDefRef::TypeAlias(TypeAliasRef { origin, id }),
            };
            if let Some(generics) = data.signature.generics() {
                self.scan_generic_params(scope, generics, data.source.file_id);
            }
            self.scan_type_bounds(scope, data.signature.bounds(), data.source.file_id);
            if let Some(ty) = data.signature.aliased_ty() {
                self.push_type_ref(scope, ty, data.source.file_id, TypeNamePosition::Type);
            }
        }

        Ok(())
    }

    fn scan_consts(&mut self) -> Result<(), PackageStoreError> {
        let origin = self.origin;
        for (id, data) in self.items.consts().iter_with_ids() {
            if !self.file_matches(data.source.file_id) {
                continue;
            }
            let Some(context) = self.owner_context(data.owner)? else {
                continue;
            };
            let scope = SignatureTypePathScope {
                context,
                generic_owner: GenericDefRef::Const(ConstRef { origin, id }),
            };
            if let Some(ty) = data.signature.ty() {
                self.push_type_ref(scope, ty, data.source.file_id, TypeNamePosition::Type);
            }
        }

        Ok(())
    }

    fn scan_statics(&mut self) -> Result<(), PackageStoreError> {
        let origin = self.origin;
        for (id, data) in self.items.statics().iter_with_ids() {
            if !self.file_matches(data.source.file_id) {
                continue;
            }
            if let Some(ty) = &data.ty {
                self.push_type_ref(
                    SignatureTypePathScope {
                        context: self.current_context(TypePathContext::module(data.owner))?,
                        generic_owner: GenericDefRef::Static(StaticRef { origin, id }),
                    },
                    ty,
                    data.source.file_id,
                    TypeNamePosition::Type,
                );
            }
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

    fn owner_context(
        &self,
        owner: ItemOwner,
    ) -> Result<Option<TypePathContext>, PackageStoreError> {
        ItemStoreQuery::new(self.db)
            .type_path_context_for_owner(self.origin, owner)?
            .map(|context| self.current_context(context))
            .transpose()
    }

    fn current_context(
        &self,
        context: TypePathContext,
    ) -> Result<TypePathContext, PackageStoreError> {
        self.db.current_signature_context(context)
    }

    fn file_matches(&self, file_id: FileId) -> bool {
        self.collector
            .selected_file()
            .is_none_or(|selected| selected == file_id)
    }
}
