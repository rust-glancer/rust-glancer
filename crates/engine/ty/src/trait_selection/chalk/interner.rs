use std::fmt;
use std::hash::Hash;
use std::sync::Arc;

use chalk_ir::interner::Interner;
use chalk_ir::{
    AdtId, AssocTypeId, CanonicalVarKind, ConstData, Constraint, Constraints, CoroutineId, FnDefId,
    ForeignDefId, GenericArg, GenericArgData, Goal, GoalData, Goals, InEnvironment, LifetimeData,
    OpaqueTyId, ProgramClause, ProgramClauseData, ProgramClauseImplication, ProgramClauses,
    ProjectionTy, QuantifiedWhereClause, QuantifiedWhereClauses, SeparatorTraitRef, Substitution,
    TraitId, TyData, TyKind, VariableKind, Variance, Variances,
};
use rg_ir_model::{FunctionRef, ImplRef, TraitRef, TypeAliasRef, TypeDefRef};

use crate::ClosureTyId;

// Chalk's `DefId` family covers more Rust item kinds than this MVP lowers today. Keeping the
// variants here makes unsupported callbacks explicit instead of smuggling every id through `u32`.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum ChalkDefId {
    Trait(TraitRef),
    Impl(ImplRef),
    AssocType(TypeAliasRef),
    Opaque(u32),
    Function(FunctionRef),
    Closure(ClosureTyId),
    Coroutine(u32),
    Foreign(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct RgChalkInterner;

impl RgChalkInterner {
    fn collect_arc<T, E>(data: impl IntoIterator<Item = Result<T, E>>) -> Result<Arc<[T]>, E> {
        data.into_iter()
            .collect::<Result<Vec<_>, _>>()
            .map(Arc::from)
    }
}

impl Interner for RgChalkInterner {
    type InternedType = Arc<TyData<Self>>;
    type InternedLifetime = LifetimeData<Self>;
    type InternedConst = Arc<ConstData<Self>>;
    type InternedConcreteConst = String;
    type InternedGenericArg = Arc<GenericArgData<Self>>;
    type InternedGoal = Arc<GoalData<Self>>;
    type InternedGoals = Arc<[Goal<Self>]>;
    type InternedSubstitution = Arc<[GenericArg<Self>]>;
    type InternedProgramClauses = Arc<[ProgramClause<Self>]>;
    type InternedProgramClause = Arc<ProgramClauseData<Self>>;
    type InternedQuantifiedWhereClauses = Arc<[QuantifiedWhereClause<Self>]>;
    type InternedVariableKinds = Arc<[VariableKind<Self>]>;
    type InternedCanonicalVarKinds = Arc<[CanonicalVarKind<Self>]>;
    type InternedConstraints = Arc<[InEnvironment<Constraint<Self>>]>;
    type InternedVariances = Arc<[Variance]>;

    type DefId = ChalkDefId;
    type InternedAdtId = TypeDefRef;
    type Identifier = String;
    type FnAbi = ();

    fn debug_adt_id(adt_id: AdtId<Self>, fmt: &mut fmt::Formatter<'_>) -> Option<fmt::Result> {
        Some(write!(fmt, "{:?}", adt_id.0))
    }

    fn debug_trait_id(
        trait_id: TraitId<Self>,
        fmt: &mut fmt::Formatter<'_>,
    ) -> Option<fmt::Result> {
        Some(write!(fmt, "{:?}", trait_id.0))
    }

    fn debug_assoc_type_id(
        type_id: AssocTypeId<Self>,
        fmt: &mut fmt::Formatter<'_>,
    ) -> Option<fmt::Result> {
        Some(write!(fmt, "{:?}", type_id.0))
    }

    fn debug_opaque_ty_id(
        opaque_ty_id: OpaqueTyId<Self>,
        fmt: &mut fmt::Formatter<'_>,
    ) -> Option<fmt::Result> {
        Some(write!(fmt, "{:?}", opaque_ty_id.0))
    }

    fn debug_fn_def_id(
        fn_def_id: FnDefId<Self>,
        fmt: &mut fmt::Formatter<'_>,
    ) -> Option<fmt::Result> {
        Some(write!(fmt, "{:?}", fn_def_id.0))
    }

    fn debug_closure_id(
        closure_id: chalk_ir::ClosureId<Self>,
        fmt: &mut fmt::Formatter<'_>,
    ) -> Option<fmt::Result> {
        Some(write!(fmt, "{:?}", closure_id.0))
    }

    fn debug_foreign_def_id(
        foreign_def_id: ForeignDefId<Self>,
        fmt: &mut fmt::Formatter<'_>,
    ) -> Option<fmt::Result> {
        Some(write!(fmt, "{:?}", foreign_def_id.0))
    }

    fn debug_coroutine_id(
        coroutine_id: CoroutineId<Self>,
        fmt: &mut fmt::Formatter<'_>,
    ) -> Option<fmt::Result> {
        Some(write!(fmt, "{:?}", coroutine_id.0))
    }

    fn debug_projection_ty(
        projection_ty: &ProjectionTy<Self>,
        fmt: &mut fmt::Formatter<'_>,
    ) -> Option<fmt::Result> {
        Some(write!(fmt, "{:?}", projection_ty.associated_ty_id.0))
    }

    fn debug_ty(ty: &chalk_ir::Ty<Self>, fmt: &mut fmt::Formatter<'_>) -> Option<fmt::Result> {
        Some(write!(fmt, "{:?}", ty.kind(Self).debug(Self)))
    }

    fn debug_lifetime(
        lifetime: &chalk_ir::Lifetime<Self>,
        fmt: &mut fmt::Formatter<'_>,
    ) -> Option<fmt::Result> {
        Some(write!(fmt, "{:?}", lifetime.data(Self)))
    }

    fn debug_const(
        constant: &chalk_ir::Const<Self>,
        fmt: &mut fmt::Formatter<'_>,
    ) -> Option<fmt::Result> {
        Some(write!(fmt, "{:?}", constant.data(Self)))
    }

    fn debug_generic_arg(
        generic_arg: &GenericArg<Self>,
        fmt: &mut fmt::Formatter<'_>,
    ) -> Option<fmt::Result> {
        Some(write!(fmt, "{:?}", generic_arg.data(Self).inner_debug()))
    }

    fn debug_goal(goal: &Goal<Self>, fmt: &mut fmt::Formatter<'_>) -> Option<fmt::Result> {
        Some(write!(fmt, "{:?}", goal.data(Self)))
    }

    fn debug_goals(goals: &Goals<Self>, fmt: &mut fmt::Formatter<'_>) -> Option<fmt::Result> {
        Some(write!(fmt, "{:?}", goals.debug(Self)))
    }

    fn debug_program_clause_implication(
        pci: &ProgramClauseImplication<Self>,
        fmt: &mut fmt::Formatter<'_>,
    ) -> Option<fmt::Result> {
        Some(write!(fmt, "{:?}", pci.debug(Self)))
    }

    fn debug_program_clause(
        clause: &ProgramClause<Self>,
        fmt: &mut fmt::Formatter<'_>,
    ) -> Option<fmt::Result> {
        Some(write!(fmt, "{:?}", clause.data(Self)))
    }

    fn debug_program_clauses(
        clauses: &ProgramClauses<Self>,
        fmt: &mut fmt::Formatter<'_>,
    ) -> Option<fmt::Result> {
        Some(write!(fmt, "{:?}", clauses.as_slice(Self)))
    }

    fn debug_substitution(
        substitution: &Substitution<Self>,
        fmt: &mut fmt::Formatter<'_>,
    ) -> Option<fmt::Result> {
        Some(write!(fmt, "{:?}", substitution.debug(Self)))
    }

    fn debug_separator_trait_ref(
        separator_trait_ref: &SeparatorTraitRef<'_, Self>,
        fmt: &mut fmt::Formatter<'_>,
    ) -> Option<fmt::Result> {
        Some(write!(fmt, "{:?}", separator_trait_ref.debug(Self)))
    }

    fn debug_quantified_where_clauses(
        clauses: &QuantifiedWhereClauses<Self>,
        fmt: &mut fmt::Formatter<'_>,
    ) -> Option<fmt::Result> {
        Some(write!(fmt, "{:?}", clauses.as_slice(Self)))
    }

    fn debug_constraints(
        clauses: &Constraints<Self>,
        fmt: &mut fmt::Formatter<'_>,
    ) -> Option<fmt::Result> {
        Some(write!(fmt, "{:?}", clauses.as_slice(Self)))
    }

    fn debug_variances(
        variances: &Variances<Self>,
        fmt: &mut fmt::Formatter<'_>,
    ) -> Option<fmt::Result> {
        Some(write!(fmt, "{:?}", variances.as_slice(Self)))
    }

    fn intern_ty(self, kind: TyKind<Self>) -> Self::InternedType {
        Arc::new(TyData {
            flags: kind.compute_flags(self),
            kind,
        })
    }

    fn ty_data(self, ty: &Self::InternedType) -> &TyData<Self> {
        ty.as_ref()
    }

    fn intern_lifetime(self, lifetime: LifetimeData<Self>) -> Self::InternedLifetime {
        lifetime
    }

    fn lifetime_data(self, lifetime: &Self::InternedLifetime) -> &LifetimeData<Self> {
        lifetime
    }

    fn intern_const(self, constant: ConstData<Self>) -> Self::InternedConst {
        Arc::new(constant)
    }

    fn const_data(self, constant: &Self::InternedConst) -> &ConstData<Self> {
        constant.as_ref()
    }

    fn const_eq(
        self,
        _ty: &Self::InternedType,
        c1: &Self::InternedConcreteConst,
        c2: &Self::InternedConcreteConst,
    ) -> bool {
        c1 == c2
    }

    fn intern_generic_arg(self, data: GenericArgData<Self>) -> Self::InternedGenericArg {
        Arc::new(data)
    }

    fn generic_arg_data(self, arg: &Self::InternedGenericArg) -> &GenericArgData<Self> {
        arg.as_ref()
    }

    fn intern_goal(self, data: GoalData<Self>) -> Self::InternedGoal {
        Arc::new(data)
    }

    fn goal_data(self, goal: &Self::InternedGoal) -> &GoalData<Self> {
        goal.as_ref()
    }

    fn intern_goals<E>(
        self,
        data: impl IntoIterator<Item = Result<Goal<Self>, E>>,
    ) -> Result<Self::InternedGoals, E> {
        Self::collect_arc(data)
    }

    fn goals_data(self, goals: &Self::InternedGoals) -> &[Goal<Self>] {
        goals
    }

    fn intern_substitution<E>(
        self,
        data: impl IntoIterator<Item = Result<GenericArg<Self>, E>>,
    ) -> Result<Self::InternedSubstitution, E> {
        Self::collect_arc(data)
    }

    fn substitution_data(self, substitution: &Self::InternedSubstitution) -> &[GenericArg<Self>] {
        substitution
    }

    fn intern_program_clause(self, data: ProgramClauseData<Self>) -> Self::InternedProgramClause {
        Arc::new(data)
    }

    fn program_clause_data(self, clause: &Self::InternedProgramClause) -> &ProgramClauseData<Self> {
        clause.as_ref()
    }

    fn intern_program_clauses<E>(
        self,
        data: impl IntoIterator<Item = Result<ProgramClause<Self>, E>>,
    ) -> Result<Self::InternedProgramClauses, E> {
        Self::collect_arc(data)
    }

    fn program_clauses_data(
        self,
        clauses: &Self::InternedProgramClauses,
    ) -> &[ProgramClause<Self>] {
        clauses
    }

    fn intern_quantified_where_clauses<E>(
        self,
        data: impl IntoIterator<Item = Result<QuantifiedWhereClause<Self>, E>>,
    ) -> Result<Self::InternedQuantifiedWhereClauses, E> {
        Self::collect_arc(data)
    }

    fn quantified_where_clauses_data(
        self,
        clauses: &Self::InternedQuantifiedWhereClauses,
    ) -> &[QuantifiedWhereClause<Self>] {
        clauses
    }

    fn intern_generic_arg_kinds<E>(
        self,
        data: impl IntoIterator<Item = Result<VariableKind<Self>, E>>,
    ) -> Result<Self::InternedVariableKinds, E> {
        Self::collect_arc(data)
    }

    fn variable_kinds_data(
        self,
        variable_kinds: &Self::InternedVariableKinds,
    ) -> &[VariableKind<Self>] {
        variable_kinds
    }

    fn intern_canonical_var_kinds<E>(
        self,
        data: impl IntoIterator<Item = Result<CanonicalVarKind<Self>, E>>,
    ) -> Result<Self::InternedCanonicalVarKinds, E> {
        Self::collect_arc(data)
    }

    fn canonical_var_kinds_data(
        self,
        canonical_var_kinds: &Self::InternedCanonicalVarKinds,
    ) -> &[CanonicalVarKind<Self>] {
        canonical_var_kinds
    }

    fn intern_constraints<E>(
        self,
        data: impl IntoIterator<Item = Result<InEnvironment<Constraint<Self>>, E>>,
    ) -> Result<Self::InternedConstraints, E> {
        Self::collect_arc(data)
    }

    fn constraints_data(
        self,
        constraints: &Self::InternedConstraints,
    ) -> &[InEnvironment<Constraint<Self>>] {
        constraints
    }

    fn intern_variances<E>(
        self,
        data: impl IntoIterator<Item = Result<Variance, E>>,
    ) -> Result<Self::InternedVariances, E> {
        Self::collect_arc(data)
    }

    fn variances_data(self, variances: &Self::InternedVariances) -> &[Variance] {
        variances
    }
}
