//! Resolve associated type identities through parameter bounds and supertraits.

use rg_def_map::DefMapSource;
use rg_ir_model::{GenericDefRef, GenericParamRef, TraitDefRef, TypeParamRef};
use rg_item_tree::{TypeRef, WherePredicate};
use rg_semantic_ir::{GenericParamSource, ItemStoreSource};
use rg_std::{ExpectedUnique, UniqueVec};
use rg_text::Name;

use super::{ImplTraitMode, TypeLoweringSession, TypePathResolver};
use crate::solver::{InferenceSubstitution as Substitution, ProjectionTy, TraitApplication, Ty};

impl<'s, 'lower, 'query, D, I, R> TypeLoweringSession<'s, 'lower, 'query, D, I, R>
where
    D: DefMapSource,
    I: ItemStoreSource<'query, Error = D::Error>,
    R: TypePathResolver<Error = D::Error>,
{
    /// Find an associated type on a trait or one of its supertraits.
    ///
    /// Associated items are inherited semantically even though they remain owned by the trait
    /// that declared them. In particular, callable syntax on `Fn` and `FnMut` constrains
    /// `FnOnce::Output`, so direct-item lookup is not enough.
    pub(crate) fn associated_type_projection(
        &mut self,
        application: &TraitApplication<'s>,
        name: &Name,
    ) -> Result<Option<ProjectionTy<'s>>, D::Error> {
        // Supertrait arguments can refer back to the associated item being searched for. The
        // lineage inside the graph walk cannot see that re-entry because lowering the argument
        // starts a fresh walk, so retain the semantic request across the complete lowering session.
        if self
            .associated_projection_stack
            .iter()
            .any(|(trait_ref, candidate)| *trait_ref == application.def && candidate == name)
        {
            self.report_limit("associated_projection_cycle", None);
            return Ok(None);
        }

        self.associated_projection_stack
            .push((application.def, name.clone()));
        let result = self.associated_type_projection_inner(application, name, &[]);
        let popped = self
            .associated_projection_stack
            .pop()
            .expect("projection request was pushed above");
        debug_assert_eq!(popped.0, application.def);
        debug_assert_eq!(popped.1, *name);
        result
    }

    fn associated_type_projection_inner(
        &mut self,
        application: &TraitApplication<'s>,
        name: &Name,
        lineage: &[TraitDefRef],
    ) -> Result<Option<ProjectionTy<'s>>, D::Error> {
        if lineage.contains(&application.def) {
            return Ok(None);
        }
        let Some(data) = self.item_paths.items().trait_data(application.def)? else {
            return Ok(None);
        };
        if let Some(alias) = self
            .item_paths
            .items()
            .declared_associated_type_by_name(application.def, name.as_str())?
        {
            return Ok(Some(ProjectionTy {
                associated_ty: alias,
                args: application.args,
            }));
        }

        let super_traits = data.super_traits.clone();
        let owner = GenericDefRef::Trait(application.def);
        let generics = self.item_paths.generics().generics(owner)?;
        let Some(self_param) = generics.iter().find_map(|param| {
            matches!(param.source(), GenericParamSource::TraitSelf).then_some(param.param())
        }) else {
            return Ok(None);
        };
        let GenericParamRef::Type(self_param) = self_param else {
            return Ok(None);
        };
        let anchor = self.anchor_for_owner(owner)?;
        let application_subst =
            Substitution::from_args(generics.iter().map(|p| p.param()), application.args);
        let mut next_lineage = lineage.to_vec();
        next_lineage.push(application.def);

        for bound in super_traits {
            let Some(trait_ty) = bound.required_trait_ty() else {
                continue;
            };

            // Supertrait syntax is written in the declaring trait's generic namespace. Lower it
            // against identity parameters first, then apply the concrete current application.
            let previous_subst = std::mem::replace(
                &mut self.subst,
                Substitution::identity(self.cx, generics.iter().map(|p| p.param())),
            );
            let lowered = self.with_owner_anchor(owner, anchor, |session| {
                session.lower_trait_ref(trait_ty, session.param_ty(self_param))
            });
            self.subst = previous_subst;
            let Some(super_trait) = lowered? else {
                continue;
            };
            let super_application = TraitApplication {
                def: super_trait.application.def,
                args: application_subst.apply(self.cx, super_trait.application.args),
            };
            if let Some(alias) =
                self.associated_type_projection_inner(&super_application, name, &next_lineage)?
            {
                return Ok(Some(alias));
            }
        }
        Ok(None)
    }

    /// Check whether a trait exposes an associated type directly or through a supertrait.
    ///
    /// This walk intentionally follows identities only. Its caller uses the answer to decide
    /// whether lowering a bound's generic arguments can contribute to `T::Assoc` resolution.
    fn trait_exposes_associated_type(
        &mut self,
        trait_ref: TraitDefRef,
        name: &Name,
    ) -> Result<bool, D::Error> {
        self.trait_exposes_associated_type_inner(trait_ref, name, &[])
    }

    fn trait_exposes_associated_type_inner(
        &mut self,
        trait_ref: TraitDefRef,
        name: &Name,
        lineage: &[TraitDefRef],
    ) -> Result<bool, D::Error> {
        if lineage.contains(&trait_ref) {
            return Ok(false);
        }
        let Some(data) = self.item_paths.items().trait_data(trait_ref)? else {
            return Ok(false);
        };
        if self
            .item_paths
            .items()
            .declared_associated_type_by_name(trait_ref, name.as_str())?
            .is_some()
        {
            return Ok(true);
        }

        let super_traits = data.super_traits.clone();
        let owner = GenericDefRef::Trait(trait_ref);
        let anchor = self.anchor_for_owner(owner)?;
        let mut next_lineage = lineage.to_vec();
        next_lineage.push(trait_ref);
        self.with_owner_anchor(owner, anchor, |session| {
            for bound in super_traits {
                let Some(trait_ty) = bound.required_trait_ty() else {
                    continue;
                };
                let Some(super_trait) = session.resolve_trait_def(trait_ty)? else {
                    continue;
                };
                if session.trait_exposes_associated_type_inner(super_trait, name, &next_lineage)? {
                    return Ok(true);
                }
            }
            Ok(false)
        })
    }

    /// Resolve `T::Assoc` from the unique trait bound on the owner-scoped parameter `T`.
    ///
    /// Bound arguments may contain the same shorthand while candidates are inspected. Re-entering
    /// one active `(T, Assoc)` request would require an infinitely recursive semantic type, so that
    /// inner occurrence stays unresolved instead of growing the Rust stack.
    pub(crate) fn param_associated_projection(
        &mut self,
        param: TypeParamRef,
        self_ty: Ty<'s>,
        assoc_name: &Name,
    ) -> Result<Option<ProjectionTy<'s>>, D::Error> {
        if self
            .param_projection_stack
            .iter()
            .any(|(candidate, name)| *candidate == param && name == assoc_name)
        {
            return Ok(None);
        }

        self.param_projection_stack
            .push((param, assoc_name.clone()));
        let result = self.param_associated_projection_inner(param, self_ty, assoc_name);
        let popped = self
            .param_projection_stack
            .pop()
            .expect("projection request was pushed above");
        debug_assert_eq!(popped.0, param);
        debug_assert_eq!(popped.1, *assoc_name);
        result
    }

    fn param_associated_projection_inner(
        &mut self,
        param: TypeParamRef,
        self_ty: Ty<'s>,
        assoc_name: &Name,
    ) -> Result<Option<ProjectionTy<'s>>, D::Error> {
        let generics = self.item_paths.generics().generics(self.owner)?;
        let mut bound_groups = Vec::new();
        if let Some(candidate) = generics
            .iter()
            .find(|candidate| candidate.param() == GenericParamRef::Type(param))
        {
            let bounds = match candidate.source() {
                GenericParamSource::Type(source) => source.bounds.clone(),
                GenericParamSource::ArgumentImplTrait(bounds) => bounds.to_vec(),
                GenericParamSource::Lifetime(_)
                | GenericParamSource::Const(_)
                | GenericParamSource::TraitSelf => Vec::new(),
            };
            bound_groups.push((param.owner, bounds));
        }

        // Child owners may constrain an inherited parameter in their own where-clause. Inspect
        // every declaration owner represented in the full generic list, while identity lookup
        // still lets a child parameter shadow a parent with the same source name.
        let mut predicate_owners = UniqueVec::new();
        for candidate in generics.iter() {
            predicate_owners.push(candidate.param().owner());
        }
        predicate_owners.push(self.owner);
        for owner in predicate_owners {
            let predicates = self
                .item_paths
                .items()
                .semantic_item_view(owner.into())?
                .and_then(|item| item.generic_params())
                .map(|params| params.where_predicates.clone())
                .unwrap_or_default();
            for predicate in predicates {
                let WherePredicate::Type {
                    ty,
                    bounds: predicate_bounds,
                } = predicate
                else {
                    continue;
                };
                let TypeRef::Path(path) = ty else {
                    continue;
                };
                let Some(name) = path.single_name() else {
                    continue;
                };
                let predicate_param = self
                    .item_paths
                    .generics()
                    .generics(owner)?
                    .param_by_name(name.as_str());
                if predicate_param == Some(GenericParamRef::Type(param)) {
                    bound_groups.push((owner, predicate_bounds));
                }
            }
        }

        let mut selected = ExpectedUnique::new();
        for (owner, bounds) in bound_groups {
            let anchor = self.anchor_for_owner(owner)?;
            let unambiguous = self.with_owner_anchor(owner, anchor, |session| {
                for bound in bounds {
                    let Some(trait_ty) = bound.required_trait_ty() else {
                        continue;
                    };
                    let TypeRef::Path(path) = &trait_ty else {
                        continue;
                    };
                    let Some(trait_def) = session.resolve_trait_def(trait_ty)? else {
                        continue;
                    };
                    // Candidate discovery is an identity operation. Only a trait that actually
                    // exposes this name needs its argument syntax lowered into a full application.
                    if !session.trait_exposes_associated_type(trait_def, assoc_name)? {
                        continue;
                    }
                    let trait_ref = session.lower_resolved_trait_ref_with_mode(
                        path,
                        trait_def,
                        self_ty,
                        ImplTraitMode::Opaque,
                        None,
                    )?;
                    let Some(candidate) =
                        session.associated_type_projection(&trait_ref.application, assoc_name)?
                    else {
                        continue;
                    };
                    selected.push(candidate);
                    if selected.is_ambiguous() {
                        return Ok(false);
                    }
                }
                Ok(true)
            })?;
            if !unambiguous {
                return Ok(None);
            }
        }

        Ok(selected.into_option())
    }
}
