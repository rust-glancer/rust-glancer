use wincode::{SchemaRead, SchemaWrite};

use crate::{TraitRef, TypeAliasRef, TypeDefRef};
use rg_std::MemorySize;

/// Type-namespace path resolution shared by semantic and body-local lookup.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub enum TypePathResolution {
    SelfType(Vec<TypeDefRef>),
    TypeDefs(Vec<TypeDefRef>),
    TypeAliases(Vec<TypeAliasRef>),
    Traits(Vec<TraitRef>),
    Unknown,
}
