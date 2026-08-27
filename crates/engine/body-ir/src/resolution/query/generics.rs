//! Owner-scoped generic substitution helpers for body queries.

use rg_def_map::DefMapSource;
use rg_ir_model::{DefMapRef, GenericDefRef, ItemOwner, ScopeId, TraitDefRef};
use rg_item_tree::GenericArg as ItemGenericArg;
use rg_package_store::PackageStoreError;
use rg_semantic_ir::{GenericParamSource, ItemStoreSource};
use rg_ty::{AdtTy, GenericArg, Substitution, TraitApplication, Ty};

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

    /// Build an item substitution from receiver facts and already-selected impl evidence.
    ///
    /// Receiver lookup has already matched `impl<T> [T]` against `[User]`, so an impl-owned item
    /// receives that `T = User` substitution directly. Passing it through this boundary avoids a
    /// second header match and makes the absence of impl evidence explicit for module- and
    /// trait-owned declarations.
    pub(crate) fn subst_for_selected_item_owner(
        &self,
        origin: DefMapRef,
        owner: ItemOwner,
        receiver_ty: &Ty,
        impl_subst: Option<&Substitution>,
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
            ItemOwner::Impl(_) => {
                // Malformed or incomplete code can leave an impl-owned declaration without a
                // usable header match. Keep the receiver's nominal bindings and fail soft rather
                // than reconstructing evidence that receiver discovery deliberately rejected.
                if let Some(impl_subst) = impl_subst {
                    subst.extend(impl_subst.clone());
                }
            }
        }
        Ok(subst)
    }

    /// Bind the selected trait application to the trait declaration's parameter identities.
    ///
    /// Method signatures are stored under the trait, not the impl. For
    /// `impl Convert<u16> for u8`, the selection's application maps the trait's `Self` to `u8`
    /// and its `T` to `u16`; the impl substitution alone has no trait-owned `T` key.
    pub(crate) fn subst_for_trait_application(
        &self,
        application: &TraitApplication,
    ) -> Result<Substitution, PackageStoreError> {
        let generics = self
            .context
            .item_paths()
            .generics()
            .generics(GenericDefRef::Trait(application.def))?;
        Ok(Substitution::from_args(&generics, &application.args))
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
