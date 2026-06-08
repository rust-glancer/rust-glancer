//! Query-facing semantic resolution context.

use rg_ir_model::{ImplRef, ModuleRef};
use rg_std::MemorySize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, MemorySize)]
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
