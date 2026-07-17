//! Member lookup over semantic-shaped item stores.
//!
//! Field and method lookup is type reasoning: it needs autoderef, impl-header matching, and one
//! coherent crate visibility/solver context, but it does not need source spans or UI labels. This
//! query returns stable item refs so higher layers can decide how to present them.

use rg_def_map::DefMapSource;
use rg_ir_model::{FieldRef, FunctionRef, TraitApplicability, TypeDefRef};
use rg_semantic_ir::ItemStoreSource;
use rg_std::UniqueVec;

use crate::{AdtTy, Autoderef, AutoderefMode, ImplMatcher, Ty, TyContext};

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
        let mut methods = Vec::new();
        for candidate in autoderef.candidates(AutoderefMode::MethodReceiver, ty) {
            let candidate = candidate?;
            for receiver_ty in candidate.ty().as_adts() {
                methods.extend(self.method_candidates_for_nominal(&matcher, receiver_ty)?);
            }
        }
        Ok(methods)
    }

    fn method_candidates_for_nominal(
        &self,
        matcher: &ImplMatcher<'query, D, I>,
        receiver_ty: &AdtTy,
    ) -> Result<Vec<MemberMethodCandidateRef>, D::Error> {
        let mut candidates = Vec::new();

        for function in self.inherent_functions_for_nominal(receiver_ty)? {
            if !matcher.function_applies_to_receiver(function, receiver_ty)? {
                continue;
            }
            candidates.push(MemberMethodCandidateRef::inherent(function));
        }

        // Keep proof confidence with each trait method. Editor lookup can then distinguish a
        // proved impl from one retained because Chalk reported ambiguity or unsupported evidence.
        for (function, applicability) in
            matcher.trait_function_candidates_for_receiver(receiver_ty, None)?
        {
            candidates.push(MemberMethodCandidateRef::trait_method(
                function,
                applicability,
            ));
        }

        Ok(candidates)
    }

    fn inherent_functions_for_nominal(
        &self,
        receiver_ty: &AdtTy,
    ) -> Result<UniqueVec<FunctionRef>, D::Error> {
        self.context
            .lookup_index()
            .inherent_functions_for_type(self.context.item_paths().items(), receiver_ty.def)
    }
}
