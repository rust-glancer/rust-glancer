use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

use crate::{
    ConstId, ConstRef, CrateRef, FunctionId, FunctionRef, ImplId, ItemStoreId, StaticId, StaticRef,
    TraitId, declare_id,
};

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
///
/// The member arena is part of the owner, not just the query key. Selection sets must also
/// distinguish two methods with the same local function ID. Statics only exist in scope arenas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
pub enum SavedBodyOwner {
    Function { store: ItemStoreId, id: FunctionId },
    Const { store: ItemStoreId, id: ConstId },
    Static(StaticId),
}

impl SavedBodyOwner {
    pub fn store(self) -> ItemStoreId {
        match self {
            Self::Function { store, .. } | Self::Const { store, .. } => store,
            Self::Static(_) => ItemStoreId::Scope,
        }
    }
}

// A saved owner's crate comes from BodyKey. These conversions retain the store selector and
// local ID, which together identify the declaration within that crate.
impl From<FunctionRef> for SavedBodyOwner {
    fn from(item: FunctionRef) -> Self {
        Self::Function {
            store: item.store.id,
            id: item.id,
        }
    }
}

impl From<ConstRef> for SavedBodyOwner {
    fn from(item: ConstRef) -> Self {
        Self::Const {
            store: item.store.id,
            id: item.id,
        }
    }
}

impl From<StaticRef> for SavedBodyOwner {
    fn from(item: StaticRef) -> Self {
        Self::Static(item.id)
    }
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
    TraitFunction {
        owner: u32,
        item: u32,
    },
    TraitConst {
        owner: u32,
        item: u32,
    },
    ImplFunction {
        owner: u32,
        item: u32,
    },
    ImplConst {
        owner: u32,
        item: u32,
    },
}

impl BodyRoot {
    pub fn saved(owner: SavedBodyOwner) -> Self {
        // Body refs occur throughout expressions, types and macro work items. Store these few
        // root indexes compactly rather than embedding a full semantic ref (which can itself
        // contain a BodyRef). Nested declarations remain slots within the outer worklist.
        let compact = |id| u32::try_from(id).expect("item index fits in u32");
        match owner {
            SavedBodyOwner::Function {
                store: ItemStoreId::Scope,
                id,
            } => Self::Function(compact(id.0)),
            SavedBodyOwner::Const {
                store: ItemStoreId::Scope,
                id,
            } => Self::Const(compact(id.0)),
            SavedBodyOwner::Static(id) => Self::Static(compact(id.0)),
            SavedBodyOwner::Function {
                store: ItemStoreId::Trait(owner),
                id,
            } => Self::TraitFunction {
                owner: compact(owner.0),
                item: compact(id.0),
            },
            SavedBodyOwner::Const {
                store: ItemStoreId::Trait(owner),
                id,
            } => Self::TraitConst {
                owner: compact(owner.0),
                item: compact(id.0),
            },
            SavedBodyOwner::Function {
                store: ItemStoreId::Impl(owner),
                id,
            } => Self::ImplFunction {
                owner: compact(owner.0),
                item: compact(id.0),
            },
            SavedBodyOwner::Const {
                store: ItemStoreId::Impl(owner),
                id,
            } => Self::ImplConst {
                owner: compact(owner.0),
                item: compact(id.0),
            },
        }
    }

    fn store(self) -> ItemStoreId {
        match self {
            Self::TraitFunction { owner, .. } | Self::TraitConst { owner, .. } => {
                ItemStoreId::Trait(TraitId(owner as usize))
            }
            Self::ImplFunction { owner, .. } | Self::ImplConst { owner, .. } => {
                ItemStoreId::Impl(ImplId(owner as usize))
            }
            _ => ItemStoreId::Scope,
        }
    }

    pub fn owner(self) -> Option<SavedBodyOwner> {
        Some(match self {
            Self::Local | Self::Current => return None,
            Self::Function(id)
            | Self::TraitFunction { item: id, .. }
            | Self::ImplFunction { item: id, .. } => SavedBodyOwner::Function {
                store: self.store(),
                id: FunctionId(id as usize),
            },
            Self::Const(id)
            | Self::TraitConst { item: id, .. }
            | Self::ImplConst { item: id, .. } => SavedBodyOwner::Const {
                store: self.store(),
                id: ConstId(id as usize),
            },
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

impl BodyRef {
    pub fn saved_key(self) -> Option<BodyKey> {
        Some(BodyKey {
            krate: self.crate_ref,
            owner: self.root.owner()?,
        })
    }
}

/// Stable reference to one local binding inside a body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
pub struct BodyBindingRef {
    pub body: BodyRef,
    pub binding: BindingId,
}
