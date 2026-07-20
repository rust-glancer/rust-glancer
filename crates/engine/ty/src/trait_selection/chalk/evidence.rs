//! Inference evidence shared by Chalk lowering, solving, and answer decoding.
//!
//! Lowering owns ordinary Rust syntax-to-Chalk conversion, and raising owns Chalk-to-project type
//! conversion. This module records which rust-glancer inference variables were exposed as
//! existential Chalk variables and how an answer maps those variables back. Predicate proof and
//! projection use the same mapping so callable bounds can return evidence to body inference.

use std::collections::HashMap;

use chalk_ir::{
    AliasTy, BoundVar, DebruijnIndex, GenericArg as ChalkGenericArg, GenericArgData, Ty as ChalkTy,
    TyKind, TyVariableKind, VariableKind, VariableKinds,
};

use super::interner::RgChalkInterner;
use crate::inference::{InferVarId, InferVarKind, InferenceTable};
use crate::trait_selection::TraitGoal;
use crate::{Clause, GenericArg as RgGenericArg, Ty as RgTy};

const INTER: RgChalkInterner = RgChalkInterner;

/// Project inference variables made existential inside one Chalk query.
#[derive(Debug, Clone)]
pub(super) struct SolverVariableEnv {
    vars: Vec<InferVarId>,
    indices: HashMap<InferVarId, usize>,
}

impl SolverVariableEnv {
    pub(super) fn empty() -> Self {
        Self {
            vars: Vec::new(),
            indices: HashMap::new(),
        }
    }

    pub(super) fn from_goal(goal: &TraitGoal, table: &InferenceTable) -> Self {
        let mut env = Self::empty();
        for arg in &goal.application.args {
            env.collect_generic_arg(arg, table);
        }
        for binding in &goal.associated_types {
            env.collect_ty(&binding.ty, table);
        }
        env
    }

    pub(super) fn from_clauses(clauses: &[Clause], table: &InferenceTable) -> Self {
        let mut env = Self::empty();
        for clause in clauses {
            match clause {
                Clause::Implemented(application) => {
                    for arg in &application.args {
                        env.collect_generic_arg(arg, table);
                    }
                }
                Clause::AliasEq { alias, ty } => {
                    for arg in &alias.args {
                        env.collect_generic_arg(arg, table);
                    }
                    env.collect_ty(ty, table);
                }
            }
        }
        env
    }

    pub(super) fn variable_kinds(&self) -> VariableKinds<RgChalkInterner> {
        VariableKinds::from_iter(
            INTER,
            self.vars
                .iter()
                .map(|_| VariableKind::Ty(TyVariableKind::General)),
        )
    }

    /// Describe project variables first, followed by one extra slot for a projection result.
    pub(super) fn variable_kinds_with_result(&self) -> VariableKinds<RgChalkInterner> {
        VariableKinds::from_iter(
            INTER,
            (0..=self.vars.len()).map(|_| VariableKind::Ty(TyVariableKind::General)),
        )
    }

    pub(super) fn result_index(&self) -> usize {
        self.vars.len()
    }

    pub(super) fn result_ty(&self) -> ChalkTy<RgChalkInterner> {
        BoundVar::new(DebruijnIndex::INNERMOST, self.result_index()).to_ty::<RgChalkInterner>(INTER)
    }

    pub(super) fn project_ty_for_index(&self, index: usize) -> Option<RgTy> {
        self.vars
            .get(index)
            .copied()
            .map(|id| RgTy::var_for_kind(InferVarKind::Type, id))
    }

    pub(super) fn iter_vars(&self) -> impl Iterator<Item = (usize, InferVarId)> + '_ {
        self.vars.iter().copied().enumerate()
    }

    pub(super) fn chalk_ty_for_var(&self, id: InferVarId) -> Option<ChalkTy<RgChalkInterner>> {
        let index = *self.indices.get(&id)?;
        Some(BoundVar::new(DebruijnIndex::INNERMOST, index).to_ty::<RgChalkInterner>(INTER))
    }

    fn push_var(&mut self, id: InferVarId) {
        if self.indices.contains_key(&id) {
            return;
        }
        let index = self.vars.len();
        self.vars.push(id);
        self.indices.insert(id, index);
    }

    fn collect_ty(&mut self, ty: &RgTy, table: &InferenceTable) {
        match table.canonicalize(ty) {
            RgTy::InferVar {
                kind: InferVarKind::Type,
                id,
            } => self.push_var(id),
            RgTy::Tuple(fields) => {
                for field in fields {
                    self.collect_ty(&field, table);
                }
            }
            RgTy::Slice(inner) | RgTy::Reference { inner, .. } | RgTy::RawPointer { inner, .. } => {
                self.collect_ty(&inner, table);
            }
            RgTy::Array { inner, .. } => {
                self.collect_ty(&inner, table);
            }
            RgTy::FnPointer { params, ret } => {
                for param in params {
                    self.collect_ty(&param, table);
                }
                self.collect_ty(&ret, table);
            }
            RgTy::Adt(ty) => {
                for arg in &ty.args {
                    self.collect_generic_arg(arg, table);
                }
            }
            RgTy::Alias(alias) => {
                for arg in alias.args() {
                    self.collect_generic_arg(arg, table);
                }
            }
            RgTy::FnDef(function) => {
                for arg in &function.args {
                    self.collect_generic_arg(arg, table);
                }
            }
            RgTy::Closure(closure) => {
                for param in &closure.params {
                    self.collect_ty(param, table);
                }
                self.collect_ty(&closure.ret, table);
            }
            RgTy::Unit
            | RgTy::Never
            | RgTy::Primitive(_)
            | RgTy::Param(_)
            | RgTy::Unknown
            | RgTy::InferVar { .. } => {}
        }
    }

    fn collect_generic_arg(&mut self, arg: &RgGenericArg, table: &InferenceTable) {
        match arg {
            RgGenericArg::Type(ty) => self.collect_ty(ty, table),
            RgGenericArg::Lifetime(_) | RgGenericArg::Const(_) => {}
        }
    }
}

/// One associated-type alias lowered with the variables needed to decode its answer.
///
/// For `<Iter<?T> as Iterator>::Item`, `alias` refers to the Chalk projection containing the
/// existential for `?T`. Projection solving adds one more existential for the resulting `Item`;
/// `variables` lets answer decoding map both pieces back into the caller's inference table.
pub(super) struct ProjectionAliasLowering {
    pub(super) alias: AliasTy<RgChalkInterner>,
    pub(super) variables: SolverVariableEnv,
}

/// Bound variables from a Chalk answer that correspond to project inference variables.
#[derive(Debug, Clone)]
pub(super) struct SolverAnswerVars {
    vars: Vec<(BoundVar, RgTy)>,
}

impl SolverAnswerVars {
    pub(super) fn empty() -> Self {
        Self { vars: Vec::new() }
    }

    pub(super) fn from_subst_args(
        variables: &SolverVariableEnv,
        subst_args: &[ChalkGenericArg<RgChalkInterner>],
    ) -> Option<Self> {
        // Chalk may answer an unconstrained projection in canonical form:
        //
        // `for<?U0> { [?Receiver := ?U0, ?Result := ?U0] }`
        //
        // That is useful only because `?Receiver` corresponds to a real rust-glancer inference
        // slot. Build that tiny map first, then decode result types through it.
        let mut vars = Vec::new();
        for (index, var) in variables.iter_vars() {
            let project_arg = subst_args.get(index)?;
            let GenericArgData::Ty(project_ty) = project_arg.data(INTER) else {
                return None;
            };
            if let TyKind::BoundVar(bound_var) = project_ty.kind(INTER) {
                vars.push((*bound_var, RgTy::var_for_kind(InferVarKind::Type, var)));
            }
        }
        Some(Self { vars })
    }

    pub(super) fn as_slice(&self) -> &[(BoundVar, RgTy)] {
        &self.vars
    }
}
