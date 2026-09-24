//! Lower trait bounds and associated equalities into semantic applications and clauses.

use rg_def_map::DefMapSource;
use rg_ir_model::{GenericDefRef, GenericParamRef, TraitDefRef};
use rg_item_tree::{GenericArg as ItemGenericArg, TypeBound, TypePath, TypeRef, WherePredicate};
use rg_semantic_ir::{GenericParamSource, ItemStoreSource, TypePathResolution};
use rg_std::UniqueVec;

use super::{ImplTraitMode, TypeLoweringSession, TypePathResolver};
use crate::{
    AssocTypeBinding, Clause, GenericArg, Substitution, TraitApplication, TraitRefLowering, Ty,
};

impl<'lower, 'query, D, I, R> TypeLoweringSession<'lower, 'query, D, I, R>
where
    D: DefMapSource,
    I: ItemStoreSource<'query, Error = D::Error>,
    R: TypePathResolver<Error = D::Error>,
{
    /// Lower a trait bound with `Self` already known.
    pub fn lower_trait_ref(
        &mut self,
        trait_ty: &TypeRef,
        self_ty: Ty,
    ) -> Result<Option<TraitRefLowering>, D::Error> {
        self.lower_trait_ref_with_mode(trait_ty, self_ty, ImplTraitMode::Opaque, None)
    }

    pub(crate) fn lower_trait_ref_with_mode(
        &mut self,
        trait_ty: &TypeRef,
        self_ty: Ty,
        impl_trait_mode: ImplTraitMode,
        inference: Option<&dyn Fn() -> Ty>,
    ) -> Result<Option<TraitRefLowering>, D::Error> {
        let TypeRef::Path(path) = trait_ty else {
            return Ok(None);
        };
        let Some(trait_ref) = self.resolve_trait_def(trait_ty)? else {
            return Ok(None);
        };

        self.lower_resolved_trait_ref_with_mode(
            path,
            trait_ref,
            self_ty,
            impl_trait_mode,
            inference,
        )
        .map(Some)
    }

    /// Resolve only the trait definition named by a bound, without interpreting its arguments.
    ///
    /// Associated shorthand lookup uses this identity first to ignore traits that cannot provide
    /// the requested associated type. Lowering every argument eagerly can recurse through the
    /// shorthand being resolved, as in `T: Other<T::Item>`.
    pub(crate) fn resolve_trait_def(
        &self,
        trait_ty: &TypeRef,
    ) -> Result<Option<TraitDefRef>, D::Error> {
        let TypeRef::Path(path) = trait_ty else {
            return Ok(None);
        };
        let Some(path_key) = path.as_def_map_path() else {
            return Ok(None);
        };
        let TypePathResolution::Trait(trait_ref) = self
            .query
            .resolver
            .resolve_type_path(self.anchor, &path_key)?
        else {
            return Ok(None);
        };
        Ok(Some(trait_ref))
    }

    pub(crate) fn lower_resolved_trait_ref_with_mode(
        &mut self,
        path: &TypePath,
        trait_ref: TraitDefRef,
        self_ty: Ty,
        impl_trait_mode: ImplTraitMode,
        inference: Option<&dyn Fn() -> Ty>,
    ) -> Result<TraitRefLowering, D::Error> {
        let generics = self
            .query
            .item_paths
            .generics()
            .generics(GenericDefRef::Trait(trait_ref))?;
        let mut seed = Substitution::new();
        if let Some(self_param) = generics.iter().find_map(|param| {
            matches!(param.source(), GenericParamSource::TraitSelf).then_some(param.param())
        }) {
            seed.push(self_param, GenericArg::Type(Box::new(self_ty)));
        }

        let syntax_args = path
            .segments
            .last()
            .map(|segment| segment.args.as_slice())
            .unwrap_or_default();
        let args =
            self.lower_generic_args(&generics, syntax_args, &seed, impl_trait_mode, inference)?;
        let application = TraitApplication {
            def: trait_ref,
            args,
        };
        let associated_types =
            self.lower_associated_bindings(&application, syntax_args, impl_trait_mode, inference)?;
        Ok(TraitRefLowering {
            application,
            associated_types,
        })
    }

    /// Find the traits that the surrounding declaration says one type must implement.
    ///
    /// For example, while lowering `inspect`, asking about `T` returns applications of `Factory`,
    /// `Send`, and `Clone`:
    ///
    /// ```text
    /// struct Wrapper<T>(T);
    ///
    /// impl<T: Factory + Send> Wrapper<T> {
    ///     fn inspect<U: Debug>(&self)
    ///     where
    ///         T: Clone,
    ///     {
    ///         T::/* Factory + Send + Clone */
    ///     }
    /// }
    /// ```
    ///
    /// The impl supplies the parent-owner bounds, while the method supplies `Clone`. `U: Debug` is
    /// ignored because this method keeps only clauses whose trait `Self` argument is exactly `ty`.
    pub fn trait_applications_for_type(
        &mut self,
        ty: &Ty,
    ) -> Result<UniqueVec<TraitApplication>, D::Error> {
        let mut applications = UniqueVec::new();
        for clause in self.lower_clauses()? {
            let Clause::Implemented(application) = clause else {
                continue;
            };
            if application.self_ty() == Some(ty) {
                applications.push(application);
            }
        }
        Ok(applications)
    }

    /// Lower every trait predicate visible from this owner.
    pub(crate) fn lower_clauses(&mut self) -> Result<Vec<Clause>, D::Error> {
        let generics = self.query.item_paths.generics().generics(self.owner)?;
        let mut inline_bounds = Vec::new();
        let mut predicate_owners = UniqueVec::new();
        for param in generics.iter() {
            let owner = param.param().owner();
            predicate_owners.push(owner);
            let GenericParamRef::Type(param_ref) = param.param() else {
                continue;
            };
            let bounds = match param.source() {
                GenericParamSource::Type(param) => param.bounds.clone(),
                GenericParamSource::ArgumentImplTrait(bounds) => bounds.to_vec(),
                GenericParamSource::Lifetime(_)
                | GenericParamSource::Const(_)
                | GenericParamSource::TraitSelf => continue,
            };
            inline_bounds.push((param_ref.owner, Ty::Param(param_ref), bounds));
        }
        predicate_owners.push(self.owner);

        let mut where_predicates = Vec::new();
        for owner in predicate_owners {
            if let Some(item) = self
                .query
                .item_paths
                .items()
                .semantic_item_view(owner.into())?
                && let Some(params) = item.generic_params()
            {
                where_predicates.extend(
                    params
                        .where_predicates
                        .iter()
                        .cloned()
                        .map(|predicate| (owner, predicate)),
                );
            }
        }

        let mut clauses = Vec::new();
        for (owner, subject, bounds) in inline_bounds {
            let anchor = self.anchor_for_owner(owner)?;
            self.with_owner_anchor(owner, anchor, |session| {
                session.lower_bound_clauses(subject, &bounds, &mut clauses)
            })?;
        }
        for (owner, predicate) in where_predicates {
            let WherePredicate::Type { ty, bounds } = predicate else {
                continue;
            };
            let anchor = self.anchor_for_owner(owner)?;
            self.with_owner_anchor(owner, anchor, |session| {
                let subject = session.lower_type_ref(&ty)?;
                session.lower_bound_clauses(subject, &bounds, &mut clauses)
            })?;
        }
        Ok(clauses)
    }

    fn lower_bound_clauses(
        &mut self,
        subject: Ty,
        bounds: &[TypeBound],
        clauses: &mut Vec<Clause>,
    ) -> Result<(), D::Error> {
        for bound in bounds {
            let Some(trait_ty) = bound.required_trait_ty() else {
                continue;
            };
            if let Some(trait_ref) = self.lower_trait_ref(trait_ty, subject.clone())? {
                clauses.extend(trait_ref.into_clauses());
            }
        }
        Ok(())
    }

    fn lower_associated_bindings(
        &mut self,
        application: &TraitApplication,
        syntax_args: &[ItemGenericArg],
        impl_trait_mode: ImplTraitMode,
        inference: Option<&dyn Fn() -> Ty>,
    ) -> Result<Vec<AssocTypeBinding>, D::Error> {
        let mut bindings = Vec::new();
        for arg in syntax_args {
            let output_name;
            let (name, ty) = match arg {
                ItemGenericArg::AssocType { name, ty, .. } => (name, ty.as_ref()),
                ItemGenericArg::FnTraitArgs { ret, .. } => {
                    output_name = rg_text::Name::new("Output");
                    (&output_name, Some(ret.as_ref()))
                }
                ItemGenericArg::Type(_)
                | ItemGenericArg::Lifetime(_)
                | ItemGenericArg::Const(_)
                | ItemGenericArg::Unsupported(_) => continue,
            };
            let Some(alias) = self.associated_type_projection(application, name)? else {
                continue;
            };
            // `AssocTypeBinding` belongs to the surrounding trait application. A transformed
            // supertrait projection needs its own argument list, which that compact goal shape
            // cannot represent yet. Keeping it unresolved is safer than attaching the equality
            // to the wrong application; `Fn`/`FnMut` inherit `Output` with the same arguments and
            // therefore take the supported path.
            if alias.args != application.args {
                continue;
            }
            let Some(ty) = ty else {
                continue;
            };
            bindings.push(AssocTypeBinding {
                associated_ty: alias.associated_ty,
                ty: self.lower_type_ref_with_mode(ty, impl_trait_mode, inference)?,
            });
        }
        Ok(bindings)
    }
}
