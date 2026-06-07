use wincode::{SchemaRead, SchemaWrite};

use rg_ir_model::{BindingId, identity::DeclarationRef};
use rg_memsize::MemorySize;
use rg_ty::Ty;

/// Resolved facts derived for one expression during body resolution.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub struct ExprFacts {
    pub resolution: BodyResolution,
    pub ty: Ty,
}

impl ExprFacts {
    pub(crate) fn shrink_to_fit(&mut self) {
        self.resolution.shrink_to_fit();
        self.ty.shrink_to_fit();
    }
}

impl Default for ExprFacts {
    fn default() -> Self {
        Self {
            resolution: BodyResolution::Unknown,
            ty: Ty::Unknown,
        }
    }
}

/// Resolved facts derived for one local binding during body resolution.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub struct BindingFacts {
    pub ty: Ty,
}

impl BindingFacts {
    pub(crate) fn shrink_to_fit(&mut self) {
        self.ty.shrink_to_fit();
    }
}

impl Default for BindingFacts {
    fn default() -> Self {
        Self { ty: Ty::Unknown }
    }
}

/// Best-effort semantic resolution attached to body expressions.
#[derive(Debug, Clone, PartialEq, Eq, Default, SchemaRead, SchemaWrite, MemorySize)]
pub enum BodyResolution {
    /// Lexical value binding introduced by a pattern or parameter.
    Binding(BindingId),
    /// Item-like declarations, fields, enum variants, functions, consts, statics, or modules.
    Declarations(Vec<DeclarationRef>),
    #[default]
    Unknown,
}

impl BodyResolution {
    pub(crate) fn shrink_to_fit(&mut self) {
        match self {
            Self::Declarations(declarations) => declarations.shrink_to_fit(),
            Self::Binding(_) | Self::Unknown => {}
        }
    }
}
