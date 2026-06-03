use crate::{TraitRef, TypeAliasRef, TypeDefRef};

/// Type-namespace path resolution shared by semantic and body-local lookup.
#[derive(
    Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite, rg_memsize::MemorySize,
)]
pub enum TypePathResolution {
    SelfType(Vec<TypeDefRef>),
    TypeDefs(Vec<TypeDefRef>),
    TypeAliases(Vec<TypeAliasRef>),
    Traits(Vec<TraitRef>),
    Unknown,
}
