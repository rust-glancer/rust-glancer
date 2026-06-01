use rg_ir_model::{BindingId, identity::DeclarationRef};

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

impl BodyResolution {
    pub(crate) fn shrink_to_fit(&mut self) {
        match self {
            Self::Declarations(declarations) => declarations.shrink_to_fit(),
            Self::Binding(_) | Self::Unknown => {}
        }
    }
}
