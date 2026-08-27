//! Shared member identities and field lookup over semantic-shaped item stores.
//!
//! Member lookup is type reasoning, but it does not need source spans or UI labels. This module
//! keeps the stable item refs used by higher layers and the crate-level field query, whose result
//! does not depend on lexical trait scope. Method lookup lives in body context because Rust only
//! makes trait methods callable when their trait is in scope at the use site.
//!
//! Body method lookup still returns the identities defined here. For
//! `String::new().contains("x")`, it checks nominal `String` and then autoderefs to structural
//! `str`, returning the same stable function ref to hover, completion, and goto-definition.

use rg_def_map::DefMapSource;
use rg_ir_model::{FieldRef, FunctionRef, TraitApplicability, TypeDefRef};
use rg_semantic_ir::ItemStoreSource;

use crate::{Autoderef, AutoderefMode, Ty, TyContext};

/// One callable member selected for a receiver type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemberMethodCandidateRef {
    function: FunctionRef,
    origin: MemberMethodOrigin,
}

impl MemberMethodCandidateRef {
    pub fn inherent(function: FunctionRef) -> Self {
        Self {
            function,
            origin: MemberMethodOrigin::Inherent,
        }
    }

    pub fn trait_method(function: FunctionRef, applicability: TraitApplicability) -> Self {
        Self {
            function,
            origin: MemberMethodOrigin::Trait { applicability },
        }
    }

    pub fn function(self) -> FunctionRef {
        self.function
    }

    pub fn origin(self) -> MemberMethodOrigin {
        self.origin
    }
}

/// Why a method candidate is visible on a receiver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberMethodOrigin {
    Inherent,
    Trait { applicability: TraitApplicability },
}

/// Ref-level field lookup shared by analysis and view adapters.
pub struct MemberQuery<'query, D, I> {
    context: TyContext<'query, D, I>,
}

impl<'query, D, I> MemberQuery<'query, D, I>
where
    D: DefMapSource + Clone,
    I: ItemStoreSource<'query, Error = D::Error> + Clone,
{
    /// Creates member lookup in one crate-scoped type-query environment.
    pub fn new(context: TyContext<'query, D, I>) -> Self {
        Self { context }
    }

    /// Returns fields visible after field-lookup autoderef.
    pub fn fields_for_ty(&self, ty: &Ty) -> Result<Vec<FieldRef>, D::Error> {
        let autoderef = Autoderef::new(self.context.clone());
        let mut fields = Vec::new();
        for candidate in autoderef.candidates(AutoderefMode::FieldLookup, ty) {
            let candidate = candidate?;
            for receiver_ty in candidate.ty().as_adts() {
                fields.extend(self.fields_for_type_def(receiver_ty.def)?);
            }
        }
        Ok(fields)
    }

    /// Returns fields declared directly on a nominal type definition.
    pub fn fields_for_type_def(&self, ty: TypeDefRef) -> Result<Vec<FieldRef>, D::Error> {
        self.context.item_paths().items().fields_for_type(ty)
    }
}
