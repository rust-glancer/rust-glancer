//! The complete semantic preparation of one outer owner and its nested bodies.

use rg_arena::Arena;
use rg_ir_model::{BodyId, BodyRef, EvaluatedConstValue};
use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

use crate::{BodyData, BodyFacts, BodyLocalItems};

/// All identities are settled before this value is published. Inference can borrow it, but cannot
/// add a nested body, compact a binding arena, or change a local declaration under existing facts.
#[derive(Debug, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct PreparedBodyStore {
    pub(crate) bodies: Arena<BodyId, BodyData>,
    pub(crate) body_refs: Arena<BodyId, BodyRef>,
    // TODO: Move initializer previews into source storage. Exact preview text still makes edits
    // inside a local initializer conservatively invalidate the enclosing owner's preparation.
    pub(crate) local_items: Arena<BodyId, BodyLocalItems>,
    pub(crate) initializer: Option<EvaluatedConstValue>,
}

/// Inferred sidecars in the prepared owner's body order. No source tables are retained here.
#[derive(Debug, Default, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct BodyFactsStore {
    pub(crate) bodies: Arena<BodyId, BodyFacts>,
}
