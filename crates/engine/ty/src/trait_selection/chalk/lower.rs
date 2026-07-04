use std::collections::HashMap;

use chalk_ir::cast::Cast;
use chalk_ir::fold::Shift;
use chalk_ir::visit::VisitExt;
use chalk_ir::{
    AdtId, AliasTy, AssocTypeId, BoundVar, ConcreteConst, ConstData, ConstValue, DebruijnIndex,
    DomainGoal, GenericArg, GenericArgData, Goal, LifetimeData, Mutability as ChalkMutability,
    ProjectionTy, QuantifiedWhereClause, Scalar, Substitution, TraitId, TraitRef as ChalkTraitRef,
    TyKind, TyVariableKind, UintTy, VariableKind, VariableKinds, WhereClause,
};
use chalk_solve::rust_ir::{
    AdtDatum, AdtDatumBound, AdtFlags, AdtKind, AdtVariantDatum, AssociatedTyDatum,
    AssociatedTyDatumBound, AssociatedTyValue, AssociatedTyValueBound, AssociatedTyValueId,
    ImplDatum, ImplDatumBound, ImplType, Polarity, TraitDatum, TraitDatumBound, TraitFlags,
};
use rg_ir_model::items::{
    GenericArg as ItemGenericArg, GenericParams, TypeBound, TypeRef, WherePredicate,
};
use rg_ir_model::{
    ImplRef, Mutability, Path, TraitRef, TypeAliasRef, TypeDefId, TypeDefRef, TypePathResolution,
    hir::items::TypeAliasData,
};
use rg_ir_storage::{DefMapSource, ItemStoreSource, TypePathContext};
use rg_text::Name;

use super::interner::{ChalkDefId, RgChalkInterner};
use super::projection::{ProjectionAliasLowering, ProjectionVariableEnv};
use crate::inference::{InferVarKind, InferenceTable, InferenceTypeSubst};
use crate::trait_selection::TraitGoal;
use crate::{
    FloatTy, GenericArg as RgGenericArg, ItemPathQuery, PrimitiveTy, SignedIntTy, Ty as RgTy,
    UnsignedIntTy,
};

pub(super) type ChalkTy = chalk_ir::Ty<RgChalkInterner>;
pub(super) type ChalkGoal = Goal<RgChalkInterner>;

const INTER: RgChalkInterner = RgChalkInterner;

#[derive(Debug, Clone)]
pub(super) struct GenericBinderEnv {
    bindings: Vec<GenericBinding>,
    type_indices: HashMap<Name, usize>,
    lifetime_indices: HashMap<String, usize>,
}

#[derive(Debug, Clone)]
enum GenericBinding {
    Type,
    Lifetime,
}

impl GenericBinderEnv {
    pub(super) fn for_impl(generics: &GenericParams) -> Option<Self> {
        if !generics.consts.is_empty() {
            return None;
        }
        Self::build(generics, false)
    }

    pub(super) fn for_trait(generics: &GenericParams) -> Option<Self> {
        if !generics.consts.is_empty() {
            return None;
        }
        Self::build(generics, true)
    }

    pub(super) fn empty() -> Self {
        Self {
            bindings: Vec::new(),
            type_indices: HashMap::new(),
            lifetime_indices: HashMap::new(),
        }
    }

    pub(super) fn variable_kinds(&self) -> VariableKinds<RgChalkInterner> {
        VariableKinds::from_iter(
            INTER,
            self.bindings.iter().map(|binding| match binding {
                GenericBinding::Type => VariableKind::Ty(TyVariableKind::General),
                GenericBinding::Lifetime => VariableKind::Lifetime,
            }),
        )
    }

    fn build(generics: &GenericParams, include_self: bool) -> Option<Self> {
        let mut env = Self::empty();
        if include_self {
            env.push_type(Name::new("Self"));
        }
        for lifetime in &generics.lifetimes {
            env.push_lifetime(lifetime.name.to_string());
        }
        for param in &generics.types {
            env.push_type(param.name.clone());
        }
        Some(env)
    }

    fn push_type(&mut self, name: Name) {
        let index = self.bindings.len();
        self.bindings.push(GenericBinding::Type);
        self.type_indices.insert(name, index);
    }

    fn push_lifetime(&mut self, name: String) {
        let index = self.bindings.len();
        self.bindings.push(GenericBinding::Lifetime);
        self.lifetime_indices.insert(name, index);
    }

    fn type_var(&self, name: &Name) -> Option<ChalkTy> {
        self.type_indices.get(name).map(|index| {
            BoundVar::new(DebruijnIndex::INNERMOST, *index).to_ty::<RgChalkInterner>(INTER)
        })
    }

    fn lifetime_var(&self, lifetime: &str) -> Option<chalk_ir::Lifetime<RgChalkInterner>> {
        self.lifetime_indices.get(lifetime).map(|index| {
            BoundVar::new(DebruijnIndex::INNERMOST, *index).to_lifetime::<RgChalkInterner>(INTER)
        })
    }
}

pub(super) struct ChalkLowerer<'lower, 'query, D, I> {
    item_paths: &'lower ItemPathQuery<'query, D, I>,
    associated_ty_by_trait_name: Option<&'lower HashMap<(TraitRef, Name), TypeAliasRef>>,
    context: TypePathContext,
    binders: &'lower GenericBinderEnv,
}

impl<'lower, 'query, D, I> ChalkLowerer<'lower, 'query, D, I>
where
    D: DefMapSource<Error = I::Error>,
    I: ItemStoreSource<'query>,
{
    pub(super) fn new(
        item_paths: &'lower ItemPathQuery<'query, D, I>,
        context: TypePathContext,
        binders: &'lower GenericBinderEnv,
    ) -> Self {
        Self {
            item_paths,
            associated_ty_by_trait_name: None,
            context,
            binders,
        }
    }

    pub(super) fn with_associated_tys(
        mut self,
        associated_ty_by_trait_name: &'lower HashMap<(TraitRef, Name), TypeAliasRef>,
    ) -> Self {
        self.associated_ty_by_trait_name = Some(associated_ty_by_trait_name);
        self
    }

    fn with_binders<'a>(&'a self, binders: &'a GenericBinderEnv) -> ChalkLowerer<'a, 'query, D, I> {
        ChalkLowerer {
            item_paths: self.item_paths,
            associated_ty_by_trait_name: self.associated_ty_by_trait_name,
            context: self.context,
            binders,
        }
    }

    pub(super) fn trait_datum(
        &self,
        trait_ref: TraitRef,
        generics: &GenericParams,
        super_traits: &[TypeBound],
        associated_ty_ids: Vec<AssocTypeId<RgChalkInterner>>,
    ) -> Option<TraitDatum<RgChalkInterner>> {
        let binders = GenericBinderEnv::for_trait(generics)?;
        let self_ty = BoundVar::new(DebruijnIndex::INNERMOST, 0).to_ty::<RgChalkInterner>(INTER);
        let lowerer = self.with_binders(&binders);
        let mut where_clauses = lowerer.type_param_bounds(generics, None)?;
        for super_trait in super_traits {
            where_clauses.push(lowerer.trait_bound_clause(&self_ty, super_trait, None)?);
        }
        where_clauses.extend(lowerer.where_predicates(&generics.where_predicates, None)?);

        Some(TraitDatum {
            id: chalk_trait_id(trait_ref),
            binders: chalk_ir::Binders::new(
                binders.variable_kinds(),
                TraitDatumBound { where_clauses },
            ),
            flags: TraitFlags {
                auto: false,
                marker: false,
                upstream: false,
                fundamental: false,
                non_enumerable: false,
                coinductive: false,
            },
            associated_ty_ids,
            well_known: None,
        })
    }

    pub(super) fn associated_ty_datum(
        &self,
        trait_ref: TraitRef,
        type_alias_ref: TypeAliasRef,
        type_alias_data: &TypeAliasData,
        trait_generics: &GenericParams,
    ) -> Option<AssociatedTyDatum<RgChalkInterner>> {
        let binders = GenericBinderEnv::for_trait(trait_generics)?;
        // V1 only lowers plain associated types like `type Item;`.
        // GAT binders and associated type bounds need another parameter/where-clause layer before
        // they can be represented without lying to Chalk.
        if type_alias_data.signature.generics().is_some()
            || !type_alias_data.signature.bounds().is_empty()
        {
            return None;
        }

        Some(AssociatedTyDatum {
            trait_id: chalk_trait_id(trait_ref),
            id: chalk_assoc_type_id(type_alias_ref),
            name: type_alias_data.name.to_string(),
            binders: chalk_ir::Binders::new(
                binders.variable_kinds(),
                AssociatedTyDatumBound {
                    bounds: Vec::new(),
                    where_clauses: Vec::new(),
                },
            ),
        })
    }

    pub(super) fn impl_datum(
        &self,
        impl_data: &rg_ir_model::hir::items::ImplData,
        associated_ty_value_ids: Vec<AssociatedTyValueId<RgChalkInterner>>,
    ) -> Option<ImplDatum<RgChalkInterner>> {
        let binders = GenericBinderEnv::for_impl(&impl_data.generics)?;
        let lowerer = self.with_binders(&binders);
        let self_ty = lowerer.impl_self_ty(impl_data, None)?;
        let trait_ref = impl_data.resolved_trait_ref.as_option().copied()?;
        let trait_args = lowerer.impl_trait_args(impl_data, None)?;
        let chalk_trait_ref = lowerer.chalk_trait_ref(trait_ref, self_ty, trait_args);
        let mut where_clauses = lowerer.type_param_bounds(&impl_data.generics, None)?;
        let predicates = lowerer.where_predicates(&impl_data.generics.where_predicates, None)?;
        where_clauses.extend(predicates);

        Some(ImplDatum {
            polarity: Polarity::Positive,
            binders: chalk_ir::Binders::new(
                binders.variable_kinds(),
                ImplDatumBound {
                    trait_ref: chalk_trait_ref,
                    where_clauses,
                },
            ),
            impl_type: ImplType::Local,
            associated_ty_value_ids,
        })
    }

    pub(super) fn associated_ty_value(
        &self,
        impl_ref: ImplRef,
        associated_ty_ref: TypeAliasRef,
        type_alias_data: &TypeAliasData,
        impl_data: &rg_ir_model::hir::items::ImplData,
    ) -> Option<AssociatedTyValue<RgChalkInterner>> {
        let binders = GenericBinderEnv::for_impl(&impl_data.generics)?;
        // Keep impl values aligned with the declaration support above: `type Item = T` is in,
        // `type Item<'a> = ...` and value-side bounds are left unsupported.
        if type_alias_data.signature.generics().is_some()
            || !type_alias_data.signature.bounds().is_empty()
        {
            return None;
        }

        let aliased_ty = type_alias_data.signature.aliased_ty()?;
        let lowerer = self.with_binders(&binders);
        let ty = lowerer.lower_type_ref(aliased_ty, None)?;

        Some(AssociatedTyValue {
            impl_id: chalk_impl_id(impl_ref),
            associated_ty_id: chalk_assoc_type_id(associated_ty_ref),
            value: chalk_ir::Binders::new(binders.variable_kinds(), AssociatedTyValueBound { ty }),
        })
    }

    pub(super) fn projection_alias(
        &self,
        assoc_type_ref: TypeAliasRef,
        goal: &TraitGoal,
        table: &InferenceTable,
    ) -> Option<ProjectionAliasLowering> {
        let variables = ProjectionVariableEnv::from_goal(goal, table);
        let self_ty = self.lower_infer_ty_with_projection_vars(&goal.self_ty, table, &variables)?;
        let mut substitution = Vec::with_capacity(1 + goal.args.len());
        substitution.push(GenericArgData::Ty(self_ty).intern(INTER));
        for arg in &goal.args {
            substitution
                .push(self.lower_infer_generic_arg_with_projection_vars(arg, table, &variables)?);
        }

        let alias = AliasTy::Projection(ProjectionTy {
            associated_ty_id: chalk_assoc_type_id(assoc_type_ref),
            substitution: Substitution::from_iter(INTER, substitution),
        });
        Some(ProjectionAliasLowering { alias, variables })
    }

    pub(super) fn candidate_where_goals(
        &self,
        impl_data: &rg_ir_model::hir::items::ImplData,
        subst: &InferenceTypeSubst,
        table: &InferenceTable,
    ) -> Option<Vec<ChalkGoal>> {
        let binders = GenericBinderEnv::for_impl(&impl_data.generics)?;
        let lowerer = self.with_binders(&binders);
        let mut clauses = lowerer.type_param_bounds(&impl_data.generics, Some((subst, table)))?;
        clauses.extend(
            lowerer.where_predicates(&impl_data.generics.where_predicates, Some((subst, table)))?,
        );

        clauses
            .into_iter()
            .map(|clause| {
                if !clause.binders.is_empty(INTER) {
                    return None;
                }
                let no_params: &[GenericArg<RgChalkInterner>] = &[];
                let where_clause = clause.substitute(INTER, no_params);
                if where_clause.has_free_vars(INTER) {
                    return None;
                }
                Some(DomainGoal::Holds(where_clause).cast(INTER))
            })
            .collect()
    }

    pub(super) fn type_param_bounds(
        &self,
        generics: &GenericParams,
        subst: Option<(&InferenceTypeSubst, &InferenceTable)>,
    ) -> Option<Vec<QuantifiedWhereClause<RgChalkInterner>>> {
        let mut clauses = Vec::new();
        for param in &generics.types {
            let subject = self.type_param_subject(&param.name, subst)?;
            for bound in &param.bounds {
                clauses.push(self.trait_bound_clause(&subject, bound, subst)?);
            }
        }
        Some(clauses)
    }

    fn where_predicates(
        &self,
        predicates: &[WherePredicate],
        subst: Option<(&InferenceTypeSubst, &InferenceTable)>,
    ) -> Option<Vec<QuantifiedWhereClause<RgChalkInterner>>> {
        let mut clauses = Vec::new();
        for predicate in predicates {
            match predicate {
                WherePredicate::Type { ty, bounds } => {
                    let subject = self.lower_type_ref(ty, subst)?;
                    for bound in bounds {
                        clauses.push(self.trait_bound_clause(&subject, bound, subst)?);
                    }
                }
                WherePredicate::Lifetime { .. } => {}
                WherePredicate::Unsupported(_) => return None,
            }
        }
        Some(clauses)
    }

    fn impl_self_ty(
        &self,
        impl_data: &rg_ir_model::hir::items::ImplData,
        subst: Option<(&InferenceTypeSubst, &InferenceTable)>,
    ) -> Option<ChalkTy> {
        if let Some(name) = impl_data.self_ty.type_param_name()
            && let Some(subject) = self.type_param_subject(&name, subst)
        {
            return Some(subject);
        }

        if let Some(type_def) = impl_data.resolved_self_ty.as_option().copied()
            && let TypeRef::Path(path) = &impl_data.self_ty
        {
            let args = self.generic_args_from_final_segment(path, subst)?;
            return Some(
                TyKind::Adt(AdtId(type_def), Substitution::from_iter(INTER, args)).intern(INTER),
            );
        }

        self.lower_type_ref(&impl_data.self_ty, subst)
    }

    fn impl_trait_args(
        &self,
        impl_data: &rg_ir_model::hir::items::ImplData,
        subst: Option<(&InferenceTypeSubst, &InferenceTable)>,
    ) -> Option<Vec<chalk_ir::GenericArg<RgChalkInterner>>> {
        let Some(TypeRef::Path(path)) = &impl_data.trait_ref else {
            return Some(Vec::new());
        };
        self.generic_args_from_final_segment(path, subst)
    }

    fn trait_bound_clause(
        &self,
        subject: &ChalkTy,
        bound: &TypeBound,
        subst: Option<(&InferenceTypeSubst, &InferenceTable)>,
    ) -> Option<QuantifiedWhereClause<RgChalkInterner>> {
        let TypeBound::Trait(TypeRef::Path(path)) = bound else {
            return None;
        };
        let path_resolution = self.resolve_trait_path(path)?;
        let args = self.generic_args_from_final_segment(path, subst)?;
        // `QuantifiedWhereClause` adds its own binder layer around the clause value. Even when it
        // binds zero new variables, references to the surrounding impl/trait parameters must move
        // one De Bruijn level out so Chalk does not treat them as parameters of this empty binder.
        let trait_ref = self
            .chalk_trait_ref(path_resolution, subject.clone(), args)
            .shifted_in(INTER);
        Some(chalk_ir::Binders::empty(
            INTER,
            WhereClause::Implemented(trait_ref),
        ))
    }

    fn chalk_trait_ref(
        &self,
        trait_ref: TraitRef,
        self_ty: ChalkTy,
        args: Vec<chalk_ir::GenericArg<RgChalkInterner>>,
    ) -> ChalkTraitRef<RgChalkInterner> {
        let mut substitution = Vec::with_capacity(1 + args.len());
        substitution.push(GenericArgData::Ty(self_ty).intern(INTER));
        substitution.extend(args);
        ChalkTraitRef {
            trait_id: chalk_trait_id(trait_ref),
            substitution: Substitution::from_iter(INTER, substitution),
        }
    }

    fn type_param_subject(
        &self,
        name: &Name,
        subst: Option<(&InferenceTypeSubst, &InferenceTable)>,
    ) -> Option<ChalkTy> {
        if let Some((subst, table)) = subst {
            let ty = subst.type_param(name.as_str())?;
            return self.lower_infer_ty(&ty, table);
        }
        self.binders.type_var(name)
    }

    fn lower_type_ref(
        &self,
        ty: &TypeRef,
        subst: Option<(&InferenceTypeSubst, &InferenceTable)>,
    ) -> Option<ChalkTy> {
        match ty {
            TypeRef::Unit => Some(TyKind::Tuple(0, Substitution::empty(INTER)).intern(INTER)),
            TypeRef::Never => Some(TyKind::Never.intern(INTER)),
            TypeRef::Infer | TypeRef::Unknown(_) => None,
            TypeRef::Path(path) => {
                let path_key = Path::from_type_path(path);
                if let Some(name) = path_key.single_name() {
                    if let Some((subst, table)) = subst
                        && let Some(ty) = subst.type_param(name)
                    {
                        return self.lower_infer_ty(&ty, table);
                    }
                    if let Some(ty) = self.binders.type_var(&Name::new(name)) {
                        return Some(ty);
                    }
                    if let Some(primitive) = PrimitiveTy::from_name(name) {
                        return self.primitive_ty(primitive);
                    }
                }

                let args = self.type_path_args(path, subst)?;
                match self
                    .item_paths
                    .resolve_type_path(self.context, &path_key)
                    .ok()?
                {
                    TypePathResolution::SelfType(type_def)
                    | TypePathResolution::TypeDef(type_def) => {
                        Some(TyKind::Adt(AdtId(type_def), args).intern(INTER))
                    }
                    TypePathResolution::TypeAlias(_)
                    | TypePathResolution::Trait(_)
                    | TypePathResolution::Unknown => None,
                }
            }
            TypeRef::Tuple(fields) => {
                let args = fields
                    .iter()
                    .map(|field| {
                        self.lower_type_ref(field, subst)
                            .map(|ty| GenericArgData::Ty(ty).intern(INTER))
                    })
                    .collect::<Option<Vec<_>>>()?;
                Some(TyKind::Tuple(args.len(), Substitution::from_iter(INTER, args)).intern(INTER))
            }
            TypeRef::Reference {
                lifetime,
                mutability,
                inner,
            } => {
                let lifetime = lifetime
                    .as_deref()
                    .and_then(|lifetime| self.binders.lifetime_var(lifetime))
                    .unwrap_or_else(|| LifetimeData::Erased.intern(INTER));
                let inner = self.lower_type_ref(inner, subst)?;
                Some(TyKind::Ref(self.chalk_mutability(*mutability), lifetime, inner).intern(INTER))
            }
            TypeRef::RawPointer { mutability, inner } => {
                let inner = self.lower_type_ref(inner, subst)?;
                Some(TyKind::Raw(self.chalk_mutability(*mutability), inner).intern(INTER))
            }
            TypeRef::Slice(inner) => {
                let inner = self.lower_type_ref(inner, subst)?;
                Some(TyKind::Slice(inner).intern(INTER))
            }
            TypeRef::Array { inner, len } => {
                let inner = self.lower_type_ref(inner, subst)?;
                let len = self.lower_array_len(len)?;
                Some(TyKind::Array(inner, len).intern(INTER))
            }
            TypeRef::FnPointer { .. } | TypeRef::ImplTrait(_) | TypeRef::DynTrait(_) => None,
            TypeRef::QualifiedAssociatedType {
                self_ty,
                trait_ty: Some(trait_ty),
                assoc_name,
            } => self.lower_qualified_associated_type(self_ty, trait_ty, assoc_name, subst),
            TypeRef::QualifiedAssociatedType { trait_ty: None, .. } => None,
        }
    }

    fn lower_qualified_associated_type(
        &self,
        self_ty: &TypeRef,
        trait_ty: &TypeRef,
        assoc_name: &Name,
        subst: Option<(&InferenceTypeSubst, &InferenceTable)>,
    ) -> Option<ChalkTy> {
        let self_ty = self.lower_type_ref(self_ty, subst)?;
        let TypeRef::Path(trait_path) = trait_ty else {
            return None;
        };
        let trait_ref = self.resolve_trait_path(trait_path)?;
        let associated_ty_ref = self
            .associated_ty_by_trait_name?
            .get(&(trait_ref, assoc_name.clone()))
            .copied()?;
        let mut args = Vec::with_capacity(1 + trait_path.segments.last()?.args.len());
        args.push(GenericArgData::Ty(self_ty).intern(INTER));
        args.extend(self.generic_args_from_final_segment(trait_path, subst)?);

        Some(
            TyKind::Alias(AliasTy::Projection(ProjectionTy {
                associated_ty_id: chalk_assoc_type_id(associated_ty_ref),
                substitution: Substitution::from_iter(INTER, args),
            }))
            .intern(INTER),
        )
    }

    fn lower_infer_ty(&self, ty: &RgTy, table: &InferenceTable) -> Option<ChalkTy> {
        self.lower_infer_ty_with_projection_vars(ty, table, &ProjectionVariableEnv::empty())
    }

    fn lower_infer_ty_with_projection_vars(
        &self,
        ty: &RgTy,
        table: &InferenceTable,
        projection_vars: &ProjectionVariableEnv,
    ) -> Option<ChalkTy> {
        let ty = table.canonicalize(ty);
        match ty {
            RgTy::Unit => Some(TyKind::Tuple(0, Substitution::empty(INTER)).intern(INTER)),
            RgTy::Never => Some(TyKind::Never.intern(INTER)),
            RgTy::Primitive(primitive) => self.primitive_ty(primitive),
            RgTy::Tuple(fields) => {
                let args = fields
                    .iter()
                    .map(|field| {
                        self.lower_infer_ty_with_projection_vars(field, table, projection_vars)
                            .map(|ty| GenericArgData::Ty(ty).intern(INTER))
                    })
                    .collect::<Option<Vec<_>>>()?;
                Some(TyKind::Tuple(args.len(), Substitution::from_iter(INTER, args)).intern(INTER))
            }
            RgTy::Slice(inner) => {
                let inner =
                    self.lower_infer_ty_with_projection_vars(&inner, table, projection_vars)?;
                Some(TyKind::Slice(inner).intern(INTER))
            }
            RgTy::Array { inner, len } => {
                let inner =
                    self.lower_infer_ty_with_projection_vars(&inner, table, projection_vars)?;
                let len = self.lower_array_len(&len)?;
                Some(TyKind::Array(inner, len).intern(INTER))
            }
            RgTy::Reference { mutability, inner } => {
                let inner =
                    self.lower_infer_ty_with_projection_vars(&inner, table, projection_vars)?;
                Some(
                    TyKind::Ref(
                        self.chalk_mutability(mutability),
                        LifetimeData::Erased.intern(INTER),
                        inner,
                    )
                    .intern(INTER),
                )
            }
            RgTy::Nominal(ty) | RgTy::SelfTy(ty) => {
                let args = ty
                    .args
                    .iter()
                    .map(|arg| {
                        self.lower_infer_generic_arg_with_projection_vars(
                            arg,
                            table,
                            projection_vars,
                        )
                    })
                    .collect::<Option<Vec<_>>>()?;
                Some(TyKind::Adt(AdtId(ty.def), Substitution::from_iter(INTER, args)).intern(INTER))
            }
            RgTy::Syntax(ty) => self.lower_type_ref(&ty, None),
            RgTy::InferVar {
                kind: InferVarKind::Type,
                id,
            } => projection_vars.chalk_ty_for_var(id),
            RgTy::InferVar { .. } | RgTy::Unknown | RgTy::Opaque { .. } | RgTy::Closure(_) => None,
            // Function items are real rust-glancer types, but lowering them to Chalk needs a real
            // `FnDef` signature. The current Chalk database only has placeholder fn-def callbacks,
            // so treating a function item as a solvable Chalk type would prove the wrong callable
            // shape. Body inference handles function-item callable evidence before this boundary.
            RgTy::FunctionItem(_) => None,
        }
    }

    fn lower_array_len(&self, len: &Option<String>) -> Option<chalk_ir::Const<RgChalkInterner>> {
        let len = len.as_ref()?;
        let ty = TyKind::Scalar(Scalar::Uint(UintTy::Usize)).intern(INTER);
        Some(
            ConstData {
                ty,
                value: ConstValue::Concrete(ConcreteConst {
                    interned: len.clone(),
                }),
            }
            .intern(INTER),
        )
    }

    fn lower_infer_generic_arg_with_projection_vars(
        &self,
        arg: &RgGenericArg,
        table: &InferenceTable,
        projection_vars: &ProjectionVariableEnv,
    ) -> Option<chalk_ir::GenericArg<RgChalkInterner>> {
        match arg {
            RgGenericArg::Type(ty) => self
                .lower_infer_ty_with_projection_vars(ty, table, projection_vars)
                .map(|ty| GenericArgData::Ty(ty).intern(INTER)),
            RgGenericArg::Lifetime(lifetime) => Some(
                self.binders
                    .lifetime_var(lifetime)
                    .unwrap_or_else(|| LifetimeData::Erased.intern(INTER)),
            )
            .map(|lifetime| GenericArgData::Lifetime(lifetime).intern(INTER)),
            RgGenericArg::Const(_)
            | RgGenericArg::FnTraitArgs { .. }
            | RgGenericArg::AssocType { .. }
            | RgGenericArg::Unsupported(_) => None,
        }
    }

    fn type_path_args(
        &self,
        path: &rg_ir_model::items::TypePath,
        subst: Option<(&InferenceTypeSubst, &InferenceTable)>,
    ) -> Option<Substitution<RgChalkInterner>> {
        Some(Substitution::from_iter(
            INTER,
            self.generic_args_from_final_segment(path, subst)?,
        ))
    }

    fn generic_args_from_final_segment(
        &self,
        path: &rg_ir_model::items::TypePath,
        subst: Option<(&InferenceTypeSubst, &InferenceTable)>,
    ) -> Option<Vec<chalk_ir::GenericArg<RgChalkInterner>>> {
        let args = path
            .segments
            .last()
            .map(|segment| segment.args.as_slice())
            .unwrap_or(&[]);
        args.iter()
            .map(|arg| self.generic_arg(arg, subst))
            .collect::<Option<Vec<_>>>()
    }

    fn generic_arg(
        &self,
        arg: &ItemGenericArg,
        subst: Option<(&InferenceTypeSubst, &InferenceTable)>,
    ) -> Option<chalk_ir::GenericArg<RgChalkInterner>> {
        match arg {
            ItemGenericArg::Type(ty) => self
                .lower_type_ref(ty, subst)
                .map(|ty| GenericArgData::Ty(ty).intern(INTER)),
            ItemGenericArg::Lifetime(lifetime) => Some(
                self.binders
                    .lifetime_var(lifetime)
                    .unwrap_or_else(|| LifetimeData::Erased.intern(INTER)),
            )
            .map(|lifetime| GenericArgData::Lifetime(lifetime).intern(INTER)),
            ItemGenericArg::Const(_)
            | ItemGenericArg::FnTraitArgs { .. }
            | ItemGenericArg::AssocType { .. }
            | ItemGenericArg::Unsupported(_) => None,
        }
    }

    fn resolve_trait_path(&self, path: &rg_ir_model::items::TypePath) -> Option<TraitRef> {
        let path_key = Path::from_type_path(path);
        if let TypePathResolution::Trait(trait_ref) = self
            .item_paths
            .resolve_type_path(self.context, &path_key)
            .ok()?
        {
            return Some(trait_ref);
        }
        None
    }

    fn primitive_ty(&self, primitive: PrimitiveTy) -> Option<ChalkTy> {
        let scalar = match primitive {
            PrimitiveTy::Bool => Scalar::Bool,
            PrimitiveTy::Char => Scalar::Char,
            PrimitiveTy::Str => return Some(TyKind::Str.intern(INTER)),
            PrimitiveTy::SignedInt(kind) => Scalar::Int(match kind {
                SignedIntTy::I8 => chalk_ir::IntTy::I8,
                SignedIntTy::I16 => chalk_ir::IntTy::I16,
                SignedIntTy::I32 => chalk_ir::IntTy::I32,
                SignedIntTy::I64 => chalk_ir::IntTy::I64,
                SignedIntTy::I128 => chalk_ir::IntTy::I128,
                SignedIntTy::Isize => chalk_ir::IntTy::Isize,
            }),
            PrimitiveTy::UnsignedInt(kind) => Scalar::Uint(match kind {
                UnsignedIntTy::U8 => chalk_ir::UintTy::U8,
                UnsignedIntTy::U16 => chalk_ir::UintTy::U16,
                UnsignedIntTy::U32 => chalk_ir::UintTy::U32,
                UnsignedIntTy::U64 => chalk_ir::UintTy::U64,
                UnsignedIntTy::U128 => chalk_ir::UintTy::U128,
                UnsignedIntTy::Usize => UintTy::Usize,
            }),
            PrimitiveTy::Float(kind) => Scalar::Float(match kind {
                FloatTy::F32 => chalk_ir::FloatTy::F32,
                FloatTy::F64 => chalk_ir::FloatTy::F64,
            }),
        };
        Some(TyKind::Scalar(scalar).intern(INTER))
    }

    fn chalk_mutability(&self, mutability: Mutability) -> ChalkMutability {
        match mutability {
            Mutability::Shared => ChalkMutability::Not,
            Mutability::Mutable => ChalkMutability::Mut,
        }
    }
}

pub(super) fn chalk_trait_id(trait_ref: TraitRef) -> TraitId<RgChalkInterner> {
    TraitId(ChalkDefId::Trait(trait_ref))
}

pub(super) fn chalk_impl_id(impl_ref: rg_ir_model::ImplRef) -> chalk_ir::ImplId<RgChalkInterner> {
    chalk_ir::ImplId(ChalkDefId::Impl(impl_ref))
}

pub(super) fn chalk_assoc_type_id(type_alias_ref: TypeAliasRef) -> AssocTypeId<RgChalkInterner> {
    AssocTypeId(ChalkDefId::AssocType(type_alias_ref))
}

pub(super) fn chalk_assoc_type_value_id(
    type_alias_ref: TypeAliasRef,
) -> AssociatedTyValueId<RgChalkInterner> {
    AssociatedTyValueId(ChalkDefId::AssocTypeValue(type_alias_ref))
}

pub(super) fn stub_trait_datum(
    trait_ref: TraitRef,
    parameter_count: usize,
) -> TraitDatum<RgChalkInterner> {
    let binders = VariableKinds::from_iter(
        INTER,
        (0..parameter_count).map(|_| VariableKind::Ty(TyVariableKind::General)),
    );
    TraitDatum {
        id: chalk_trait_id(trait_ref),
        binders: chalk_ir::Binders::new(
            binders,
            TraitDatumBound {
                where_clauses: Vec::new(),
            },
        ),
        flags: TraitFlags {
            auto: false,
            marker: false,
            upstream: false,
            fundamental: false,
            non_enumerable: false,
            coinductive: false,
        },
        associated_ty_ids: Vec::new(),
        well_known: None,
    }
}

pub(super) fn adt_datum(
    type_def: TypeDefRef,
    generics: Option<&GenericParams>,
) -> Option<AdtDatum<RgChalkInterner>> {
    let binders = match generics {
        Some(generics) => GenericBinderEnv::for_impl(generics)?,
        None => GenericBinderEnv::empty(),
    };
    let kind = match type_def.id {
        TypeDefId::Struct(_) => AdtKind::Struct,
        TypeDefId::Enum(_) => AdtKind::Enum,
        TypeDefId::Union(_) => AdtKind::Union,
    };
    Some(AdtDatum {
        binders: chalk_ir::Binders::new(
            binders.variable_kinds(),
            AdtDatumBound {
                variants: match kind {
                    AdtKind::Struct | AdtKind::Union => {
                        vec![AdtVariantDatum { fields: Vec::new() }]
                    }
                    AdtKind::Enum => Vec::new(),
                },
                where_clauses: Vec::new(),
            },
        ),
        id: AdtId(type_def),
        flags: AdtFlags {
            upstream: false,
            fundamental: false,
            phantom_data: false,
        },
        kind,
    })
}

pub(super) fn unit_ty() -> ChalkTy {
    TyKind::Tuple(0, Substitution::empty(INTER)).intern(INTER)
}
