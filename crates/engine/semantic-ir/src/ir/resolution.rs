//! Query-facing semantic resolution context.

use rg_ir_model::ModuleRef;

use rg_ir_model::ImplRef;

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
