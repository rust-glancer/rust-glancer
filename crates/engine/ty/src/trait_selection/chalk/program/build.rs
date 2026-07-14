//! Materialization of discovered semantic definitions into Chalk datums.
//!
//! Chalk datums refer to each other by solver IDs, so build order matters. Associated-type
//! declarations are registered first; trait and impl predicates can then mention those IDs; opaque
//! bounds come after the traits they expose. ADTs are collected along the way whenever they appear
//! inside a lowered substitution or predicate.

use std::collections::HashMap;
use std::sync::Arc;

use chalk_ir::{AliasTy, AssocTypeId, Substitution, Ty, TyKind, Variance, Variances, WhereClause};
use chalk_solve::rust_ir::{AssociatedTyValueId, ImplDatum, TraitDatum};
use rg_def_map::DefMapSource;
use rg_ir_model::{AssocItemId, GenericDefRef, ImplRef, TraitDefRef, TypeAliasRef, TypeDefRef};
use rg_semantic_ir::{CrateItemQuery, ImplData, ItemStoreSource};
use rg_std::UniqueVec;

use super::super::interner::RgChalkInterner;
use super::super::lower::{
    ChalkLowerer, GenericBinderEnv, adt_datum, chalk_assoc_type_id, chalk_assoc_type_value_id,
};
use super::{ChalkProgram, ChalkProgramRoots, ChalkProgramScope};
use crate::trait_selection::TraitSelectionCache;
use crate::{ItemPathQuery, SemanticSignatureQuery};

const INTER: RgChalkInterner = RgChalkInterner;

impl ChalkProgram {
    pub(super) fn empty() -> Self {
        Self {
            materialized_traits: UniqueVec::new(),
            materialized_opaque_owners: UniqueVec::new(),
            traits: HashMap::new(),
            trait_arities: HashMap::new(),
            associated_tys: HashMap::new(),
            associated_ty_by_trait_name: HashMap::new(),
            associated_ty_values: HashMap::new(),
            associated_ty_value_by_impl: HashMap::new(),
            opaque_tys: HashMap::new(),
            adts: HashMap::new(),
            adt_variances: HashMap::new(),
            impls: HashMap::new(),
            impls_by_trait: HashMap::new(),
        }
    }

    /// Add the complete dependency closure of a set of previously unseen roots.
    pub(super) fn extend<'query, D, I>(
        &mut self,
        item_paths: &ItemPathQuery<'query, D, I>,
        crate_items: &CrateItemQuery<'query, D, I>,
        cache: &TraitSelectionCache,
        roots: &ChalkProgramRoots,
    ) -> Result<(), I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        let scope = ChalkProgramScope::discover(item_paths, crate_items, cache, roots, self)?;

        // Associated-type declarations must exist before lowering any trait/impl predicates that
        // can mention their projection IDs.
        let mut associated_ty_ids_by_trait = HashMap::new();
        for &trait_ref in &scope.definitions.traits {
            if !scope.trait_headers.contains_key(&trait_ref) {
                continue;
            }
            let Some(trait_data) = crate_items.items().trait_data(trait_ref)? else {
                continue;
            };
            let generics = item_paths
                .generics()
                .generics(GenericDefRef::Trait(trait_ref))?;
            let binders = GenericBinderEnv::for_generics(&generics);
            let lowerer = ChalkLowerer::new(&binders);
            let associated_ty_ids =
                self.collect_trait_associated_tys(item_paths, &lowerer, trait_ref, trait_data)?;
            associated_ty_ids_by_trait.insert(trait_ref, associated_ty_ids);
        }

        // Once every associated-type ID exists, trait predicates can safely refer to them.
        for &trait_ref in &scope.definitions.traits {
            let Some(header) = scope.trait_headers.get(&trait_ref) else {
                continue;
            };
            let generics = item_paths
                .generics()
                .generics(GenericDefRef::Trait(trait_ref))?;
            let binders = GenericBinderEnv::for_generics(&generics);
            let lowerer = ChalkLowerer::new(&binders).with_associated_tys(&self.associated_tys);
            let associated_ty_ids = associated_ty_ids_by_trait
                .get(&trait_ref)
                .cloned()
                .unwrap_or_default();
            let Some(datum) = lowerer.trait_datum(header, associated_ty_ids) else {
                continue;
            };
            self.ensure_trait_datum_adts(item_paths, &datum)?;
            self.trait_arities
                .insert(trait_ref, datum.binders.len(INTER));
            self.traits.insert(trait_ref, Arc::new(datum));
        }

        // Impl datums use the same registry for their predicates and associated-type values.
        for &impl_ref in &scope.impls {
            let Some(header) = scope.impl_headers.get(&impl_ref) else {
                continue;
            };
            let Some(impl_data) = crate_items.items().impl_data(impl_ref)? else {
                continue;
            };
            let Some(trait_ref) = impl_data.resolved_trait_ref.as_option().copied() else {
                continue;
            };
            let generics = item_paths
                .generics()
                .generics(GenericDefRef::Impl(impl_ref))?;
            let binders = GenericBinderEnv::for_generics(&generics);
            let associated_ty_value_ids =
                self.collect_impl_associated_ty_values(item_paths, &binders, impl_ref, impl_data)?;
            let lowerer = ChalkLowerer::new(&binders).with_associated_tys(&self.associated_tys);
            let Some(datum) = lowerer.impl_datum(header, associated_ty_value_ids) else {
                continue;
            };
            self.ensure_impl_datum_adts(item_paths, &datum)?;
            self.trait_arities.entry(trait_ref).or_insert_with(|| {
                datum
                    .binders
                    .skip_binders()
                    .trait_ref
                    .substitution
                    .len(INTER)
            });
            self.impls_by_trait
                .entry(trait_ref)
                .or_default()
                .push(impl_ref);
            self.impls.insert(impl_ref, Arc::new(datum));
        }

        // Opaque bounds are declaration predicates. Materialize them in the solver program while
        // keeping the opaque identity itself compact and independent from those predicates.
        for &opaque_ref in &scope.definitions.opaque_tys {
            let Some((opaque, bounds)) = scope.opaque_bounds.get(&opaque_ref) else {
                continue;
            };
            let generics = item_paths.generics().generics(opaque.opaque.owner)?;
            let binders = GenericBinderEnv::for_generics(&generics);
            let lowerer = ChalkLowerer::new(&binders).with_associated_tys(&self.associated_tys);
            let Some(datum) = lowerer.opaque_ty_datum(opaque, bounds) else {
                continue;
            };
            self.opaque_tys.insert(opaque.opaque, Arc::new(datum));
        }

        self.materialized_traits
            .extend(scope.definitions.traits.iter().copied());
        self.materialized_opaque_owners
            .extend(scope.loaded_opaque_owners.iter().copied());
        Ok(())
    }

    fn collect_trait_associated_tys<'query, D, I>(
        &mut self,
        item_paths: &ItemPathQuery<'query, D, I>,
        lowerer: &ChalkLowerer<'_>,
        trait_ref: TraitDefRef,
        trait_data: &rg_semantic_ir::TraitData,
    ) -> Result<Vec<AssocTypeId<RgChalkInterner>>, I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        let mut associated_ty_ids = Vec::new();
        for item in &trait_data.items {
            let AssocItemId::TypeAlias(type_alias_id) = item else {
                continue;
            };
            let type_alias_ref = TypeAliasRef {
                origin: trait_ref.origin,
                id: *type_alias_id,
            };
            let Some(type_alias_data) = item_paths.items().type_alias_data(type_alias_ref)? else {
                continue;
            };
            let Some(datum) =
                lowerer.associated_ty_datum(trait_ref, type_alias_ref, type_alias_data)
            else {
                continue;
            };

            self.associated_ty_by_trait_name
                .insert((trait_ref, type_alias_data.name.clone()), type_alias_ref);
            self.associated_tys.insert(type_alias_ref, Arc::new(datum));
            associated_ty_ids.push(chalk_assoc_type_id(type_alias_ref));
        }
        Ok(associated_ty_ids)
    }

    fn collect_impl_associated_ty_values<'query, D, I>(
        &mut self,
        item_paths: &ItemPathQuery<'query, D, I>,
        binders: &GenericBinderEnv,
        impl_ref: ImplRef,
        impl_data: &ImplData,
    ) -> Result<Vec<AssociatedTyValueId<RgChalkInterner>>, I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        let Some(trait_ref) = impl_data.resolved_trait_ref.as_option().copied() else {
            return Ok(Vec::new());
        };

        let mut associated_ty_value_ids = Vec::new();
        for item in &impl_data.items {
            let AssocItemId::TypeAlias(type_alias_id) = item else {
                continue;
            };
            let type_alias_ref = TypeAliasRef {
                origin: impl_ref.origin,
                id: *type_alias_id,
            };
            let Some(type_alias_data) = item_paths.items().type_alias_data(type_alias_ref)? else {
                continue;
            };
            let Some(associated_ty_ref) = self
                .associated_ty_by_trait_name
                .get(&(trait_ref, type_alias_data.name.clone()))
                .copied()
            else {
                continue;
            };
            let Some(ty) = SemanticSignatureQuery::type_alias_ty_from(item_paths, type_alias_ref)?
            else {
                continue;
            };
            let value = ChalkLowerer::new(binders)
                .with_associated_tys(&self.associated_tys)
                .associated_ty_value(impl_ref, associated_ty_ref, type_alias_data, &ty);
            let Some(value) = value else {
                continue;
            };

            self.ensure_ty_adts(item_paths, &value.value.skip_binders().ty)?;
            self.associated_ty_value_by_impl
                .insert((impl_ref, associated_ty_ref), type_alias_ref);
            self.associated_ty_values
                .insert(type_alias_ref, Arc::new(value));
            associated_ty_value_ids.push(chalk_assoc_type_value_id(type_alias_ref));
        }
        Ok(associated_ty_value_ids)
    }

    fn ensure_adt<'query, D, I>(
        &mut self,
        item_paths: &ItemPathQuery<'query, D, I>,
        type_def: TypeDefRef,
    ) -> Result<(), I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        if self.adts.contains_key(&type_def) {
            return Ok(());
        }
        let generics = item_paths
            .generics()
            .generics(GenericDefRef::TypeDef(type_def))?;
        let datum = adt_datum(type_def, Some(&generics));
        let arity = datum.binders.len(INTER);
        self.adt_variances.insert(
            type_def,
            Variances::from_iter(INTER, (0..arity).map(|_| Variance::Invariant)),
        );
        self.adts.insert(type_def, Arc::new(datum));
        Ok(())
    }

    fn ensure_trait_datum_adts<'query, D, I>(
        &mut self,
        item_paths: &ItemPathQuery<'query, D, I>,
        datum: &TraitDatum<RgChalkInterner>,
    ) -> Result<(), I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        // Chalk may ask for ADT metadata while solving any lowered type, not just the root
        // impl `Self` type. Register the ADTs that appear in substitutions up front so generic
        // shapes like `Vec<User>` keep their real arity and variance slots.
        for clause in &datum.binders.skip_binders().where_clauses {
            self.ensure_where_clause_adts(item_paths, clause.skip_binders())?;
        }
        Ok(())
    }

    fn ensure_impl_datum_adts<'query, D, I>(
        &mut self,
        item_paths: &ItemPathQuery<'query, D, I>,
        datum: &ImplDatum<RgChalkInterner>,
    ) -> Result<(), I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        let bound = datum.binders.skip_binders();
        self.ensure_trait_ref_adts(item_paths, &bound.trait_ref)?;
        for clause in &bound.where_clauses {
            self.ensure_where_clause_adts(item_paths, clause.skip_binders())?;
        }
        Ok(())
    }

    fn ensure_where_clause_adts<'query, D, I>(
        &mut self,
        item_paths: &ItemPathQuery<'query, D, I>,
        clause: &WhereClause<RgChalkInterner>,
    ) -> Result<(), I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        match clause {
            WhereClause::Implemented(trait_ref) => {
                self.ensure_trait_ref_adts(item_paths, trait_ref)?;
            }
            WhereClause::AliasEq(alias_eq) => {
                self.ensure_alias_ty_adts(item_paths, &alias_eq.alias)?;
                self.ensure_ty_adts(item_paths, &alias_eq.ty)?;
            }
            WhereClause::LifetimeOutlives(_) => {}
            WhereClause::TypeOutlives(type_outlives) => {
                self.ensure_ty_adts(item_paths, &type_outlives.ty)?;
            }
        }
        Ok(())
    }

    fn ensure_trait_ref_adts<'query, D, I>(
        &mut self,
        item_paths: &ItemPathQuery<'query, D, I>,
        trait_ref: &chalk_ir::TraitRef<RgChalkInterner>,
    ) -> Result<(), I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        for ty in trait_ref.type_parameters(INTER) {
            self.ensure_ty_adts(item_paths, &ty)?;
        }
        Ok(())
    }

    fn ensure_substitution_adts<'query, D, I>(
        &mut self,
        item_paths: &ItemPathQuery<'query, D, I>,
        substitution: &Substitution<RgChalkInterner>,
    ) -> Result<(), I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        for ty in substitution.type_parameters(INTER) {
            self.ensure_ty_adts(item_paths, &ty)?;
        }
        Ok(())
    }

    fn ensure_alias_ty_adts<'query, D, I>(
        &mut self,
        item_paths: &ItemPathQuery<'query, D, I>,
        alias: &AliasTy<RgChalkInterner>,
    ) -> Result<(), I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        match alias {
            AliasTy::Projection(projection) => {
                self.ensure_substitution_adts(item_paths, &projection.substitution)?;
            }
            AliasTy::Opaque(opaque) => {
                self.ensure_substitution_adts(item_paths, &opaque.substitution)?;
            }
        }
        Ok(())
    }

    fn ensure_ty_adts<'query, D, I>(
        &mut self,
        item_paths: &ItemPathQuery<'query, D, I>,
        ty: &Ty<RgChalkInterner>,
    ) -> Result<(), I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        match ty.kind(INTER) {
            TyKind::Adt(adt_id, substitution) => {
                self.ensure_adt(item_paths, adt_id.0)?;
                self.ensure_substitution_adts(item_paths, substitution)?;
            }
            TyKind::AssociatedType(_, substitution)
            | TyKind::Tuple(_, substitution)
            | TyKind::OpaqueType(_, substitution)
            | TyKind::FnDef(_, substitution)
            | TyKind::Closure(_, substitution)
            | TyKind::Coroutine(_, substitution)
            | TyKind::CoroutineWitness(_, substitution) => {
                self.ensure_substitution_adts(item_paths, substitution)?;
            }
            TyKind::Array(inner, _)
            | TyKind::Slice(inner)
            | TyKind::Raw(_, inner)
            | TyKind::Ref(_, _, inner) => {
                self.ensure_ty_adts(item_paths, inner)?;
            }
            TyKind::Alias(alias) => {
                self.ensure_alias_ty_adts(item_paths, alias)?;
            }
            TyKind::Function(pointer) => {
                self.ensure_substitution_adts(item_paths, &pointer.substitution.0)?;
            }
            TyKind::Scalar(_)
            | TyKind::Str
            | TyKind::Never
            | TyKind::Foreign(_)
            | TyKind::Error
            | TyKind::Placeholder(_)
            | TyKind::Dyn(_)
            | TyKind::BoundVar(_)
            | TyKind::InferenceVar(_, _) => {}
        }
        Ok(())
    }
}
