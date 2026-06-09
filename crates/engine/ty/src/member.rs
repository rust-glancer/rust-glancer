//! Member lookup over semantic-shaped item stores.
//!
//! Field and method lookup is type reasoning: it needs autoderef, impl-header matching, and the
//! item/path providers, but it does not need source spans or UI labels. This query returns stable
//! item refs so higher layers can decide how to present them.

use rg_ir_model::{FieldRef, FunctionRef, TraitApplicability, TypeDefRef};
use rg_ir_storage::{DefMapSource, ItemLookupIndex, ItemStoreSource, TargetItemQuery};
use rg_std::UniqueVec;

use crate::{Autoderef, AutoderefMode, ImplMatcher, ItemPathQuery, NominalTy, Ty};

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
    item_paths: ItemPathQuery<'query, D, I>,
    target_items: TargetItemQuery<'query, D, I>,
    lookup_index: Option<&'query ItemLookupIndex>,
}

impl<'query, D, I> MemberQuery<'query, D, I>
where
    D: DefMapSource + Clone,
    I: ItemStoreSource<'query, Error = D::Error> + Clone,
{
    /// Creates a member query that scans the visible item stores directly.
    pub fn new(
        item_paths: ItemPathQuery<'query, D, I>,
        target_items: TargetItemQuery<'query, D, I>,
    ) -> Self {
        Self {
            item_paths,
            target_items,
            lookup_index: None,
        }
    }

    /// Creates a member query that can reuse a precomputed receiver lookup index.
    pub fn with_index(
        item_paths: ItemPathQuery<'query, D, I>,
        target_items: TargetItemQuery<'query, D, I>,
        lookup_index: &'query ItemLookupIndex,
    ) -> Self {
        Self {
            item_paths,
            target_items,
            lookup_index: Some(lookup_index),
        }
    }

    /// Returns fields visible after field-lookup autoderef.
    pub fn fields_for_ty(&self, ty: &Ty) -> Result<Vec<FieldRef>, D::Error> {
        let mut fields = Vec::new();
        for candidate in self.autoderef().candidates(AutoderefMode::FieldLookup, ty) {
            let candidate = candidate?;
            for receiver_ty in candidate.ty().as_nominals() {
                fields.extend(self.fields_for_type_def(receiver_ty.def)?);
            }
        }
        Ok(fields)
    }

    /// Returns fields declared directly on a nominal type definition.
    pub fn fields_for_type_def(&self, ty: TypeDefRef) -> Result<Vec<FieldRef>, D::Error> {
        self.item_paths.items().fields_for_type(ty)
    }

    /// Returns method candidates visible after method-receiver autoderef.
    pub fn method_candidates_for_ty(
        &self,
        ty: &Ty,
    ) -> Result<Vec<MemberMethodCandidateRef>, D::Error> {
        let mut methods = Vec::new();
        for candidate in self
            .autoderef()
            .candidates(AutoderefMode::MethodReceiver, ty)
        {
            let candidate = candidate?;
            for receiver_ty in candidate.ty().as_nominals() {
                methods.extend(self.method_candidates_for_nominal(receiver_ty)?);
            }
        }
        Ok(methods)
    }

    fn method_candidates_for_nominal(
        &self,
        receiver_ty: &NominalTy,
    ) -> Result<Vec<MemberMethodCandidateRef>, D::Error> {
        let mut candidates = Vec::new();
        let matcher = ImplMatcher::new(self.item_paths.clone(), self.target_items.clone());

        for function in self.inherent_functions_for_nominal(receiver_ty)? {
            if !matcher.function_applies_to_receiver(function, receiver_ty)? {
                continue;
            }
            candidates.push(MemberMethodCandidateRef::inherent(function));
        }

        // Trait candidates carry applicability because this project intentionally avoids full
        // solving, but still wants useful editor suggestions for likely matches.
        for (function, applicability) in
            matcher.trait_function_candidates_for_receiver(self.lookup_index, receiver_ty, None)?
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
        receiver_ty: &NominalTy,
    ) -> Result<UniqueVec<FunctionRef>, D::Error> {
        match self.lookup_index {
            Some(index) => {
                index.inherent_functions_for_type(self.item_paths.items(), receiver_ty.def)
            }
            None => self
                .target_items
                .inherent_functions_for_type(receiver_ty.def),
        }
    }

    fn autoderef(&self) -> Autoderef<'query, D, I> {
        match self.lookup_index {
            Some(index) => {
                Autoderef::with_index(self.item_paths.clone(), self.target_items.clone(), index)
            }
            None => Autoderef::new(self.item_paths.clone(), self.target_items.clone()),
        }
    }
}
