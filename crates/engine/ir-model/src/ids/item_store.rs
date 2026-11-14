use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

use crate::{DefMapRef, ImplId, TraitId};

/// Selects the scope's own item arenas or the members of one trait/impl declared in it.
///
/// `Impl(id)` selects the declarations inside that block. The impl header itself belongs to
/// `Scope`, where `id` identifies the header. The same rule applies to traits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
pub enum ItemStoreId {
    /// Free declarations and trait/impl headers in the enclosing crate or body scope.
    Scope,
    Trait(TraitId),
    Impl(ImplId),
}

/// Where a semantic item's local ID is meaningful.
///
/// `impl A { fn first() {} }` and `impl B { fn first() {} }` can both allocate `FunctionId(0)`.
/// Their store IDs distinguish the functions; the enclosing scope and `FunctionId` alone cannot.
/// `origin` still selects that enclosing definition scope for name resolution. A member arena
/// does not introduce a Rust module or change which names a declaration can resolve.
///
/// Body-local traits and impls use the same rule, with a body definition scope as their origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[shrink(leaf)]
pub struct ItemStoreRef {
    pub origin: DefMapRef,
    pub id: ItemStoreId,
}

impl ItemStoreRef {
    pub fn scope(origin: DefMapRef) -> Self {
        Self {
            origin,
            id: ItemStoreId::Scope,
        }
    }
}
