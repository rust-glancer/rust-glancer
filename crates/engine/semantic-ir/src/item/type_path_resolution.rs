use wincode::{SchemaRead, SchemaWrite};

use rg_ir_model::{TraitDefRef, TypeAliasRef, TypeDefRef};
use rg_std::{ExpectedUnique, MemorySize};

/// Definition-level result of resolving syntax in the type namespace.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub enum TypePathResolution {
    SelfType(TypeDefRef),
    TypeDef(TypeDefRef),
    TypeAlias(TypeAliasRef),
    Trait(TraitDefRef),
    Unknown,
}

impl TypePathResolution {
    pub fn self_type(candidate: ExpectedUnique<TypeDefRef>) -> Self {
        candidate
            .into_option()
            .map(Self::SelfType)
            .unwrap_or(Self::Unknown)
    }

    pub fn type_def(candidate: ExpectedUnique<TypeDefRef>) -> Self {
        candidate
            .into_option()
            .map(Self::TypeDef)
            .unwrap_or(Self::Unknown)
    }

    pub fn type_alias(candidate: ExpectedUnique<TypeAliasRef>) -> Self {
        candidate
            .into_option()
            .map(Self::TypeAlias)
            .unwrap_or(Self::Unknown)
    }

    pub fn trait_ref(candidate: ExpectedUnique<TraitDefRef>) -> Self {
        candidate
            .into_option()
            .map(Self::Trait)
            .unwrap_or(Self::Unknown)
    }
}
