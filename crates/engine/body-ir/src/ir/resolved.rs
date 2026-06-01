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
    /// Lexical value binding introduced by a pattern or parameter.
    Binding(BindingId),
    /// Item-like declarations, fields, enum variants, functions, consts, statics, or modules.
    Declarations(Vec<DeclarationRef>),
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
            Self::Declarations(declarations) => declarations.shrink_to_fit(),
            Self::Binding(_) | Self::Unknown => {}
        }
    }
}
