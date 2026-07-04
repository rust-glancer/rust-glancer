use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use chalk_engine::solve::SLGSolver;
use chalk_ir::cast::Cast;
use chalk_ir::{
    AdtId, AliasTy, AssocTypeId, Binders, CanonicalVarKinds, ClosureId, CoroutineId, DomainGoal,
    FnDefId, GenericArg, GenericArgData, GoalData, Normalize, OpaqueTyId, ProgramClause,
    ProgramClauses, QuantifierKind, Substitution, Ty, TyKind, UnificationDatabase, Variance,
    Variances, WhereClause,
};
use chalk_solve::ext::GoalExt;
use chalk_solve::rust_ir::{
    AdtRepr, AdtSizeAlign, AssociatedTyDatum, AssociatedTyDatumBound, AssociatedTyValue,
    AssociatedTyValueBound, AssociatedTyValueId, ClosureKind, CoroutineDatum,
    CoroutineInputOutputDatum, CoroutineWitnessDatum, FnDefDatum, FnDefDatumBound,
    FnDefInputsAndOutputDatum, ImplDatum, Movability, OpaqueTyDatum, OpaqueTyDatumBound, Polarity,
    TraitDatum, WellKnownAssocType, WellKnownTrait,
};
use chalk_solve::{RustIrDatabase, Solver};
use rg_ir_model::hir::items::ImplData;
use rg_ir_model::{
    AssocItemId, ImplRef, TraitApplicability, TraitImplRef, TraitRef, TypeAliasRef, TypeDefRef,
};
use rg_ir_storage::{DefMapSource, ItemStoreSource, TargetItemQuery, TypePathContext};
use rg_text::Name;

use super::interner::{ChalkDefId, RgChalkInterner};
use super::lower::{
    ChalkLowerer, GenericBinderEnv, TraitNameIndex, adt_datum, chalk_assoc_type_id,
    chalk_assoc_type_value_id, chalk_impl_id, chalk_trait_id, stub_trait_datum, unit_ty,
};
use super::projection::ProjectionAnswerVars;
use super::raise;
use crate::ItemPathQuery;
use crate::inference::{InferTy, InferTypeSubst, InferenceTable};
use crate::trait_selection::AssocProjectionResult;

const INTER: RgChalkInterner = RgChalkInterner;
const SOLVER_MAX_SIZE: usize = 32;
const UNKNOWN_ADT_VARIANCE_SLOTS: usize = 32;

pub(crate) struct ChalkTraitSolver {
    program: ChalkProgram,
    impl_bounds_solver: SLGSolver<RgChalkInterner>,
    assoc_projection_solver: SLGSolver<RgChalkInterner>,
}

impl ChalkTraitSolver {
    pub(crate) fn new<'query, D, I>(
        item_paths: &ItemPathQuery<'query, D, I>,
        target_items: &TargetItemQuery<'query, D, I>,
    ) -> Result<Self, I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        crate::profile::metric::PROGRAM_BUILDS.inc();
        let started = Instant::now();
        let program = ChalkProgram::build(item_paths, target_items);
        crate::profile::metric::PROGRAM_BUILD_TIME.record(started.elapsed());
        program.map(|program| Self {
            program,
            // Impl-bound checks only need to know whether a candidate obligation has at least one
            // answer, while associated projection needs the substitution for the projected type.
            // Keep separate SLG forests so the two goal modes do not share different answer limits.
            impl_bounds_solver: SLGSolver::new(SOLVER_MAX_SIZE, Some(1)),
            assoc_projection_solver: SLGSolver::new(SOLVER_MAX_SIZE, None),
        })
    }

    pub(crate) fn impl_bounds_applicability<'query, D, I>(
        &mut self,
        item_paths: &ItemPathQuery<'query, D, I>,
        trait_impl: TraitImplRef,
        impl_data: &ImplData,
        subst: &InferTypeSubst,
        table: &InferenceTable,
    ) -> Option<TraitApplicability>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        let binders = GenericBinderEnv::for_impl(&impl_data.generics)?;
        let lowerer = ChalkLowerer::new(
            item_paths,
            &self.program.trait_names,
            TypePathContext {
                module: impl_data.owner,
                impl_ref: Some(trait_impl.impl_ref),
            },
            &binders,
        );
        let goals = lowerer.candidate_where_goals(impl_data, subst, table)?;
        if goals.is_empty() {
            return Some(TraitApplicability::Yes);
        }

        let mut applicability = TraitApplicability::Yes;
        for goal in goals {
            let canonical_goal = goal.into_closed_goal(INTER);
            crate::profile::metric::SOLVER_GOALS.inc();
            let started = Instant::now();
            let solution = self
                .impl_bounds_solver
                .solve(&self.program, &canonical_goal);
            crate::profile::metric::SOLVER_GOAL_TIME_BY_KIND
                .record("impl_bounds", started.elapsed());
            let solution = solution?;
            if solution.is_ambig() {
                crate::profile::metric::SOLVER_AMBIGUOUS_GOALS.inc();
                applicability = applicability.and(TraitApplicability::Maybe);
            }
        }
        Some(applicability)
    }

    pub(crate) fn normalize_assoc_type<'query, D, I>(
        &mut self,
        item_paths: &ItemPathQuery<'query, D, I>,
        context: TypePathContext,
        goal: &crate::trait_selection::TraitGoal,
        assoc_name: &str,
        table: &InferenceTable,
    ) -> Option<AssocProjectionResult>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        let assoc_type_ref = self.program.associated_ty_ref(goal.trait_ref, assoc_name)?;
        let binders = GenericBinderEnv::empty();
        let lowerer = ChalkLowerer::new(item_paths, &self.program.trait_names, context, &binders);
        let projection = lowerer.projection_alias(assoc_type_ref, goal, table)?;
        // Ask Chalk for the one existential result type in:
        //
        // `Normalize(<Self as Trait>::Assoc -> ?Result)`
        //
        // The binder also includes any ordinary project inference variables used by the receiver
        // goal. If Chalk answers `?Result = ?T`, the decoder maps that bound var back to the same
        // rust-glancer `InferTy::Var`, then commits only the concrete equalities it can decode.
        let normalize = Normalize {
            alias: projection.alias,
            ty: projection.variables.result_ty(),
        };
        let goal = GoalData::Quantified(
            QuantifierKind::Exists,
            Binders::new(
                projection.variables.variable_kinds_with_result(),
                DomainGoal::Normalize(normalize).cast(INTER),
            ),
        )
        .intern(INTER);

        let canonical_goal = goal.into_peeled_goal(INTER);
        crate::profile::metric::SOLVER_GOALS.inc();
        let started = Instant::now();
        let solution = self
            .assoc_projection_solver
            .solve(&self.program, &canonical_goal)?;
        crate::profile::metric::SOLVER_GOAL_TIME_BY_KIND
            .record("assoc_projection", started.elapsed());
        if solution.is_ambig() {
            crate::profile::metric::SOLVER_AMBIGUOUS_GOALS.inc();
        }

        let applicability = if solution.is_ambig() {
            TraitApplicability::Maybe
        } else {
            TraitApplicability::Yes
        };
        let subst = solution.definite_subst(INTER)?;
        let subst_args = subst.value.subst.as_slice(INTER);
        let mut table = table.clone();

        let answer_vars = ProjectionAnswerVars::from_subst_args(&projection.variables, subst_args)?;

        for (index, var) in projection.variables.iter_project_vars() {
            let project_arg = subst_args.get(index)?;
            let GenericArgData::Ty(project_ty) = project_arg.data(INTER) else {
                return None;
            };
            if let Some(evidence) = raise::infer_ty_from_chalk_projection(
                project_ty,
                &projection.variables,
                &answer_vars,
            ) {
                table.try_unify(&InferTy::Var(var), &evidence).ok()?;
            }
        }

        let projected_arg = subst_args.get(projection.variables.result_index())?;
        let GenericArgData::Ty(projected_ty) = projected_arg.data(INTER) else {
            return None;
        };
        let ty = raise::infer_ty_from_chalk_projection(
            projected_ty,
            &projection.variables,
            &answer_vars,
        )?;
        Some(AssocProjectionResult {
            ty,
            applicability,
            table,
        })
    }
}

#[derive(Debug)]
struct ChalkProgram {
    trait_names: TraitNameIndex,
    traits: HashMap<TraitRef, Arc<TraitDatum<RgChalkInterner>>>,
    trait_arities: HashMap<TraitRef, usize>,
    associated_tys: HashMap<TypeAliasRef, Arc<AssociatedTyDatum<RgChalkInterner>>>,
    associated_ty_by_trait_name: HashMap<(TraitRef, Name), TypeAliasRef>,
    associated_ty_values: HashMap<TypeAliasRef, Arc<AssociatedTyValue<RgChalkInterner>>>,
    associated_ty_value_by_impl: HashMap<(ImplRef, TypeAliasRef), TypeAliasRef>,
    adts: HashMap<TypeDefRef, Arc<chalk_solve::rust_ir::AdtDatum<RgChalkInterner>>>,
    adt_variances: HashMap<TypeDefRef, Variances<RgChalkInterner>>,
    impls: HashMap<rg_ir_model::ImplRef, Arc<ImplDatum<RgChalkInterner>>>,
    impls_by_trait: HashMap<TraitRef, Vec<rg_ir_model::ImplRef>>,
}

impl ChalkProgram {
    fn build<'query, D, I>(
        item_paths: &ItemPathQuery<'query, D, I>,
        target_items: &TargetItemQuery<'query, D, I>,
    ) -> Result<Self, I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        let mut program = Self {
            trait_names: TraitNameIndex::new(),
            traits: HashMap::new(),
            trait_arities: HashMap::new(),
            associated_tys: HashMap::new(),
            associated_ty_by_trait_name: HashMap::new(),
            associated_ty_values: HashMap::new(),
            associated_ty_value_by_impl: HashMap::new(),
            adts: HashMap::new(),
            adt_variances: HashMap::new(),
            impls: HashMap::new(),
            impls_by_trait: HashMap::new(),
        };

        let visible_stores = target_items.visible_stores()?;
        for store in &visible_stores {
            for (trait_ref, trait_data) in store.traits_with_refs() {
                program.trait_names.push(trait_data.name.clone(), trait_ref);
            }
        }

        let trait_names = program.trait_names.clone();
        for store in &visible_stores {
            for (trait_ref, trait_data) in store.traits_with_refs() {
                let binders = GenericBinderEnv::empty();
                let lowerer = ChalkLowerer::new(
                    item_paths,
                    &trait_names,
                    TypePathContext::module(trait_data.owner),
                    &binders,
                );
                let associated_ty_ids = program
                    .collect_trait_associated_tys(item_paths, &lowerer, trait_ref, trait_data)?;
                let Some(datum) = lowerer.trait_datum(
                    trait_ref,
                    &trait_data.generics,
                    &trait_data.super_traits,
                    associated_ty_ids,
                ) else {
                    continue;
                };
                program.ensure_trait_datum_adts(target_items, &datum)?;
                program
                    .trait_arities
                    .insert(trait_ref, datum.binders.len(INTER));
                program.traits.insert(trait_ref, Arc::new(datum));
            }
        }

        let trait_names = program.trait_names.clone();
        let associated_ty_by_trait_name = program.associated_ty_by_trait_name.clone();
        for store in &visible_stores {
            for (impl_ref, impl_data) in store.impls_with_refs() {
                let Some(trait_ref) = impl_data.resolved_trait_ref.as_option().copied() else {
                    continue;
                };
                let binders = GenericBinderEnv::empty();
                let lowerer = ChalkLowerer::new(
                    item_paths,
                    &trait_names,
                    TypePathContext {
                        module: impl_data.owner,
                        impl_ref: Some(impl_ref),
                    },
                    &binders,
                )
                .with_associated_tys(&associated_ty_by_trait_name);
                let associated_ty_value_ids = program.collect_impl_associated_ty_values(
                    item_paths,
                    target_items,
                    &lowerer,
                    impl_ref,
                    impl_data,
                )?;
                let Some(datum) = lowerer.impl_datum(impl_data, associated_ty_value_ids) else {
                    continue;
                };
                program.ensure_impl_datum_adts(target_items, &datum)?;
                program.trait_arities.entry(trait_ref).or_insert_with(|| {
                    datum
                        .binders
                        .skip_binders()
                        .trait_ref
                        .substitution
                        .len(INTER)
                });
                program
                    .impls_by_trait
                    .entry(trait_ref)
                    .or_default()
                    .push(impl_ref);
                program.impls.insert(impl_ref, Arc::new(datum));
            }
        }

        Ok(program)
    }

    fn collect_trait_associated_tys<'query, D, I>(
        &mut self,
        item_paths: &ItemPathQuery<'query, D, I>,
        lowerer: &ChalkLowerer<'_, 'query, D, I>,
        trait_ref: TraitRef,
        trait_data: &rg_ir_model::hir::items::TraitData,
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
            let Some(datum) = lowerer.associated_ty_datum(
                trait_ref,
                type_alias_ref,
                type_alias_data,
                &trait_data.generics,
            ) else {
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
        target_items: &TargetItemQuery<'query, D, I>,
        lowerer: &ChalkLowerer<'_, 'query, D, I>,
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
            let Some(value) = lowerer.associated_ty_value(
                impl_ref,
                associated_ty_ref,
                type_alias_data,
                impl_data,
            ) else {
                continue;
            };

            self.ensure_ty_adts(target_items, &value.value.skip_binders().ty)?;
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
        target_items: &TargetItemQuery<'query, D, I>,
        type_def: TypeDefRef,
    ) -> Result<(), I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        if self.adts.contains_key(&type_def) {
            return Ok(());
        }
        let generics = target_items.items().generic_params_for_type_def(type_def)?;
        let Some(datum) = adt_datum(type_def, generics) else {
            return Ok(());
        };
        let arity = datum.binders.len(INTER);
        self.adt_variances.insert(
            type_def,
            Variances::from_iter(INTER, (0..arity).map(|_| Variance::Invariant)),
        );
        self.adts.insert(type_def, Arc::new(datum));
        Ok(())
    }

    fn associated_ty_ref(&self, trait_ref: TraitRef, assoc_name: &str) -> Option<TypeAliasRef> {
        self.associated_ty_by_trait_name
            .get(&(trait_ref, Name::new(assoc_name)))
            .copied()
    }

    fn ensure_trait_datum_adts<'query, D, I>(
        &mut self,
        target_items: &TargetItemQuery<'query, D, I>,
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
            self.ensure_where_clause_adts(target_items, clause.skip_binders())?;
        }
        Ok(())
    }

    fn ensure_impl_datum_adts<'query, D, I>(
        &mut self,
        target_items: &TargetItemQuery<'query, D, I>,
        datum: &ImplDatum<RgChalkInterner>,
    ) -> Result<(), I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        let bound = datum.binders.skip_binders();
        self.ensure_trait_ref_adts(target_items, &bound.trait_ref)?;
        for clause in &bound.where_clauses {
            self.ensure_where_clause_adts(target_items, clause.skip_binders())?;
        }
        Ok(())
    }

    fn ensure_where_clause_adts<'query, D, I>(
        &mut self,
        target_items: &TargetItemQuery<'query, D, I>,
        clause: &WhereClause<RgChalkInterner>,
    ) -> Result<(), I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        match clause {
            WhereClause::Implemented(trait_ref) => {
                self.ensure_trait_ref_adts(target_items, trait_ref)?;
            }
            WhereClause::AliasEq(alias_eq) => {
                self.ensure_alias_ty_adts(target_items, &alias_eq.alias)?;
                self.ensure_ty_adts(target_items, &alias_eq.ty)?;
            }
            WhereClause::LifetimeOutlives(_) => {}
            WhereClause::TypeOutlives(type_outlives) => {
                self.ensure_ty_adts(target_items, &type_outlives.ty)?;
            }
        }
        Ok(())
    }

    fn ensure_trait_ref_adts<'query, D, I>(
        &mut self,
        target_items: &TargetItemQuery<'query, D, I>,
        trait_ref: &chalk_ir::TraitRef<RgChalkInterner>,
    ) -> Result<(), I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        for ty in trait_ref.type_parameters(INTER) {
            self.ensure_ty_adts(target_items, &ty)?;
        }
        Ok(())
    }

    fn ensure_substitution_adts<'query, D, I>(
        &mut self,
        target_items: &TargetItemQuery<'query, D, I>,
        substitution: &Substitution<RgChalkInterner>,
    ) -> Result<(), I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        for ty in substitution.type_parameters(INTER) {
            self.ensure_ty_adts(target_items, &ty)?;
        }
        Ok(())
    }

    fn ensure_alias_ty_adts<'query, D, I>(
        &mut self,
        target_items: &TargetItemQuery<'query, D, I>,
        alias: &AliasTy<RgChalkInterner>,
    ) -> Result<(), I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        match alias {
            AliasTy::Projection(projection) => {
                self.ensure_substitution_adts(target_items, &projection.substitution)?;
            }
            AliasTy::Opaque(opaque) => {
                self.ensure_substitution_adts(target_items, &opaque.substitution)?;
            }
        }
        Ok(())
    }

    fn ensure_ty_adts<'query, D, I>(
        &mut self,
        target_items: &TargetItemQuery<'query, D, I>,
        ty: &Ty<RgChalkInterner>,
    ) -> Result<(), I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        match ty.kind(INTER) {
            TyKind::Adt(adt_id, substitution) => {
                self.ensure_adt(target_items, adt_id.0)?;
                self.ensure_substitution_adts(target_items, substitution)?;
            }
            TyKind::AssociatedType(_, substitution)
            | TyKind::Tuple(_, substitution)
            | TyKind::OpaqueType(_, substitution)
            | TyKind::FnDef(_, substitution)
            | TyKind::Closure(_, substitution)
            | TyKind::Coroutine(_, substitution)
            | TyKind::CoroutineWitness(_, substitution) => {
                self.ensure_substitution_adts(target_items, substitution)?;
            }
            TyKind::Array(inner, _)
            | TyKind::Slice(inner)
            | TyKind::Raw(_, inner)
            | TyKind::Ref(_, _, inner) => {
                self.ensure_ty_adts(target_items, inner)?;
            }
            TyKind::Alias(alias) => {
                self.ensure_alias_ty_adts(target_items, alias)?;
            }
            TyKind::Scalar(_)
            | TyKind::Str
            | TyKind::Never
            | TyKind::Foreign(_)
            | TyKind::Error
            | TyKind::Placeholder(_)
            | TyKind::Dyn(_)
            | TyKind::Function(_)
            | TyKind::BoundVar(_)
            | TyKind::InferenceVar(_, _) => {}
        }
        Ok(())
    }

    fn stub_trait(&self, trait_ref: TraitRef) -> Arc<TraitDatum<RgChalkInterner>> {
        let arity = self.trait_arities.get(&trait_ref).copied().unwrap_or(1);
        Arc::new(stub_trait_datum(trait_ref, arity))
    }

    fn stub_adt(
        &self,
        type_def: TypeDefRef,
    ) -> Arc<chalk_solve::rust_ir::AdtDatum<RgChalkInterner>> {
        Arc::new(adt_datum(type_def, None).expect("stub ADT without generics should always lower"))
    }
}

impl UnificationDatabase<RgChalkInterner> for ChalkProgram {
    fn fn_def_variance(&self, _fn_def_id: FnDefId<RgChalkInterner>) -> Variances<RgChalkInterner> {
        Variances::empty(INTER)
    }

    fn adt_variance(&self, adt_id: AdtId<RgChalkInterner>) -> Variances<RgChalkInterner> {
        self.adt_variances
            .get(&adt_id.0)
            .cloned()
            .unwrap_or_else(|| {
                Variances::from_iter(
                    INTER,
                    (0..UNKNOWN_ADT_VARIANCE_SLOTS).map(|_| Variance::Invariant),
                )
            })
    }
}

impl RustIrDatabase<RgChalkInterner> for ChalkProgram {
    fn custom_clauses(&self) -> Vec<ProgramClause<RgChalkInterner>> {
        Vec::new()
    }

    fn associated_ty_data(
        &self,
        ty: AssocTypeId<RgChalkInterner>,
    ) -> Arc<AssociatedTyDatum<RgChalkInterner>> {
        if let ChalkDefId::AssocType(type_alias_ref) = ty.0
            && let Some(datum) = self.associated_tys.get(&type_alias_ref)
        {
            return datum.clone();
        }

        Arc::new(AssociatedTyDatum {
            trait_id: chalk_trait_id(TraitRef {
                origin: rg_ir_model::DefMapRef::Target(rg_ir_model::TargetRef {
                    package: rg_ir_model::PackageSlot(0),
                    target: rg_ir_model::TargetId(0),
                }),
                id: rg_ir_model::TraitId(0),
            }),
            id: ty,
            name: "Unsupported".to_owned(),
            binders: Binders::empty(
                INTER,
                AssociatedTyDatumBound {
                    bounds: Vec::new(),
                    where_clauses: Vec::new(),
                },
            ),
        })
    }

    fn trait_datum(
        &self,
        trait_id: chalk_ir::TraitId<RgChalkInterner>,
    ) -> Arc<TraitDatum<RgChalkInterner>> {
        let ChalkDefId::Trait(trait_ref) = trait_id.0 else {
            return self.stub_trait(TraitRef {
                origin: rg_ir_model::DefMapRef::Target(rg_ir_model::TargetRef {
                    package: rg_ir_model::PackageSlot(0),
                    target: rg_ir_model::TargetId(0),
                }),
                id: rg_ir_model::TraitId(0),
            });
        };
        self.traits
            .get(&trait_ref)
            .cloned()
            .unwrap_or_else(|| self.stub_trait(trait_ref))
    }

    fn adt_datum(
        &self,
        adt_id: AdtId<RgChalkInterner>,
    ) -> Arc<chalk_solve::rust_ir::AdtDatum<RgChalkInterner>> {
        self.adts
            .get(&adt_id.0)
            .cloned()
            .unwrap_or_else(|| self.stub_adt(adt_id.0))
    }

    fn coroutine_datum(
        &self,
        coroutine_id: CoroutineId<RgChalkInterner>,
    ) -> Arc<CoroutineDatum<RgChalkInterner>> {
        let _ = coroutine_id;
        Arc::new(CoroutineDatum {
            movability: Movability::Static,
            input_output: Binders::empty(
                INTER,
                CoroutineInputOutputDatum {
                    resume_type: unit_ty(),
                    yield_type: unit_ty(),
                    return_type: unit_ty(),
                    upvars: Vec::new(),
                },
            ),
        })
    }

    fn coroutine_witness_datum(
        &self,
        coroutine_id: CoroutineId<RgChalkInterner>,
    ) -> Arc<CoroutineWitnessDatum<RgChalkInterner>> {
        let _ = coroutine_id;
        Arc::new(CoroutineWitnessDatum {
            inner_types: Binders::empty(
                INTER,
                chalk_solve::rust_ir::CoroutineWitnessExistential {
                    types: Binders::empty(INTER, Vec::new()),
                },
            ),
        })
    }

    fn adt_repr(&self, _id: AdtId<RgChalkInterner>) -> Arc<AdtRepr<RgChalkInterner>> {
        Arc::new(AdtRepr {
            c: false,
            packed: false,
            int: None,
        })
    }

    fn adt_size_align(&self, _id: AdtId<RgChalkInterner>) -> Arc<AdtSizeAlign> {
        Arc::new(AdtSizeAlign::from_one_zst(false))
    }

    fn fn_def_datum(
        &self,
        fn_def_id: FnDefId<RgChalkInterner>,
    ) -> Arc<FnDefDatum<RgChalkInterner>> {
        Arc::new(FnDefDatum {
            id: fn_def_id,
            sig: chalk_ir::FnSig {
                abi: (),
                safety: chalk_ir::Safety::Safe,
                variadic: false,
            },
            binders: Binders::empty(
                INTER,
                FnDefDatumBound {
                    inputs_and_output: Binders::empty(
                        INTER,
                        FnDefInputsAndOutputDatum {
                            argument_types: Vec::new(),
                            return_type: unit_ty(),
                        },
                    ),
                    where_clauses: Vec::new(),
                },
            ),
        })
    }

    fn impl_datum(
        &self,
        impl_id: chalk_ir::ImplId<RgChalkInterner>,
    ) -> Arc<ImplDatum<RgChalkInterner>> {
        let ChalkDefId::Impl(impl_ref) = impl_id.0 else {
            return Arc::new(ImplDatum {
                polarity: Polarity::Negative,
                binders: Binders::empty(
                    INTER,
                    chalk_solve::rust_ir::ImplDatumBound {
                        trait_ref: chalk_solve::rust_ir::TraitBound {
                            trait_id: chalk_trait_id(TraitRef {
                                origin: rg_ir_model::DefMapRef::Target(rg_ir_model::TargetRef {
                                    package: rg_ir_model::PackageSlot(0),
                                    target: rg_ir_model::TargetId(0),
                                }),
                                id: rg_ir_model::TraitId(0),
                            }),
                            args_no_self: Vec::new(),
                        }
                        .as_trait_ref(INTER, unit_ty()),
                        where_clauses: Vec::new(),
                    },
                ),
                impl_type: chalk_solve::rust_ir::ImplType::External,
                associated_ty_value_ids: Vec::new(),
            });
        };
        self.impls.get(&impl_ref).cloned().unwrap_or_else(|| {
            Arc::new(ImplDatum {
                polarity: Polarity::Negative,
                binders: Binders::empty(
                    INTER,
                    chalk_solve::rust_ir::ImplDatumBound {
                        trait_ref: chalk_ir::TraitRef {
                            trait_id: chalk_trait_id(TraitRef {
                                origin: impl_ref.origin,
                                id: rg_ir_model::TraitId(0),
                            }),
                            substitution: Substitution::from_iter(
                                INTER,
                                [chalk_ir::GenericArgData::Ty(unit_ty()).intern(INTER)],
                            ),
                        },
                        where_clauses: Vec::new(),
                    },
                ),
                impl_type: chalk_solve::rust_ir::ImplType::External,
                associated_ty_value_ids: Vec::new(),
            })
        })
    }

    fn associated_ty_from_impl(
        &self,
        impl_id: chalk_ir::ImplId<RgChalkInterner>,
        assoc_type_id: AssocTypeId<RgChalkInterner>,
    ) -> Option<AssociatedTyValueId<RgChalkInterner>> {
        let (ChalkDefId::Impl(impl_ref), ChalkDefId::AssocType(assoc_type_ref)) =
            (impl_id.0, assoc_type_id.0)
        else {
            return None;
        };
        self.associated_ty_value_by_impl
            .get(&(impl_ref, assoc_type_ref))
            .copied()
            .map(chalk_assoc_type_value_id)
    }

    fn associated_ty_value(
        &self,
        id: AssociatedTyValueId<RgChalkInterner>,
    ) -> Arc<AssociatedTyValue<RgChalkInterner>> {
        if let ChalkDefId::AssocTypeValue(type_alias_ref) = id.0
            && let Some(value) = self.associated_ty_values.get(&type_alias_ref)
        {
            return value.clone();
        }

        Arc::new(AssociatedTyValue {
            impl_id: chalk_impl_id(ImplRef {
                origin: rg_ir_model::DefMapRef::Target(rg_ir_model::TargetRef {
                    package: rg_ir_model::PackageSlot(0),
                    target: rg_ir_model::TargetId(0),
                }),
                id: rg_ir_model::ImplId(0),
            }),
            associated_ty_id: AssocTypeId(ChalkDefId::AssocType(TypeAliasRef {
                origin: rg_ir_model::DefMapRef::Target(rg_ir_model::TargetRef {
                    package: rg_ir_model::PackageSlot(0),
                    target: rg_ir_model::TargetId(0),
                }),
                id: rg_ir_model::TypeAliasId(0),
            })),
            value: Binders::empty(INTER, AssociatedTyValueBound { ty: unit_ty() }),
        })
    }

    fn opaque_ty_data(
        &self,
        id: OpaqueTyId<RgChalkInterner>,
    ) -> Arc<OpaqueTyDatum<RgChalkInterner>> {
        Arc::new(OpaqueTyDatum {
            opaque_ty_id: id,
            bound: Binders::empty(
                INTER,
                OpaqueTyDatumBound {
                    bounds: Binders::empty(INTER, Vec::new()),
                    where_clauses: Binders::empty(INTER, Vec::new()),
                },
            ),
        })
    }

    fn hidden_opaque_type(
        &self,
        _id: OpaqueTyId<RgChalkInterner>,
    ) -> chalk_ir::Ty<RgChalkInterner> {
        unit_ty()
    }

    fn impls_for_trait(
        &self,
        trait_id: chalk_ir::TraitId<RgChalkInterner>,
        _parameters: &[GenericArg<RgChalkInterner>],
        _binders: &CanonicalVarKinds<RgChalkInterner>,
    ) -> Vec<chalk_ir::ImplId<RgChalkInterner>> {
        let ChalkDefId::Trait(trait_ref) = trait_id.0 else {
            return Vec::new();
        };
        self.impls_by_trait
            .get(&trait_ref)
            .into_iter()
            .flat_map(|impls| impls.iter().copied())
            .map(chalk_impl_id)
            .collect()
    }

    fn local_impls_to_coherence_check(
        &self,
        trait_id: chalk_ir::TraitId<RgChalkInterner>,
    ) -> Vec<chalk_ir::ImplId<RgChalkInterner>> {
        self.impls_for_trait(trait_id, &[], &CanonicalVarKinds::empty(INTER))
    }

    fn impl_provided_for(
        &self,
        _auto_trait_id: chalk_ir::TraitId<RgChalkInterner>,
        _ty: &TyKind<RgChalkInterner>,
    ) -> bool {
        false
    }

    fn well_known_trait_id(
        &self,
        _well_known_trait: WellKnownTrait,
    ) -> Option<chalk_ir::TraitId<RgChalkInterner>> {
        None
    }

    fn well_known_assoc_type_id(
        &self,
        _assoc_type: WellKnownAssocType,
    ) -> Option<AssocTypeId<RgChalkInterner>> {
        None
    }

    fn program_clauses_for_env(
        &self,
        environment: &chalk_ir::Environment<RgChalkInterner>,
    ) -> ProgramClauses<RgChalkInterner> {
        chalk_solve::program_clauses_for_env(self, environment)
    }

    fn interner(&self) -> RgChalkInterner {
        INTER
    }

    fn is_object_safe(&self, _trait_id: chalk_ir::TraitId<RgChalkInterner>) -> bool {
        false
    }

    fn closure_kind(
        &self,
        _closure_id: ClosureId<RgChalkInterner>,
        _substs: &Substitution<RgChalkInterner>,
    ) -> ClosureKind {
        ClosureKind::FnOnce
    }

    fn closure_inputs_and_output(
        &self,
        _closure_id: ClosureId<RgChalkInterner>,
        _substs: &Substitution<RgChalkInterner>,
    ) -> Binders<FnDefInputsAndOutputDatum<RgChalkInterner>> {
        Binders::empty(
            INTER,
            FnDefInputsAndOutputDatum {
                argument_types: Vec::new(),
                return_type: unit_ty(),
            },
        )
    }

    fn closure_upvars(
        &self,
        _closure_id: ClosureId<RgChalkInterner>,
        _substs: &Substitution<RgChalkInterner>,
    ) -> Binders<chalk_ir::Ty<RgChalkInterner>> {
        Binders::empty(INTER, unit_ty())
    }

    fn closure_fn_substitution(
        &self,
        _closure_id: ClosureId<RgChalkInterner>,
        _substs: &Substitution<RgChalkInterner>,
    ) -> Substitution<RgChalkInterner> {
        Substitution::empty(INTER)
    }

    fn unification_database(&self) -> &dyn UnificationDatabase<RgChalkInterner> {
        self
    }

    fn trait_name(&self, trait_id: chalk_ir::TraitId<RgChalkInterner>) -> String {
        match trait_id.0 {
            ChalkDefId::Trait(trait_ref) => format!("{trait_ref:?}"),
            other => format!("{other:?}"),
        }
    }

    fn adt_name(&self, adt_id: AdtId<RgChalkInterner>) -> String {
        format!("{:?}", adt_id.0)
    }

    fn assoc_type_name(&self, assoc_ty_id: AssocTypeId<RgChalkInterner>) -> String {
        format!("{:?}", assoc_ty_id.0)
    }

    fn opaque_type_name(&self, opaque_ty_id: OpaqueTyId<RgChalkInterner>) -> String {
        format!("{:?}", opaque_ty_id.0)
    }

    fn fn_def_name(&self, fn_def_id: FnDefId<RgChalkInterner>) -> String {
        format!("{:?}", fn_def_id.0)
    }

    fn discriminant_type(
        &self,
        _ty: chalk_ir::Ty<RgChalkInterner>,
    ) -> chalk_ir::Ty<RgChalkInterner> {
        TyKind::Scalar(chalk_ir::Scalar::Uint(chalk_ir::UintTy::Usize)).intern(INTER)
    }
}
