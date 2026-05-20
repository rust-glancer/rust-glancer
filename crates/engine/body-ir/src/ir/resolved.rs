use rg_def_map::DefId;
use rg_semantic_ir::{
    EnumVariantRef, FieldRef, FunctionRef, SemanticTypePathResolution, TraitRef, TypeDefRef,
};

use super::ids::{
    BindingId, BodyEnumVariantRef, BodyFieldRef, BodyFunctionRef, BodyItemRef, BodyValueItemRef,
};

/// Stable field identity across module-level Semantic IR and body-local declarations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, wincode::SchemaRead, wincode::SchemaWrite)]
pub enum ResolvedFieldRef {
    Semantic(FieldRef),
    BodyLocal(BodyFieldRef),
}

/// Stable function identity across module-level Semantic IR and body-local declarations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, wincode::SchemaRead, wincode::SchemaWrite)]
pub enum ResolvedFunctionRef {
    Semantic(FunctionRef),
    BodyLocal(BodyFunctionRef),
}

/// Stable enum variant identity across module-level Semantic IR and body-local declarations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, wincode::SchemaRead, wincode::SchemaWrite)]
pub enum ResolvedEnumVariantRef {
    Semantic(EnumVariantRef),
    BodyLocal(BodyEnumVariantRef),
}

/// Best-effort semantic resolution attached to body expressions.
#[derive(Debug, Clone, PartialEq, Eq, Default, wincode::SchemaRead, wincode::SchemaWrite)]
pub enum BodyResolution {
    Local(BindingId),
    LocalItem(BodyItemRef),
    LocalValueItem(BodyValueItemRef),
    Item(Vec<DefId>),
    Field(Vec<ResolvedFieldRef>),
    /// Associated or free functions resolved through a qualified value path.
    ///
    /// Method calls use `Method` because they start from a receiver expression; this variant is
    /// for value paths like `Type::new` where the type prefix is resolved first.
    Function(Vec<ResolvedFunctionRef>),
    /// Enum variants are stored inside enum definitions rather than DefMap scopes.
    ///
    /// Keeping them explicit here lets goto/type queries land on the variant declaration while
    /// still reporting the owning enum as the expression type.
    EnumVariant(Vec<ResolvedEnumVariantRef>),
    Method(Vec<ResolvedFunctionRef>),
    #[default]
    Unknown,
}

/// Body-scoped type path resolution result.
#[derive(Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub enum BodyTypePathResolution {
    BodyLocal(BodyItemRef),
    SelfType(Vec<TypeDefRef>),
    TypeDefs(Vec<TypeDefRef>),
    Traits(Vec<TraitRef>),
    Unknown,
}

impl From<SemanticTypePathResolution> for BodyTypePathResolution {
    fn from(resolution: SemanticTypePathResolution) -> Self {
        match resolution {
            SemanticTypePathResolution::SelfType(types) => Self::SelfType(types),
            SemanticTypePathResolution::TypeDefs(types) => Self::TypeDefs(types),
            SemanticTypePathResolution::Traits(traits) => Self::Traits(traits),
            SemanticTypePathResolution::Unknown => Self::Unknown,
        }
    }
}

impl BodyResolution {
    pub(crate) fn shrink_to_fit(&mut self) {
        match self {
            Self::Item(items) => items.shrink_to_fit(),
            Self::Field(fields) => fields.shrink_to_fit(),
            Self::Function(functions) | Self::Method(functions) => functions.shrink_to_fit(),
            Self::EnumVariant(variants) => variants.shrink_to_fit(),
            Self::Local(_) | Self::LocalItem(_) | Self::LocalValueItem(_) | Self::Unknown => {}
        }
    }
}
