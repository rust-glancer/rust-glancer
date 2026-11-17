use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

use crate::{ConstId, CrateRef, FunctionId, StaticId, declare_id};

declare_id! {
    /// Stable identifier for one lowered body within its owning worklist.
    pub struct BodyId;

    /// Stable identifier for one expression inside a body.
    pub struct ExprId;

    /// Stable identifier for one pattern inside a body.
    pub struct PatId;

    /// Stable identifier for one statement inside a body.
    pub struct StmtId;

    /// Stable identifier for one local binding inside a body.
    pub struct BindingId;

    /// Stable identifier for one lexical scope inside a body.
    pub struct ScopeId;
}

/// A target-level declaration that can start a saved body worklist. Nested declarations
/// already belong to their enclosing root and use local IDs within that result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
pub enum SavedBodyOwner {
    Function(FunctionId),
    Const(ConstId),
    Static(StaticId),
}

/// One independently inferred body worklist, including nested items declared inside its root.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
pub struct BodyKey {
    pub krate: CrateRef,
    pub owner: SavedBodyOwner,
}

impl BodyKey {
    pub fn body(self, body: BodyId) -> BodyRef {
        BodyRef {
            crate_ref: self.krate,
            root: BodyRoot::saved(self.owner),
            body,
        }
    }
}

/// Saved queries own independent arenas. Current-only bodies and local kernel fixtures also
/// have their own identity spaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
pub enum BodyRoot {
    /// Unowned identities used by standalone inference kernels.
    Local,
    Current,
    Function(u32),
    Const(u32),
    Static(u32),
}

impl BodyRoot {
    pub fn saved(owner: SavedBodyOwner) -> Self {
        // Body refs also appear inside expressions, spans and macro work items. Keeping the root
        // index compact limits the growth of values that can carry a body-local identity.
        let compact = |id| u32::try_from(id).expect("target item index fits in u32");
        match owner {
            SavedBodyOwner::Function(id) => Self::Function(compact(id.0)),
            SavedBodyOwner::Const(id) => Self::Const(compact(id.0)),
            SavedBodyOwner::Static(id) => Self::Static(compact(id.0)),
        }
    }

    pub fn owner(self) -> Option<SavedBodyOwner> {
        Some(match self {
            Self::Local | Self::Current => return None,
            Self::Function(id) => SavedBodyOwner::Function(FunctionId(id as usize)),
            Self::Const(id) => SavedBodyOwner::Const(ConstId(id as usize)),
            Self::Static(id) => SavedBodyOwner::Static(StaticId(id as usize)),
        })
    }
}

/// Stable reference to one lowered body across the project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
pub struct BodyRef {
    pub crate_ref: CrateRef,
    pub root: BodyRoot,
    pub body: BodyId,
}

/// Stable reference to one local binding inside a body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
pub struct BodyBindingRef {
    pub body: BodyRef,
    pub binding: BindingId,
}
