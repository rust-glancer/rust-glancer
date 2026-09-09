//! Query-facing semantic resolution context.

use rg_ir_model::{ImplRef, ModuleRef, TraitDefRef, TypeDefRef};
use rg_std::MemorySize;

/// Module lookup and the declaration that gives `Self` its meaning.
///
/// Keep the owner identity alongside the module. Resolving `Self` through the owner's name
/// would lose that binding when the same spelling also names a value in the module.
#[derive(Debug, Clone, Copy, PartialEq, Eq, MemorySize)]
pub struct TypePathContext {
    pub module: ModuleRef,
    pub self_owner: Option<SelfTypeOwner>,
}

impl TypePathContext {
    pub fn module(module: ModuleRef) -> Self {
        Self {
            module,
            self_owner: None,
        }
    }

    pub fn impl_ref(self) -> Option<ImplRef> {
        match self.self_owner {
            Some(SelfTypeOwner::Impl(impl_ref)) => Some(impl_ref),
            _ => None,
        }
    }
}

/// The declaration that binds `Self` in a type-path context.
///
/// Types and traits already have the identity needed for declaration lookup. An impl retains
/// its own identity because its receiver may be generic, primitive, or structural; the type
/// lowerer needs the complete receiver spelling from that impl.
#[derive(Debug, Clone, Copy, PartialEq, Eq, MemorySize)]
pub enum SelfTypeOwner {
    TypeDef(TypeDefRef),
    Trait(TraitDefRef),
    Impl(ImplRef),
}
