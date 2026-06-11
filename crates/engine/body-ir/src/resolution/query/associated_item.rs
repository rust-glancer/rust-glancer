//! Read-only associated item path resolution for Body IR.
//!
//! This query owns `Type::item` lookup in value position: enum variants, associated consts, and
//! associated functions. Ordinary lexical/module value paths stay in
//! `BodyValuePathQuery`.

use rg_ir_model::{
    AssocItemId, ConstRef, DefMapRef, EnumVariantRef, FunctionRef, ImplRef, ItemOwner, Path,
    ScopeId, TraitImplRef, TypeDefId, identity::DeclarationRef,
};
use rg_ir_storage::{DefMapSource, ItemStoreSource, TypePathContext};
use rg_package_store::PackageStoreError;
use rg_std::UniqueVec;
use rg_ty::{NominalTy, Ty, TypeSubst};

use crate::{
    ir::resolved::BodyResolution,
    resolution::{BodyResolutionContext, TypeRefUseSite, support::unique_ty_or_unknown},
};

pub(crate) struct BodyAssociatedItemQuery<'query, D, I> {
    context: BodyResolutionContext<'query, D, I>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum BodyAssociatedItemCandidate {
    EnumVariant(EnumVariantRef, Ty),
    Const(ConstRef, Ty),
    Function(FunctionRef),
}

impl<'query, D, I> BodyAssociatedItemQuery<'query, D, I>
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
        // Associated item paths are resolved as "type prefix + value member". This keeps
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
        let mut variants = Vec::new();
        for nominal_ty in prefix_ty.as_nominals() {
            if let Some(candidate) =
                self.enum_variant_candidate_for_type(nominal_ty, last_segment)?
            {
                variants.push(candidate);
            }
        }

        if !variants.is_empty() {
            return Ok(Some(Self::enum_variant_resolution(variants)));
        }

        for nominal_ty in prefix_ty.as_nominals() {
            if let Some(candidate) =
                self.inherent_associated_const_candidate_for_type(nominal_ty, last_segment)?
            {
                return Ok(Some(Self::const_resolution([candidate])));
            }
        }

        // Trait associated const lookup is still receiver-driven: `Type::CONST` first proves that
        // `Type` has a visible trait impl, then reads the matching const item from that impl or
        // falls back to the trait declaration. This mirrors trait method lookup without pretending
        // to be a full trait solver.
        let mut trait_consts = Vec::new();
        for nominal_ty in prefix_ty.as_nominals() {
            trait_consts
                .extend(self.trait_associated_const_candidates_for_type(nominal_ty, last_segment)?);
        }

        if !trait_consts.is_empty() {
            return Ok(Some(Self::const_resolution(trait_consts)));
        }

        // Inherent associated functions are exact candidates. Trait-associated functions are kept
        // deliberately optimistic, following the same "prefer useful candidates over false
        // negatives" policy as dot completion.
        let mut functions = Vec::new();
        for nominal_ty in prefix_ty.as_nominals() {
            functions
                .extend(self.associated_function_candidates_for_type(nominal_ty, last_segment)?);
        }

        Ok((!functions.is_empty()).then_some(Self::function_resolution(functions)))
    }

    fn enum_variant_resolution(
        candidates: impl IntoIterator<Item = BodyAssociatedItemCandidate>,
    ) -> (BodyResolution, Ty) {
        let mut variants = UniqueVec::new();
        let mut tys = UniqueVec::new();

        for candidate in candidates {
            let BodyAssociatedItemCandidate::EnumVariant(variant_ref, ty) = candidate else {
                continue;
            };
            variants.push(variant_ref);
            tys.push(ty);
        }

        (
            BodyResolution::Declarations(variants.into_iter().map(DeclarationRef::from).collect()),
            unique_ty_or_unknown(tys),
        )
    }

    fn const_resolution(
        candidates: impl IntoIterator<Item = BodyAssociatedItemCandidate>,
    ) -> (BodyResolution, Ty) {
        let mut consts = UniqueVec::new();
        let mut tys = UniqueVec::new();

        for candidate in candidates {
            let BodyAssociatedItemCandidate::Const(const_ref, ty) = candidate else {
                continue;
            };
            consts.push(const_ref);
            tys.push(ty);
        }

        (
            BodyResolution::Declarations(consts.into_iter().map(DeclarationRef::from).collect()),
            unique_ty_or_unknown(tys),
        )
    }

    fn function_resolution(
        candidates: impl IntoIterator<Item = BodyAssociatedItemCandidate>,
    ) -> (BodyResolution, Ty) {
        let mut functions = UniqueVec::new();

        for candidate in candidates {
            let BodyAssociatedItemCandidate::Function(function_ref) = candidate else {
                continue;
            };
            functions.push(function_ref);
        }

        (
            BodyResolution::Declarations(functions.into_iter().map(DeclarationRef::from).collect()),
            Ty::Unknown,
        )
    }

    fn enum_variant_candidate_for_type(
        &self,
        ty: &NominalTy,
        name: &str,
    ) -> Result<Option<BodyAssociatedItemCandidate>, PackageStoreError> {
        if !matches!(ty.def.id, TypeDefId::Enum(_)) {
            return Ok(None);
        }

        Ok(self
            .context
            .item_query()
            .enum_variant_ref_for_type_def(ty.def, name)?
            .map(|variant_ref| {
                BodyAssociatedItemCandidate::EnumVariant(
                    variant_ref,
                    Ty::nominal([ty.clone()].into_iter().collect()),
                )
            }))
    }

    fn inherent_associated_const_candidate_for_type(
        &self,
        ty: &NominalTy,
        name: &str,
    ) -> Result<Option<BodyAssociatedItemCandidate>, PackageStoreError> {
        if let Some(item) = self.associated_const_candidate_for_impls(
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

        self.associated_const_candidate_for_impls(
            self.context
                .target_items()
                .inherent_impls_for_type(ty.def)?,
            ty,
            name,
        )
    }

    fn trait_associated_const_candidates_for_type(
        &self,
        ty: &NominalTy,
        name: &str,
    ) -> Result<Vec<BodyAssociatedItemCandidate>, PackageStoreError> {
        let mut items = Vec::new();

        self.push_trait_associated_const_candidates_for_impls(
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
            Some(index) => index
                .trait_impls_for_type(ty.def)
                .cloned()
                .unwrap_or_default(),
            None => target_items.trait_impls_for_type(ty.def)?,
        };
        self.push_trait_associated_const_candidates_for_impls(
            &mut items,
            semantic_trait_impls,
            ty,
            name,
        )?;

        Ok(items)
    }

    fn associated_function_candidates_for_type(
        &self,
        ty: &NominalTy,
        name: &str,
    ) -> Result<Vec<BodyAssociatedItemCandidate>, PackageStoreError> {
        let body_items = self.context.body_local_items();
        let matcher = self.context.impl_matcher();
        let mut functions = Vec::new();

        for function_ref in body_items.inherent_functions_for_type(ty.def)? {
            if matcher.function_applies_to_receiver(function_ref, ty)? {
                self.push_associated_function(&mut functions, function_ref, name)?;
            }
        }

        if ty.def.origin.as_target_ref().is_some() {
            for function_ref in self.semantic_inherent_function_items_for_type(ty, name)? {
                if matcher.function_applies_to_receiver(function_ref, ty)? {
                    self.push_associated_function(&mut functions, function_ref, name)?;
                }
            }
        }

        let body_trait_impls = body_items.trait_impls_for_type(ty.def)?;
        for (function_ref, _) in matcher.trait_function_candidates_from_impls(
            self.context.semantic_index(),
            body_trait_impls,
            ty,
            Some(name),
        )? {
            self.push_associated_function(&mut functions, function_ref, name)?;
        }

        if ty.def.origin.as_target_ref().is_some() {
            for (function_ref, _) in matcher.trait_function_candidates_for_receiver(
                self.context.semantic_index(),
                ty,
                Some(name),
            )? {
                self.push_associated_function(&mut functions, function_ref, name)?;
            }
        }

        Ok(functions)
    }

    fn semantic_inherent_function_items_for_type(
        &self,
        ty: &NominalTy,
        name: &str,
    ) -> Result<UniqueVec<FunctionRef>, PackageStoreError> {
        match self.context.semantic_index() {
            Some(index) => Ok(index
                .inherent_functions_for_type_and_name(ty.def, name)
                .cloned()
                .unwrap_or_default()),
            None => self
                .context
                .target_items()
                .inherent_functions_for_type(ty.def),
        }
    }

    fn push_associated_function(
        &self,
        functions: &mut Vec<BodyAssociatedItemCandidate>,
        function_ref: FunctionRef,
        name: &str,
    ) -> Result<(), PackageStoreError> {
        let Some(function_data) = self.context.item_query().function_data(function_ref)? else {
            return Ok(());
        };
        if function_data.name == name && !function_data.has_self_receiver() {
            let candidate = BodyAssociatedItemCandidate::Function(function_ref);
            if !functions.contains(&candidate) {
                functions.push(candidate);
            }
        }
        Ok(())
    }

    fn push_trait_associated_const_candidates_for_impls(
        &self,
        items: &mut Vec<BodyAssociatedItemCandidate>,
        trait_impls: UniqueVec<TraitImplRef>,
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
            if !items.iter().any(|existing| {
                matches!(
                    (existing, &candidate),
                    (
                        BodyAssociatedItemCandidate::Const(existing, _),
                        BodyAssociatedItemCandidate::Const(candidate, _)
                    ) if existing == candidate
                )
            }) {
                items.push(candidate);
            }
        }

        Ok(())
    }

    fn associated_const_candidate_for_impls(
        &self,
        impls: UniqueVec<ImplRef>,
        ty: &NominalTy,
        name: &str,
    ) -> Result<Option<BodyAssociatedItemCandidate>, PackageStoreError> {
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
    ) -> Result<Option<BodyAssociatedItemCandidate>, PackageStoreError> {
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
                return Ok(Some(BodyAssociatedItemCandidate::Const(
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
            return Ok(Ty::nominal([receiver_ty.clone()].into_iter().collect()));
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
            .type_refs(TypeRefUseSite::OwnerContext(context))
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
