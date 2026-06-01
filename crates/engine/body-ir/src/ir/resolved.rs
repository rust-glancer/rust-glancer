use rg_ir_model::{BindingId, TraitRef, TypeAliasRef, TypeDefRef, identity::DeclarationRef};
use rg_semantic_ir::SemanticTypePathResolution;

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
    Declaration(Vec<DeclarationRef>),
    Field(Vec<DeclarationRef>),
    /// Associated or free functions resolved through a qualified value path.
    ///
    /// Method calls use `Method` because they start from a receiver expression; this variant is
    /// for value paths like `Type::new` where the type prefix is resolved first.
    Function(Vec<DeclarationRef>),
    /// Enum variants are stored inside enum definitions rather than DefMap scopes.
    ///
    /// Keeping them explicit here lets goto/type queries land on the variant declaration while
    /// still reporting the owning enum as the expression type.
    EnumVariant(Vec<DeclarationRef>),
    Method(Vec<DeclarationRef>),
    #[default]
    Unknown,
}

/// Body-scoped type path resolution result.
#[derive(
    Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite, rg_memsize::MemorySize,
)]
pub enum BodyTypePathResolution {
    #[memsize(skip)]
    Primitive(rg_ty::PrimitiveTy),
    SelfType(Vec<TypeDefRef>),
    TypeDefs(Vec<TypeDefRef>),
    TypeAliases(Vec<TypeAliasRef>),
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
