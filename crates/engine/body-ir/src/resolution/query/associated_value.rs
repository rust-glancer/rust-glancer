//! Read-only associated-value path resolution for Body IR.
//!
//! This query owns `Type::value` lookup: enum variants, inherent associated consts, trait
//! associated consts, and static associated functions. Ordinary lexical/module value paths stay in
//! `BodyValuePathQuery`.

use rg_ir_model::{
    AssocItemId, ConstRef, DefMapRef, ImplRef, ItemOwner, Path, ScopeId, TraitImplRef, TypeDefId,
    identity::DeclarationRef,
};
use rg_ir_storage::{DefMapSource, ItemStoreSource, TypePathContext};
use rg_package_store::PackageStoreError;
use rg_std::UniqueVec;
use rg_ty::{NominalTy, Ty, TypeSubst};

use crate::{
    ir::resolved::BodyResolution,
    resolution::{BodyResolutionContext, TypeRefUseSite, support::unique_ty_or_unknown},
};

pub(crate) struct BodyAssociatedValueQuery<'query, D, I> {
    context: BodyResolutionContext<'query, D, I>,
}

impl<'query, D, I> BodyAssociatedValueQuery<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    pub(crate) fn new(context: BodyResolutionContext<'query, D, I>) -> Self {
        Self { context }
    }

    pub(crate) fn resolve_path(
        &self,
        scope: ScopeId,
        prefix: &Path,
        last_segment: &str,
    ) -> Result<Option<(BodyResolution, Ty)>, PackageStoreError> {
        // Associated value paths are resolved as "type prefix + value member". This keeps
        // `Action::Start` distinct from a module path while also handling `Widget::new` through
        // the same type-substitution rules used by method calls.
        let prefix_resolution = self
            .context
            .type_path_query()
            .resolve_in_scope(scope, prefix)?;
        let prefix_ty =
            Ty::from_type_path_resolution(prefix_resolution, Vec::new()).unwrap_or(Ty::Unknown);

        // First treat the final segment as an enum variant. Variants are not ordinary associated
        // functions in either Semantic IR or Body IR, but value paths use the same syntax for
        // `Action::Start` and `Widget::new`, so they need an explicit pass.
        let mut variants = UniqueVec::new();
        let mut variant_tys = UniqueVec::new();
        for nominal_ty in prefix_ty.as_nominals() {
            if !matches!(nominal_ty.def.id, TypeDefId::Enum(_)) {
                continue;
            }
            let Some(variant_ref) = self
                .context
                .item_query()
                .enum_variant_ref_for_type_def(nominal_ty.def, last_segment)?
            else {
                continue;
            };
            variants.push(variant_ref);
            variant_tys.push(Ty::nominal(vec![nominal_ty.clone()]));
        }

        if !variants.is_empty() {
            let ty = unique_ty_or_unknown(variant_tys);
            return Ok(Some((
                BodyResolution::Declarations(
                    variants.into_iter().map(DeclarationRef::from).collect(),
                ),
                ty,
            )));
        }

        for nominal_ty in prefix_ty.as_nominals() {
            if let Some((const_ref, ty)) =
                self.semantic_associated_value_item_for_type(nominal_ty, last_segment)?
            {
                return Ok(Some((
                    BodyResolution::Declarations(vec![const_ref.into()]),
                    ty,
                )));
            }
        }

        // Trait associated const lookup is still receiver-driven: `Type::CONST` first proves that
        // `Type` has a visible trait impl, then reads the matching const item from that impl or
        // falls back to the trait declaration. This mirrors trait method lookup without pretending
        // to be a full trait solver.
        let mut trait_consts = UniqueVec::new();
        let mut trait_const_tys = UniqueVec::new();
        for nominal_ty in prefix_ty.as_nominals() {
            for (const_ref, ty) in
                self.semantic_associated_trait_value_items_for_type(nominal_ty, last_segment)?
            {
                trait_consts.push(const_ref);
                trait_const_tys.push(ty);
            }
        }

        if !trait_consts.is_empty() {
            return Ok(Some((
                BodyResolution::Declarations(
                    trait_consts.into_iter().map(DeclarationRef::from).collect(),
                ),
                unique_ty_or_unknown(trait_const_tys),
            )));
        }

        // Inherent associated functions are exact candidates. Trait-associated functions are kept
        // deliberately optimistic, following the same "prefer useful candidates over false
        // negatives" policy as dot completion.
        let mut functions = UniqueVec::new();
        let item_query = self.context.item_query();
        for nominal_ty in prefix_ty.as_nominals() {
            for function_ref in self
                .context
                .receiver_functions()
                .function_refs_for_receiver(nominal_ty, Some(last_segment))?
            {
                let Some(function_data) = item_query.function_data(function_ref)? else {
                    continue;
                };
                if function_data.name == last_segment && !function_data.has_self_receiver() {
                    functions.push(function_ref);
                }
            }
        }

        Ok((!functions.is_empty()).then_some((
            BodyResolution::Declarations(functions.into_iter().map(DeclarationRef::from).collect()),
            Ty::Unknown,
        )))
    }

    fn semantic_associated_value_item_for_type(
        &self,
        ty: &NominalTy,
        name: &str,
    ) -> Result<Option<(ConstRef, Ty)>, PackageStoreError> {
        if let Some(item) = self.associated_value_item_for_impls(
            self.context
                .body_local_items()
                .inherent_impls_for_type(ty.def)?,
            ty,
            name,
        )? {
            return Ok(Some(item));
        }

        if ty.def.origin == DefMapRef::Body(self.context.body_ref()) {
            return Ok(None);
        }

        self.associated_value_item_for_impls(
            self.context
                .target_items()
                .inherent_impls_for_type(ty.def)?,
            ty,
            name,
        )
    }

    fn semantic_associated_trait_value_items_for_type(
        &self,
        ty: &NominalTy,
        name: &str,
    ) -> Result<Vec<(ConstRef, Ty)>, PackageStoreError> {
        let mut items = Vec::new();

        self.push_associated_trait_value_items_for_impls(
            &mut items,
            self.context
                .body_local_items()
                .trait_impls_for_type(ty.def)?,
            ty,
            name,
        )?;

        if ty.def.origin == DefMapRef::Body(self.context.body_ref()) {
            return Ok(items);
        }

        let target_items = self.context.target_items();
        let semantic_trait_impls = match self.context.semantic_index() {
            Some(index) => index.trait_impls_for_type(ty.def).to_vec(),
            None => target_items.trait_impls_for_type(ty.def)?,
        };
        self.push_associated_trait_value_items_for_impls(
            &mut items,
            semantic_trait_impls,
            ty,
            name,
        )?;

        Ok(items)
    }

    fn push_associated_trait_value_items_for_impls(
        &self,
        items: &mut Vec<(ConstRef, Ty)>,
        trait_impls: Vec<TraitImplRef>,
        ty: &NominalTy,
        name: &str,
    ) -> Result<(), PackageStoreError> {
        let item_query = self.context.item_query();
        let matcher = self.context.impl_matcher();
        for trait_impl in trait_impls {
            if !matcher
                .trait_impl_applicability(trait_impl, ty)?
                .is_applicable()
            {
                continue;
            }

            let Some(impl_data) = item_query.impl_data(trait_impl.impl_ref)? else {
                continue;
            };

            // Impl consts are the concrete declaration for `Type::CONST`. When the impl omits the
            // item, use the trait declaration as a best-effort source for defaulted or incomplete
            // code; const signatures do not preserve whether a default body was written.
            let mut candidate = self.associated_const_from_items(
                trait_impl.impl_ref.origin,
                &impl_data.items,
                ty,
                name,
            )?;
            if candidate.is_none()
                && let Some(trait_data) = item_query.trait_data(trait_impl.trait_ref)?
            {
                candidate = self.associated_const_from_items(
                    trait_impl.trait_ref.origin,
                    &trait_data.items,
                    ty,
                    name,
                )?;
            }

            let Some(candidate) = candidate else {
                continue;
            };
            if !items.iter().any(|(existing, _)| *existing == candidate.0) {
                items.push(candidate);
            }
        }

        Ok(())
    }

    fn associated_value_item_for_impls(
        &self,
        impls: Vec<ImplRef>,
        ty: &NominalTy,
        name: &str,
    ) -> Result<Option<(ConstRef, Ty)>, PackageStoreError> {
        let item_query = self.context.item_query();
        for impl_ref in impls {
            let Some(impl_data) = item_query.impl_data(impl_ref)? else {
                continue;
            };
            if !self
                .context
                .impl_matcher()
                .impl_applies_to_receiver(impl_ref, impl_data, ty)?
            {
                continue;
            }

            if let Some(item) =
                self.associated_const_from_items(impl_ref.origin, &impl_data.items, ty, name)?
            {
                return Ok(Some(item));
            }
        }

        Ok(None)
    }

    fn associated_const_from_items(
        &self,
        origin: DefMapRef,
        assoc_items: &[AssocItemId],
        receiver_ty: &NominalTy,
        name: &str,
    ) -> Result<Option<(ConstRef, Ty)>, PackageStoreError> {
        let item_query = self.context.item_query();
        for item in assoc_items {
            let AssocItemId::Const(id) = item else {
                continue;
            };
            let const_ref = ConstRef { origin, id: *id };
            let Some(const_data) = item_query.const_data(const_ref)? else {
                continue;
            };
            if const_data.name == name {
                return Ok(Some((
                    const_ref,
                    self.semantic_const_ty_for_receiver(const_ref, const_data.owner, receiver_ty)?,
                )));
            }
        }

        Ok(None)
    }

    fn semantic_const_ty_for_receiver(
        &self,
        const_ref: ConstRef,
        owner: ItemOwner,
        receiver_ty: &NominalTy,
    ) -> Result<Ty, PackageStoreError> {
        let item_query = self.context.item_query();
        let Some(const_data) = item_query.const_data(const_ref)? else {
            return Ok(Ty::Unknown);
        };
        let Some(ty) = const_data.signature.ty() else {
            return Ok(Ty::Unknown);
        };

        if ty.is_self_type() {
            return Ok(Ty::nominal(vec![receiver_ty.clone()]));
        }

        let mut subst = self.semantic_type_subst(receiver_ty)?;
        if let ItemOwner::Impl(impl_id) = owner {
            let impl_ref = ImplRef {
                origin: const_ref.origin,
                id: impl_id,
            };
            if let Some(impl_data) = item_query.impl_data(impl_ref)? {
                subst.extend(
                    self.context
                        .impl_matcher()
                        .impl_self_subst_for_impl(impl_data, receiver_ty),
                );
            }
        }

        let context = self
            .context
            .item_query()
            .type_path_context_for_owner(const_ref.origin, owner)?
            .unwrap_or_else(|| TypePathContext::module(self.context.body().owner_module()));
        self.context
            .type_path_query()
            .type_ref(TypeRefUseSite::OwnerContext(context))
            .with_subst(&subst)
            .resolve(ty)
    }

    fn semantic_type_subst(&self, ty: &NominalTy) -> Result<TypeSubst, PackageStoreError> {
        Ok(self
            .context
            .item_query()
            .generic_params_for_type_def(ty.def)?
            .map(|generics| TypeSubst::from_generics(generics, &ty.args))
            .unwrap_or_else(TypeSubst::new))
    }
}
