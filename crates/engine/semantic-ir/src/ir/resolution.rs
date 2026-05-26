//! Query-facing semantic resolution result types.

use rg_ir_model::ModuleRef;

use super::ids::{ImplRef, TraitRef, TypeDefRef};

#[derive(Debug, Clone, Copy, PartialEq, Eq, rg_memsize::MemorySize)]
pub struct TypePathContext {
    pub module: ModuleRef,
    pub impl_ref: Option<ImplRef>,
}

impl TypePathContext {
    pub fn module(module: ModuleRef) -> Self {
        Self {
            module,
            impl_ref: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, rg_memsize::MemorySize)]
pub enum SemanticTypePathResolution {
    SelfType(Vec<TypeDefRef>),
    TypeDefs(Vec<TypeDefRef>),
    Traits(Vec<TraitRef>),
    Unknown,
}
