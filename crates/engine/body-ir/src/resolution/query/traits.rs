//! Trait lookup in body context.

use rg_def_map::DefMapSource;
use rg_ir_model::{GenericDefRef, ScopeId, TraitImplRef};
use rg_item_tree::TypeRef;
use rg_package_store::PackageStoreError;
use rg_semantic_ir::ItemStoreSource;
use rg_std::UniqueVec;
use rg_ty::{AdtTy, GenericArg, Substitution, TraitApplication, Ty};

use crate::resolution::BodyResolutionContext;

/// Resolves trait-shaped questions in body context.
pub(crate) struct BodyTraitQuery<'query, D, I> {
    context: BodyResolutionContext<'query, D, I>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BodyQualifiedTraitSelection {
    subst: Substitution,
    receivers: Vec<BodyQualifiedTraitReceiverSelection>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BodyQualifiedTraitReceiverSelection {
    receiver_ty: AdtTy,
    impls: UniqueVec<TraitImplRef>,
}

struct ResolvedTraitPrefix {
    application: TraitApplication,
    subst: Substitution,
}

impl BodyQualifiedTraitSelection {
    /// Return substitutions from the written trait prefix, such as `T = User`.
    pub(crate) fn subst(&self) -> &Substitution {
        &self.subst
    }

    /// Return receiver types and impls selected by the qualified trait prefix.
    pub(crate) fn receivers(&self) -> &[BodyQualifiedTraitReceiverSelection] {
        &self.receivers
    }
}

impl BodyQualifiedTraitReceiverSelection {
    /// Return the `Self` type from `<Self as Trait>`.
    pub(crate) fn receiver_ty(&self) -> &AdtTy {
        &self.receiver_ty
    }

    /// Return impls matching the written trait path and receiver.
    pub(crate) fn impls(&self) -> &UniqueVec<TraitImplRef> {
        &self.impls
    }
}

impl<'query, D, I> BodyTraitQuery<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    pub(crate) fn new(context: BodyResolutionContext<'query, D, I>) -> Self {
        Self { context }
    }

    /// Resolve `<Self as Trait<Args>>` into receiver-specific trait impls.
    pub(crate) fn qualified_selection(
        &self,
        scope: ScopeId,
        self_ty_ref: &TypeRef,
        trait_ty_ref: &TypeRef,
    ) -> Result<Option<BodyQualifiedTraitSelection>, PackageStoreError> {
        let self_ty = self.resolve_type_ref(scope, self_ty_ref)?;
        let Some(trait_prefix) = self.resolve_trait_prefix(scope, trait_ty_ref, self_ty.clone())?
        else {
            return Ok(None);
        };

        let mut receivers = Vec::new();
        for receiver_ty in self_ty.as_adts() {
            let receiver_ty = self
                .context
                .generics()
                .complete_omitted_nominal_args(receiver_ty)?;
            let impls =
                self.qualified_trait_impls_for_type(&receiver_ty, &trait_prefix.application)?;
            if !impls.is_empty() {
                receivers.push(BodyQualifiedTraitReceiverSelection { receiver_ty, impls });
            }
        }

        Ok(
            (!receivers.is_empty()).then_some(BodyQualifiedTraitSelection {
                subst: trait_prefix.subst,
                receivers,
            }),
        )
    }

    /// Resolve a type syntax where the qualified path is written.
    fn resolve_type_ref(&self, scope: ScopeId, ty: &TypeRef) -> Result<Ty, PackageStoreError> {
        self.context.type_refs(scope).resolve(ty)
    }

    /// Resolve `Trait<Args>` from `<Self as Trait<Args>>`.
    fn resolve_trait_prefix(
        &self,
        scope: ScopeId,
        trait_ty_ref: &TypeRef,
        self_ty: Ty,
    ) -> Result<Option<ResolvedTraitPrefix>, PackageStoreError> {
        let Some(lowering) = self
            .context
            .type_refs(scope)
            .resolve_trait_ref(trait_ty_ref, self_ty)?
        else {
            return Ok(None);
        };
        let generics = self
            .context
            .item_paths()
            .generics()
            .generics(GenericDefRef::Trait(lowering.application.def))?;
        Ok(Some(ResolvedTraitPrefix {
            subst: Substitution::from_args(&generics, &lowering.application.args),
            application: lowering.application,
        }))
    }

    /// Keep impls whose trait definition and concrete trait args match the written prefix.
    fn qualified_trait_impls_for_type(
        &self,
        ty: &AdtTy,
        application: &TraitApplication,
    ) -> Result<UniqueVec<TraitImplRef>, PackageStoreError> {
        let mut impls = UniqueVec::new();
        self.push_matching_qualified_trait_impls(
            &mut impls,
            self.context
                .body_local_items()
                .trait_impls_for_type(ty.def)?,
            ty,
            application,
        )?;

        if ty.def.origin.as_crate_ref().is_some() {
            let semantic_impls = self
                .context
                .item_lookup_index()
                .trait_impls_for_type(ty.def);
            self.push_matching_qualified_trait_impls(&mut impls, semantic_impls, ty, application)?;
        }

        Ok(impls)
    }

    fn push_matching_qualified_trait_impls(
        &self,
        impls: &mut UniqueVec<TraitImplRef>,
        candidates: UniqueVec<TraitImplRef>,
        ty: &AdtTy,
        application: &TraitApplication,
    ) -> Result<(), PackageStoreError> {
        for candidate in candidates {
            if candidate.trait_ref != application.def {
                continue;
            }
            if self.trait_impl_args_match_written_args(candidate, ty, application)? {
                impls.push(candidate);
            }
        }
        Ok(())
    }

    /// Compare concrete args in the impl header with `<Self as Trait<Args>>`.
    ///
    /// This is a syntax-driven filter: it resolves impl-header args after applying receiver subst,
    /// then compares them to the args written in the qualified path.
    fn trait_impl_args_match_written_args(
        &self,
        trait_impl: TraitImplRef,
        receiver_ty: &AdtTy,
        written: &TraitApplication,
    ) -> Result<bool, PackageStoreError> {
        let matcher = self.context.impl_matcher();
        let Some(header) = matcher.impl_header(trait_impl.impl_ref)? else {
            return Ok(false);
        };
        let Some((impl_subst, _applicability)) =
            matcher.impl_self_subst_for_impl(trait_impl.impl_ref, &Ty::adt(receiver_ty.clone()))?
        else {
            return Ok(false);
        };
        let Some(impl_trait) = header.trait_ref else {
            return Ok(false);
        };
        let impl_application = impl_subst.apply_trait_application(&impl_trait.application);
        Ok(impl_application.def == written.def
            && Self::generic_args_match(&written.args, &impl_application.args))
    }

    /// Treat unknown args as compatible; incomplete code should not create false negatives.
    fn generic_args_match(written_args: &[GenericArg], impl_args: &[GenericArg]) -> bool {
        written_args.len() == impl_args.len()
            && written_args
                .iter()
                .zip(impl_args)
                .all(|(written_arg, impl_arg)| {
                    written_arg == impl_arg || written_arg.has_unknown() || impl_arg.has_unknown()
                })
    }
}
