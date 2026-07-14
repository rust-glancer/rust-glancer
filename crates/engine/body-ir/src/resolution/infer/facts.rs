use std::{marker::PhantomData, sync::Arc};

use rg_ir_model::{BindingId, ExprId};
use rg_ty::{Ty, inference::InferenceTable};

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
#[derive(Clone)]
pub(super) struct InferenceFacts<Id> {
    // Most speculative trait probes only read body facts. Copy-on-write makes that trial snapshot
    // cheap while still isolating the closure facts changed by a probe that does commit evidence.
    facts: Arc<Vec<Ty>>,
    _id: PhantomData<fn(Id)>,
}

impl<Id: InferenceFactId> InferenceFacts<Id> {
    pub(super) fn new(count: usize) -> Self {
        Self {
            facts: Arc::new(vec![Ty::Unknown; count]),
            _id: PhantomData,
        }
    }

    pub(super) fn get(&self, id: Id) -> Ty {
        self.get_ref(id).clone()
    }

    pub(super) fn get_ref(&self, id: Id) -> &Ty {
        &self.facts[id.index()]
    }

    pub(super) fn root_resolved(&self, table: &InferenceTable, id: Id) -> Ty {
        table.resolve_root_var(self.get_ref(id))
    }

    /// Store a fact if its canonical form changed.
    pub(super) fn set(&mut self, table: &InferenceTable, id: Id, ty: Ty) -> bool {
        let previous_ty = table.canonicalize(self.get_ref(id));
        let canonical_ty = table.canonicalize(&ty);
        if previous_ty == canonical_ty {
            return false;
        }

        Arc::make_mut(&mut self.facts)[id.index()] = ty;
        true
    }

    /// Store a new slot even if its weak evidence still canonicalizes to the old shape.
    pub(super) fn set_allowing_weak_slot(
        &mut self,
        table: &InferenceTable,
        id: Id,
        ty: Ty,
    ) -> bool {
        let previous_ty = table.canonicalize(self.get_ref(id));
        let canonical_ty = table.canonicalize(&ty);
        if previous_ty == canonical_ty && !self.get_ref(id).has_var() && ty.has_var() {
            Arc::make_mut(&mut self.facts)[id.index()] = ty;
            return true;
        }

        self.set(table, id, ty)
    }

    pub(super) fn finalize(&self, table: &InferenceTable, id: Id) -> Ty {
        table.finalize(self.get_ref(id))
    }
}
