use std::marker::PhantomData;

use rg_ir_model::{BindingId, ExprId};
use rg_ty::{
    Ty,
    inference::{InferTy, InferenceTable},
};

pub(super) trait InferenceFactId: Copy {
    fn index(self) -> usize;
}

impl InferenceFactId for ExprId {
    fn index(self) -> usize {
        self.0
    }
}

impl InferenceFactId for BindingId {
    fn index(self) -> usize {
        self.0
    }
}

/// Body-owned expression or binding inference facts.
pub(super) struct InferenceFacts<Id> {
    facts: Vec<InferTy>,
    _id: PhantomData<fn(Id)>,
}

impl<Id: InferenceFactId> InferenceFacts<Id> {
    pub(super) fn new(count: usize) -> Self {
        Self {
            facts: vec![InferTy::Unknown; count],
            _id: PhantomData,
        }
    }

    pub(super) fn get(&self, id: Id) -> InferTy {
        self.get_ref(id).clone()
    }

    pub(super) fn get_ref(&self, id: Id) -> &InferTy {
        &self.facts[id.index()]
    }

    pub(super) fn root_resolved(&self, table: &InferenceTable, id: Id) -> InferTy {
        table.resolve_root_var(self.get_ref(id))
    }

    /// Store a fact if its canonical form changed.
    pub(super) fn set(&mut self, table: &InferenceTable, id: Id, ty: InferTy) -> bool {
        let previous_ty = table.canonicalize(self.get_ref(id));
        let canonical_ty = table.canonicalize(&ty);
        if previous_ty == canonical_ty {
            return false;
        }

        self.facts[id.index()] = ty;
        true
    }

    /// Store a new slot even if its weak evidence still canonicalizes to the old shape.
    pub(super) fn set_allowing_weak_slot(
        &mut self,
        table: &InferenceTable,
        id: Id,
        ty: InferTy,
    ) -> bool {
        let previous_ty = table.canonicalize(self.get_ref(id));
        let canonical_ty = table.canonicalize(&ty);
        if previous_ty == canonical_ty && !self.get_ref(id).has_var() && ty.has_var() {
            self.facts[id.index()] = ty;
            return true;
        }

        self.set(table, id, ty)
    }

    pub(super) fn finalize(&self, table: &InferenceTable, id: Id) -> Ty {
        table.finalize(self.get_ref(id))
    }
}
