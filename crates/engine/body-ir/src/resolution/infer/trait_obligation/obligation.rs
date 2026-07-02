//! Small body-side vocabulary for solver-shaped work.
//!
//! `rg_ty::TraitGoal` is the pure question that can eventually go to a real solver. The body
//! layer wraps it in an obligation when the question came from a concrete local operation, such
//! as a selected call bound. That wrapper stays intentionally small: it lets selected-call code
//! produce obligations before the evaluation code decides how to use today's shallow solver hooks.

use rg_ty::{TraitGoal, inference::InferTy};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BodyObligation {
    goal: BodyObligationGoal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum BodyObligationGoal {
    Trait(TraitGoal),
    Callable(BodyCallableObligation),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BodyCallableObligation {
    self_ty: InferTy,
    params: Vec<InferTy>,
    ret: InferTy,
}

impl BodyObligation {
    pub(super) fn trait_goal(goal: TraitGoal) -> Self {
        Self {
            goal: BodyObligationGoal::Trait(goal),
        }
    }

    pub(super) fn callable(goal: BodyCallableObligation) -> Self {
        Self {
            goal: BodyObligationGoal::Callable(goal),
        }
    }

    pub(super) fn into_goal(self) -> BodyObligationGoal {
        self.goal
    }
}

impl BodyCallableObligation {
    pub(super) fn new(self_ty: InferTy, params: Vec<InferTy>, ret: InferTy) -> Self {
        Self {
            self_ty,
            params,
            ret,
        }
    }

    pub(super) fn self_ty(&self) -> &InferTy {
        &self.self_ty
    }

    pub(super) fn params(&self) -> &[InferTy] {
        &self.params
    }

    pub(super) fn ret(&self) -> &InferTy {
        &self.ret
    }
}
