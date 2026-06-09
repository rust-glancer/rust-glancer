use wincode::{SchemaRead, SchemaWrite};

use crate::{TraitRef, TypeAliasRef, TypeDefRef};
use rg_std::{MemorySize, UniqueVec};

/// Type-namespace path resolution shared by semantic and body-local lookup.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub enum TypePathResolution {
    SelfType(UniqueVec<TypeDefRef>),
    TypeDefs(UniqueVec<TypeDefRef>),
    TypeAliases(UniqueVec<TypeAliasRef>),
    Traits(UniqueVec<TraitRef>),
    Unknown,
}
