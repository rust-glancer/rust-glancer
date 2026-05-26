use rg_ir_model::ResolvedDeclarationRef;
use rg_semantic_ir::{
    EnumVariantRef, FieldRef, FunctionRef, SemanticTypePathResolution, TraitRef, TypeDefRef,
};

use super::ids::{BindingId, BodyEnumVariantRef, BodyFieldRef, BodyFunctionRef, BodyItemRef};

/// Stable field identity across module-level Semantic IR and body-local declarations.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
pub(crate) enum ResolvedFieldRef {
    Semantic(FieldRef),
    BodyLocal(BodyFieldRef),
}

/// Stable function identity across module-level Semantic IR and body-local declarations.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
pub(crate) enum ResolvedFunctionRef {
    Semantic(FunctionRef),
    BodyLocal(BodyFunctionRef),
}

/// Stable enum variant identity across module-level Semantic IR and body-local declarations.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
pub(crate) enum ResolvedEnumVariantRef {
    Semantic(EnumVariantRef),
    BodyLocal(BodyEnumVariantRef),
}

impl From<ResolvedFieldRef> for ResolvedDeclarationRef {
    fn from(field: ResolvedFieldRef) -> Self {
        match field {
            ResolvedFieldRef::Semantic(field) => field.into(),
            ResolvedFieldRef::BodyLocal(field) => field.into(),
        }
    }
}

impl From<ResolvedFunctionRef> for ResolvedDeclarationRef {
    fn from(function: ResolvedFunctionRef) -> Self {
        match function {
            ResolvedFunctionRef::Semantic(function) => function.into(),
            ResolvedFunctionRef::BodyLocal(function) => function.into(),
        }
    }
}

impl From<ResolvedEnumVariantRef> for ResolvedDeclarationRef {
    fn from(variant: ResolvedEnumVariantRef) -> Self {
        match variant {
            ResolvedEnumVariantRef::Semantic(variant) => variant.into(),
            ResolvedEnumVariantRef::BodyLocal(variant) => variant.into(),
        }
    }
}

/// Best-effort semantic resolution attached to body expressions.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Default,
    wincode::SchemaRead,
    wincode::SchemaWrite,
    rg_memsize::MemorySize,
)]
pub enum BodyResolution {
    Local(BindingId),
    Declaration(Vec<ResolvedDeclarationRef>),
    Field(Vec<ResolvedDeclarationRef>),
    /// Associated or free functions resolved through a qualified value path.
    ///
    /// Method calls use `Method` because they start from a receiver expression; this variant is
    /// for value paths like `Type::new` where the type prefix is resolved first.
    Function(Vec<ResolvedDeclarationRef>),
    /// Enum variants are stored inside enum definitions rather than DefMap scopes.
    ///
    /// Keeping them explicit here lets goto/type queries land on the variant declaration while
    /// still reporting the owning enum as the expression type.
    EnumVariant(Vec<ResolvedDeclarationRef>),
    Method(Vec<ResolvedDeclarationRef>),
    #[default]
    Unknown,
}

/// Body-scoped type path resolution result.
#[derive(
    Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite, rg_memsize::MemorySize,
)]
pub enum BodyTypePathResolution {
    BodyLocal(BodyItemRef),
    #[memsize(skip)]
    Primitive(rg_ty::PrimitiveTy),
    SelfType(Vec<TypeDefRef>),
    TypeDefs(Vec<TypeDefRef>),
    Traits(Vec<TraitRef>),
    Unknown,
}

impl BodyTypePathResolution {
    pub fn is_primitive(&self, primitive: &rg_ty::PrimitiveTy) -> bool {
        matches!(self, Self::Primitive(resolved) if resolved == primitive)
    }
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
            Self::Declaration(declarations) => declarations.shrink_to_fit(),
            Self::Field(fields) => fields.shrink_to_fit(),
            Self::Function(functions) | Self::Method(functions) => functions.shrink_to_fit(),
            Self::EnumVariant(variants) => variants.shrink_to_fit(),
            Self::Local(_) | Self::Unknown => {}
        }
    }
}
