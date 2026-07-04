//! Projection-specific state shared by Chalk lowering, solving, and answer decoding.
//!
//! Lowering owns ordinary Rust syntax-to-Chalk conversion, and raising owns Chalk-to-project type
//! conversion. Projection normalization needs a small bit of state that belongs to neither side:
//! which rust-glancer inference variables were exposed as existential Chalk variables, and how a
//! Chalk answer maps those variables back.

use std::collections::HashMap;

use chalk_ir::{
    AliasTy, BoundVar, DebruijnIndex, GenericArg as ChalkGenericArg, GenericArgData, Ty as ChalkTy,
    TyKind, TyVariableKind, VariableKind, VariableKinds,
};

use super::interner::RgChalkInterner;
use crate::inference::{InferVarId, InferVarKind, InferenceTable};
use crate::trait_selection::TraitGoal;
use crate::{GenericArg as RgGenericArg, Ty as RgTy};

const INTER: RgChalkInterner = RgChalkInterner;

/// Inference variables that a projection goal lets Chalk mention in its answer.
///
/// This is deliberately much smaller than a general Chalk-to-inference bridge. Projection
/// normalization needs one specific thing first: when the caller asks for
/// `<Iter<?T> as Iterator>::Item`, Chalk should be able to answer `?T` instead of forcing the
/// project-side alias fallback to preserve that relationship by hand.
#[derive(Debug, Clone)]
pub(super) struct ProjectionVariableEnv {
    vars: Vec<InferVarId>,
    indices: HashMap<InferVarId, usize>,
}

impl ProjectionVariableEnv {
    pub(super) fn empty() -> Self {
        Self {
            vars: Vec::new(),
            indices: HashMap::new(),
        }
    }

    pub(super) fn from_goal(goal: &TraitGoal, table: &InferenceTable) -> Self {
        let mut env = Self::empty();
        env.collect_ty(&goal.self_ty, table);
        for arg in &goal.args {
            env.collect_generic_arg(arg, table);
        }
        env
    }

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

    pub(super) fn project_var_ty(&self, index: usize) -> Option<RgTy> {
        self.vars
            .get(index)
            .copied()
            .map(|id| RgTy::var_for_kind(InferVarKind::Type, id))
    }

    pub(super) fn iter_project_vars(&self) -> impl Iterator<Item = (usize, InferVarId)> + '_ {
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
            RgTy::Slice(inner) | RgTy::Reference { inner, .. } => {
                self.collect_ty(&inner, table);
            }
            RgTy::Array { inner, .. } => {
                self.collect_ty(&inner, table);
            }
            RgTy::Nominal(ty) | RgTy::SelfTy(ty) => {
                for arg in &ty.args {
                    self.collect_generic_arg(arg, table);
                }
            }
            RgTy::Unit
            | RgTy::Never
            | RgTy::Primitive(_)
            | RgTy::Opaque { .. }
            | RgTy::Closure(_)
            | RgTy::FunctionItem(_)
            | RgTy::Syntax(_)
            | RgTy::Unknown
            | RgTy::InferVar { .. } => {}
        }
    }

    fn collect_generic_arg(&mut self, arg: &RgGenericArg, table: &InferenceTable) {
        match arg {
            RgGenericArg::Type(ty) => self.collect_ty(ty, table),
            RgGenericArg::Lifetime(_)
            | RgGenericArg::Const(_)
            | RgGenericArg::FnTraitArgs { .. }
            | RgGenericArg::AssocType { .. }
            | RgGenericArg::Unsupported(_) => {}
        }
    }
}

pub(super) struct ProjectionAliasLowering {
    pub(super) alias: AliasTy<RgChalkInterner>,
    pub(super) variables: ProjectionVariableEnv,
}

/// Bound variables from a Chalk projection answer that correspond to project inference variables.
#[derive(Debug, Clone)]
pub(super) struct ProjectionAnswerVars {
    vars: Vec<(BoundVar, RgTy)>,
}

impl ProjectionAnswerVars {
    pub(super) fn from_subst_args(
        variables: &ProjectionVariableEnv,
        subst_args: &[ChalkGenericArg<RgChalkInterner>],
    ) -> Option<Self> {
        // Chalk may answer an unconstrained projection in canonical form:
        //
        // `for<?U0> { [?Receiver := ?U0, ?Result := ?U0] }`
        //
        // That is useful only because `?Receiver` corresponds to a real rust-glancer inference
        // slot. Build that tiny map first, then decode result types through it.
        let mut vars = Vec::new();
        for (index, var) in variables.iter_project_vars() {
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
