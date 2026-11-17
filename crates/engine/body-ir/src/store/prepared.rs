//! The complete semantic preparation of one outer owner and its nested bodies.

use rg_arena::Arena;
use rg_ir_model::{BodyId, BodyRef};
use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

use crate::{BodyData, BodyFacts, BodyLocalItems};

/// All identities are settled before this value is published. Inference can borrow it, but cannot
/// add a nested body, compact a binding arena, or change a local declaration under existing facts.
#[derive(Debug, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct PreparedBodyStore {
    pub(crate) bodies: Arena<BodyId, BodyData>,
    pub(crate) body_refs: Arena<BodyId, BodyRef>,
    pub(crate) local_items: Arena<BodyId, BodyLocalItems>,
}

/// Inferred sidecars in the prepared owner's body order. No source tables are retained here.
#[derive(Debug, Default, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct BodyFactsStore {
    pub(crate) bodies: Arena<BodyId, BodyFacts>,
}
