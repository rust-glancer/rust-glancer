use rg_std::MemorySize;
use wincode::{SchemaRead, SchemaWrite};

use rg_arena::Arena;
use rg_ir_model::{BindingId, BodyBindingRef, BodyRef, ExprId, identity::DeclarationRef};
use rg_ty::Ty;

/// Pass-derived facts for one resolved body.
#[derive(Debug, Clone, PartialEq, Eq, Default, SchemaRead, SchemaWrite, MemorySize)]
pub struct BodyFacts {
    pub(crate) bindings: Arena<BindingId, BindingFacts>,
    pub(crate) exprs: Arena<ExprId, ExprFacts>,
}

impl BodyFacts {
    pub(crate) fn shrink_to_fit(&mut self) {
        self.bindings.shrink_to_fit();
        for facts in self.bindings.iter_mut() {
            facts.shrink_to_fit();
        }
        self.exprs.shrink_to_fit();
        for facts in self.exprs.iter_mut() {
            facts.shrink_to_fit();
        }
    }
}

/// Resolved facts derived for one expression during body resolution.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub struct ExprFacts {
    pub(crate) resolution: BodyResolution,
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
pub(crate) enum BodyResolution {
    /// Lexical value binding introduced by a pattern or parameter.
    Binding(BindingId),
    /// Item-like declarations, fields, enum variants, functions, consts, statics, or modules.
    Declarations(Vec<DeclarationRef>),
    #[default]
    Unknown,
}

impl BodyResolution {
    pub(crate) fn declarations(&self, body_ref: BodyRef) -> Vec<DeclarationRef> {
        match self {
            Self::Binding(binding) => vec![DeclarationRef::body_binding(BodyBindingRef {
                body: body_ref,
                binding: *binding,
            })],
            Self::Declarations(declarations) => declarations.clone(),
            Self::Unknown => Vec::new(),
        }
    }

    pub(crate) fn shrink_to_fit(&mut self) {
        match self {
            Self::Declarations(declarations) => declarations.shrink_to_fit(),
            Self::Binding(_) | Self::Unknown => {}
        }
    }
}
