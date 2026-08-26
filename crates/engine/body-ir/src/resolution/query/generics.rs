//! Owner-scoped generic substitution helpers for body queries.

use rg_def_map::DefMapSource;
use rg_ir_model::{DefMapRef, GenericDefRef, ImplRef, ItemOwner, ScopeId, TraitDefRef};
use rg_item_tree::GenericArg as ItemGenericArg;
use rg_package_store::PackageStoreError;
use rg_semantic_ir::{GenericParamSource, ItemStoreSource};
use rg_ty::{AdtTy, GenericArg, Substitution, Ty};

use crate::resolution::BodyResolutionContext;

pub(crate) struct BodyGenericsQuery<'query, D, I> {
    context: BodyResolutionContext<'query, D, I>,
}

impl<'query, D, I> BodyGenericsQuery<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    pub(crate) fn new(context: BodyResolutionContext<'query, D, I>) -> Self {
        Self { context }
    }

    /// Bind a concrete ADT's full argument list to the type definition's semantic parameters.
    pub(crate) fn subst_for_nominal_ty(
        &self,
        ty: &AdtTy,
    ) -> Result<Substitution, PackageStoreError> {
        let generics = self
            .context
            .item_paths()
            .generics()
            .generics(GenericDefRef::TypeDef(ty.def))?;
        Ok(Substitution::from_args(&generics, &ty.args))
    }

    /// Fill every canonical generic slot when a type-only prefix omitted its arguments.
    pub(crate) fn complete_omitted_nominal_args(
        &self,
        ty: &AdtTy,
    ) -> Result<AdtTy, PackageStoreError> {
        if !ty.args.is_empty() {
            return Ok(ty.clone());
        }
        let generics = self
            .context
            .item_paths()
            .generics()
            .generics(GenericDefRef::TypeDef(ty.def))?;
        if generics.is_empty() {
            return Ok(ty.clone());
        }

        Ok(AdtTy {
            def: ty.def,
            args: Substitution::new().args_for(&generics),
        })
    }

    /// Combine the receiver ADT arguments with bindings learned from its selected impl header.
    pub(crate) fn subst_for_receiver_owner(
        &self,
        origin: DefMapRef,
        owner: ItemOwner,
        receiver_ty: &AdtTy,
    ) -> Result<Substitution, PackageStoreError> {
        self.subst_for_receiver_ty_owner(origin, owner, &Ty::adt(receiver_ty.clone()))
    }

    /// Combine any canonical receiver shape with bindings learned from its declaration owner.
    ///
    /// For a function declared by `impl<T> [T]`, receiver `[User]` contributes `T = User`. For a
    /// trait method called on `[User; 3]`, the declaration owner contributes
    /// `Self = [User; 3]`; the candidate's separate trait selection retains the array impl's
    /// `T = User, N = 3` evidence.
    pub(crate) fn subst_for_receiver_ty_owner(
        &self,
        origin: DefMapRef,
        owner: ItemOwner,
        receiver_ty: &Ty,
    ) -> Result<Substitution, PackageStoreError> {
        let mut subst = match receiver_ty {
            Ty::Adt(receiver_ty) => self.subst_for_nominal_ty(receiver_ty)?,
            _ => Substitution::new(),
        };
        match owner {
            ItemOwner::Module(_) => {}
            ItemOwner::Trait(id) => {
                let generics = self
                    .context
                    .item_paths()
                    .generics()
                    .generics(GenericDefRef::Trait(TraitDefRef { origin, id }))?;
                if let Some(self_param) = generics.iter().find_map(|param| {
                    matches!(param.source(), GenericParamSource::TraitSelf).then_some(param.param())
                }) {
                    subst.push(self_param, GenericArg::Type(Box::new(receiver_ty.clone())));
                }
            }
            ItemOwner::Impl(impl_id) => {
                let impl_ref = ImplRef {
                    origin,
                    id: impl_id,
                };
                if let Some((impl_subst, _)) = self
                    .context
                    .impl_matcher()
                    .impl_self_subst_for_impl(impl_ref, receiver_ty)?
                {
                    subst.extend(impl_subst);
                }
            }
        }
        Ok(subst)
    }

    /// Lower a turbofish against the declaration owner's canonical parameter order.
    pub(crate) fn subst_for_explicit_args(
        &self,
        owner: GenericDefRef,
        args: &[ItemGenericArg],
        scope: ScopeId,
    ) -> Result<Substitution, PackageStoreError> {
        if args.is_empty() {
            return Ok(Substitution::new());
        }
        let args = self
            .context
            .type_refs(scope)
            .resolve_generic_args_for(owner, args)?;
        let generics = self.context.item_paths().generics().generics(owner)?;
        let mut subst = Substitution::new();

        // The written turbofish belongs only to this owner. `lower_generic_args_for` returns a
        // full-arity list so defaults can refer to parent parameters, but those placeholder parent
        // entries must not overwrite receiver/impl evidence selected by the caller.
        for (param, arg) in generics
            .iter_self()
            .zip(args.iter().skip(generics.parent_len()))
        {
            subst.push(param.param(), arg.clone());
        }
        Ok(subst)
    }
}
