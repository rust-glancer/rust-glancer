//! Trait lookup in body context.

use rg_ir_model::{
    ItemOwner, Path, ScopeId, TraitImplRef, TraitRef, TypePathResolution, items::TypeRef,
};
use rg_ir_storage::{DefMapSource, ItemStoreSource, TypePathContext};
use rg_package_store::PackageStoreError;
use rg_std::UniqueVec;
use rg_ty::{GenericArg, NominalTy, Ty, TypeSubst};

use crate::resolution::{BodyResolutionContext, TypeRefUseSite};

/// Resolves trait-shaped questions in body context.
pub(crate) struct BodyTraitQuery<'query, D, I> {
    context: BodyResolutionContext<'query, D, I>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BodyQualifiedTraitSelection {
    subst: TypeSubst,
    receivers: Vec<BodyQualifiedTraitReceiverSelection>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BodyQualifiedTraitReceiverSelection {
    receiver_ty: NominalTy,
    impls: UniqueVec<TraitImplRef>,
}

struct ResolvedTraitPrefix {
    trait_ref: TraitRef,
    subst: TypeSubst,
    args: Vec<GenericArg>,
}

impl BodyQualifiedTraitSelection {
    /// Return substitutions from the written trait prefix, such as `T = User`.
    pub(crate) fn subst(&self) -> &TypeSubst {
        &self.subst
    }

    /// Return receiver types and impls selected by the qualified trait prefix.
    pub(crate) fn receivers(&self) -> &[BodyQualifiedTraitReceiverSelection] {
        &self.receivers
    }
}

impl BodyQualifiedTraitReceiverSelection {
    /// Return the `Self` type from `<Self as Trait>`.
    pub(crate) fn receiver_ty(&self) -> &NominalTy {
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
        let Some(trait_prefix) = self.resolve_trait_prefix(scope, trait_ty_ref)? else {
            return Ok(None);
        };

        let mut receivers = Vec::new();
        for receiver_ty in self.receiver_tys_for_prefix(&self_ty)? {
            let impls = self.qualified_trait_impls_for_type(
                &receiver_ty,
                trait_prefix.trait_ref,
                &trait_prefix.args,
            )?;
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
        self.context
            .type_refs(TypeRefUseSite::Scope(scope))
            .resolve(ty)
    }

    /// Resolve `Trait<Args>` from `<Self as Trait<Args>>`.
    fn resolve_trait_prefix(
        &self,
        scope: ScopeId,
        trait_ty_ref: &TypeRef,
    ) -> Result<Option<ResolvedTraitPrefix>, PackageStoreError> {
        let TypeRef::Path(type_path) = trait_ty_ref else {
            return Ok(None);
        };
        let path = Path::from_type_path(type_path);
        let TypePathResolution::Trait(trait_ref) = self
            .context
            .type_path_query()
            .resolve_in_scope(scope, &path)?
        else {
            return Ok(None);
        };

        let args = self.resolve_type_path_args(scope, type_path.segments.last())?;
        let Some(trait_data) = self.context.item_query().trait_data(trait_ref)? else {
            return Ok(None);
        };
        Ok(Some(ResolvedTraitPrefix {
            trait_ref,
            subst: TypeSubst::from_generics(&trait_data.generics, &args),
            args,
        }))
    }

    /// Keep impls whose trait definition and concrete trait args match the written prefix.
    fn qualified_trait_impls_for_type(
        &self,
        ty: &NominalTy,
        trait_ref: TraitRef,
        trait_args: &[GenericArg],
    ) -> Result<UniqueVec<TraitImplRef>, PackageStoreError> {
        let mut impls = UniqueVec::new();
        self.push_matching_qualified_trait_impls(
            &mut impls,
            self.context
                .body_local_items()
                .trait_impls_for_type(ty.def)?,
            ty,
            trait_ref,
            trait_args,
        )?;

        if ty.def.origin.as_target_ref().is_some() {
            let semantic_impls = self
                .context
                .semantic_index()
                .trait_impls_for_type(ty.def)
                .cloned()
                .unwrap_or_default();
            self.push_matching_qualified_trait_impls(
                &mut impls,
                semantic_impls,
                ty,
                trait_ref,
                trait_args,
            )?;
        }

        Ok(impls)
    }

    fn push_matching_qualified_trait_impls(
        &self,
        impls: &mut UniqueVec<TraitImplRef>,
        candidates: UniqueVec<TraitImplRef>,
        ty: &NominalTy,
        trait_ref: TraitRef,
        trait_args: &[GenericArg],
    ) -> Result<(), PackageStoreError> {
        for candidate in candidates {
            if candidate.trait_ref != trait_ref {
                continue;
            }
            if self.trait_impl_args_match_written_args(candidate, ty, trait_args)? {
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
        receiver_ty: &NominalTy,
        written_args: &[GenericArg],
    ) -> Result<bool, PackageStoreError> {
        if written_args.is_empty() {
            return Ok(true);
        }

        let Some(impl_data) = self.context.item_query().impl_data(trait_impl.impl_ref)? else {
            return Ok(false);
        };
        let Some(impl_trait_ref) = impl_data.trait_ref.as_ref() else {
            return Ok(false);
        };
        let TypeRef::Path(type_path) = impl_trait_ref else {
            return Ok(false);
        };

        let impl_subst = self
            .context
            .impl_matcher()
            .impl_self_subst_for_impl(impl_data, receiver_ty);
        let context = self
            .context
            .item_query()
            .type_path_context_for_owner(
                trait_impl.impl_ref.origin,
                ItemOwner::Impl(trait_impl.impl_ref.id),
            )?
            .unwrap_or_else(|| TypePathContext::module(self.context.body().owner_module()));
        let impl_args = self
            .context
            .type_refs(TypeRefUseSite::OwnerContext(context))
            .with_subst(&impl_subst)
            .resolve_generic_args(
                type_path
                    .segments
                    .last()
                    .map(|segment| segment.args.as_slice())
                    .unwrap_or(&[]),
            )?;

        Ok(Self::generic_args_match(written_args, &impl_args))
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

    /// Preserve written args and treat omitted type args as inferable unknowns.
    fn receiver_tys_for_prefix(&self, prefix_ty: &Ty) -> Result<Vec<NominalTy>, PackageStoreError> {
        prefix_ty
            .as_nominals()
            .iter()
            .map(|ty| self.receiver_ty_for_prefix(ty))
            .collect()
    }

    fn receiver_ty_for_prefix(&self, ty: &NominalTy) -> Result<NominalTy, PackageStoreError> {
        if !ty.args.is_empty() {
            return Ok(ty.clone());
        }
        let Some(generics) = self
            .context
            .item_query()
            .generic_params_for_type_def(ty.def)?
        else {
            return Ok(ty.clone());
        };
        if generics.types.is_empty() {
            return Ok(ty.clone());
        }

        Ok(NominalTy {
            def: ty.def,
            args: generics
                .types
                .iter()
                .map(|_| GenericArg::Type(Box::new(Ty::Unknown)))
                .collect(),
        })
    }

    fn resolve_type_path_args(
        &self,
        scope: ScopeId,
        segment: Option<&rg_ir_model::items::TypePathSegment>,
    ) -> Result<Vec<GenericArg>, PackageStoreError> {
        self.context
            .type_refs(TypeRefUseSite::Scope(scope))
            .resolve_generic_args(
                segment
                    .map(|segment| segment.args.as_slice())
                    .unwrap_or(&[]),
            )
    }
}
