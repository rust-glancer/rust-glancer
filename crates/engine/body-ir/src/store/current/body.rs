//! A selected editor body borrows the same three-part owner representation as saved analysis.

use std::sync::Arc;

use rg_ir_model::{BodyId, BodyRef, Span};

use crate::{BodyLocalItems, BodyStore, BodyView};

#[derive(Debug)]
pub(crate) struct CurrentBody {
    owner: Arc<BodyStore>,
    slot: BodyId,
}

impl CurrentBody {
    pub(crate) fn for_owner(owner: BodyStore) -> impl Iterator<Item = Self> {
        let owner = Arc::new(owner);
        (0..owner.bodies().len()).map(move |slot| Self {
            owner: owner.clone(),
            slot: BodyId(slot),
        })
    }

    pub(crate) fn body_ref(&self) -> BodyRef {
        self.owner.prepared.body_refs[self.slot]
    }

    pub(crate) fn view(&self) -> BodyView<'_> {
        self.owner
            .body(self.slot)
            .expect("current body belongs to its prepared owner")
    }

    pub(crate) fn local_items(&self) -> &BodyLocalItems {
        &self.owner.prepared.local_items[self.slot]
    }

    pub(crate) fn source_span(&self) -> Span {
        self.view().source().span
    }
}
