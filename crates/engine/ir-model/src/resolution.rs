use wincode::{SchemaRead, SchemaWrite};

use rg_memsize::MemorySize;

use crate::{TraitRef, TypeAliasRef, TypeDefRef};

/// Type-namespace path resolution shared by semantic and body-local lookup.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub enum TypePathResolution {
    SelfType(Vec<TypeDefRef>),
    TypeDefs(Vec<TypeDefRef>),
    TypeAliases(Vec<TypeAliasRef>),
    Traits(Vec<TraitRef>),
    Unknown,
}
