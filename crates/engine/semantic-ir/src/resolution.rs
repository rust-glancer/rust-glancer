//! Query-facing semantic resolution result types.

use rg_def_map::ModuleRef;

use crate::ids::{ImplRef, TraitRef, TypeDefRef};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SemanticTypePathResolution {
    SelfType(Vec<TypeDefRef>),
    TypeDefs(Vec<TypeDefRef>),
    Traits(Vec<TraitRef>),
    Unknown,
}
