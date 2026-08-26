//! Member lookup over semantic-shaped item stores.
//!
//! Field and method lookup is type reasoning: it needs autoderef, impl-header matching, and one
//! coherent crate visibility/solver context, but it does not need source spans or UI labels. This
//! query returns stable item refs so higher layers can decide how to present them.
//!
//! For `String::new().contains("x")`, lookup first checks the nominal `String` candidate, then
//! autoderefs to `str`. `str` has no `TypeDefRef`, so that second step uses structural impl matching
//! and returns the same stable function ref that hover, completion, and goto-definition consume.

use rg_def_map::DefMapSource;
use rg_ir_model::{FieldRef, FunctionRef, TraitApplicability, TypeDefRef};
use rg_semantic_ir::ItemStoreSource;

use crate::{Autoderef, AutoderefMode, ImplMatcher, Ty, TyContext, inference::InferenceTable};

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

/// Ref-level member lookup shared by analysis and view adapters.
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

    /// Returns method candidates visible after method-receiver autoderef.
    pub fn method_candidates_for_ty(
        &self,
        ty: &Ty,
    ) -> Result<Vec<MemberMethodCandidateRef>, D::Error> {
        // Method autoderef and impl classification are one lookup operation. Sharing their session
        // keeps every trait proof in the same crate-visible solver program and lets later receiver
        // depths reuse exact classifications from earlier ones.
        let autoderef = Autoderef::new(self.context.clone());
        let matcher = ImplMatcher::new(self.context.clone());
        // Public member lookup consumes durable semantic types, so it has no body-owned inference
        // slots to preserve. Body IR supplies its live table through its own lookup projection.
        let table = InferenceTable::new();
        let mut methods = Vec::new();
        for candidate in autoderef.candidates(AutoderefMode::MethodReceiver, ty) {
            let candidate = candidate?;
            let matches = matcher.matches_for_receiver_with_traits(
                candidate.ty(),
                self.context.item_lookup().traits_with_functions(),
                &table,
            )?;
            for function in matcher.function_candidates_for_matches(&matches, None)? {
                let Some(function_data) = self
                    .context
                    .item_paths()
                    .items()
                    .function_data(function.function())?
                else {
                    continue;
                };
                if !function_data.has_self_receiver() {
                    continue;
                }

                let candidate = match function.trait_selection() {
                    Some(selection) => MemberMethodCandidateRef::trait_method(
                        function.function(),
                        selection.applicability,
                    ),
                    None => MemberMethodCandidateRef::inherent(function.function()),
                };
                methods.push(candidate);
            }
        }
        Ok(methods)
    }
}
