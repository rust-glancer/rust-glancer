//! Receiver-based function lookup for a body use site.
//!
//! A body has two item layers: target-visible semantic items and the active body's local item
//! overlay. Method lookup should not care which layer produced an impl candidate after visibility
//! has been decided, so this query merges both layers before returning ref-level candidates.

use rg_ir_model::FunctionRef;
use rg_ir_storage::{
    DefMapSource, ItemLookupIndex, ItemStoreQuery, ItemStoreSource, TargetItemQuery,
};
use rg_package_store::PackageStoreError;
use rg_ty::{
    Autoderef, AutoderefMode, ImplMatcher, ItemPathQuery, MemberMethodCandidateRef,
    MemberMethodOrigin, NominalTy, Ty,
};

use super::{BodyLocalItemQuery, BodyQuerySource, push_unique};

pub(crate) struct BodyReceiverFunctionQuery<'query, D, I> {
    source: BodyQuerySource<'query, D, I>,
    semantic_index: Option<&'query ItemLookupIndex>,
}

impl<'query, D, I> BodyReceiverFunctionQuery<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    pub(super) fn new(
        source: BodyQuerySource<'query, D, I>,
        semantic_index: Option<&'query ItemLookupIndex>,
    ) -> Self {
        Self {
            source,
            semantic_index,
        }
    }

    pub(super) fn method_candidates_for_ty(
        &self,
        ty: &Ty,
    ) -> Result<Vec<MemberMethodCandidateRef>, PackageStoreError> {
        let source = self.source;
        let item_paths = ItemPathQuery::new(source, source);
        let target_items = TargetItemQuery::new(source, source, self.source.body_ref().target);
        let autoderef = match self.semantic_index {
            Some(index) => Autoderef::with_index(item_paths, target_items, index),
            None => Autoderef::new(item_paths, target_items),
        };

        let mut candidates = Vec::new();
        for candidate in autoderef.candidates(AutoderefMode::MethodReceiver, ty) {
            let candidate = candidate?;
            for receiver_ty in candidate.ty().as_nominals() {
                for method in self.function_candidates_for_receiver(receiver_ty, None)? {
                    Self::push_candidate(&mut candidates, method);
                }
            }
        }

        Ok(candidates)
    }

    pub(super) fn function_refs_for_receiver(
        &self,
        receiver_ty: &NominalTy,
        method_name: Option<&str>,
    ) -> Result<Vec<FunctionRef>, PackageStoreError> {
        let mut functions = Vec::new();
        for candidate in self.function_candidates_for_receiver(receiver_ty, method_name)? {
            push_unique(&mut functions, candidate.function());
        }
        Ok(functions)
    }

    pub(super) fn function_candidates_for_receiver(
        &self,
        receiver_ty: &NominalTy,
        method_name: Option<&str>,
    ) -> Result<Vec<MemberMethodCandidateRef>, PackageStoreError> {
        let source = self.source;
        let item_paths = ItemPathQuery::new(source, source);
        let target_items = TargetItemQuery::new(source, source, self.source.body_ref().target);
        let matcher = ImplMatcher::new(item_paths.clone(), target_items.clone());
        let body_items = BodyLocalItemQuery::new(source);
        let mut candidates = Vec::new();

        for function in self.body_inherent_functions(&body_items, receiver_ty, method_name)? {
            if matcher.function_applies_to_receiver(function, receiver_ty)? {
                Self::push_candidate(
                    &mut candidates,
                    MemberMethodCandidateRef::inherent(function),
                );
            }
        }

        if receiver_ty.def.origin.as_target_ref().is_some() {
            for function in self.semantic_inherent_functions(receiver_ty, method_name)? {
                if matcher.function_applies_to_receiver(function, receiver_ty)? {
                    Self::push_candidate(
                        &mut candidates,
                        MemberMethodCandidateRef::inherent(function),
                    );
                }
            }
        }

        let body_trait_impls = body_items.trait_impls_for_type(receiver_ty.def)?;
        for (function, applicability) in matcher.trait_function_candidates_from_impls(
            self.semantic_index,
            body_trait_impls,
            receiver_ty,
            method_name,
        )? {
            Self::push_candidate(
                &mut candidates,
                MemberMethodCandidateRef::trait_method(function, applicability),
            );
        }

        if receiver_ty.def.origin.as_target_ref().is_some() {
            for (function, applicability) in matcher.trait_function_candidates_for_receiver(
                self.semantic_index,
                receiver_ty,
                method_name,
            )? {
                Self::push_candidate(
                    &mut candidates,
                    MemberMethodCandidateRef::trait_method(function, applicability),
                );
            }
        }

        Ok(candidates)
    }

    fn body_inherent_functions(
        &self,
        body_items: &BodyLocalItemQuery<'query, D, I>,
        receiver_ty: &NominalTy,
        method_name: Option<&str>,
    ) -> Result<Vec<FunctionRef>, PackageStoreError> {
        match method_name {
            Some(name) => body_items.inherent_functions_for_type_and_name(receiver_ty.def, name),
            None => body_items.inherent_functions_for_type(receiver_ty.def),
        }
    }

    fn semantic_inherent_functions(
        &self,
        receiver_ty: &NominalTy,
        method_name: Option<&str>,
    ) -> Result<Vec<FunctionRef>, PackageStoreError> {
        let source = self.source;
        match (self.semantic_index, method_name) {
            (Some(index), Some(name)) => Ok(index
                .inherent_functions_for_type_and_name(receiver_ty.def, name)
                .to_vec()),
            (Some(index), None) => {
                let item_query = ItemStoreQuery::new(source);
                index.inherent_functions_for_type(&item_query, receiver_ty.def)
            }
            (None, method_name) => {
                let target_items =
                    TargetItemQuery::new(source, source, self.source.body_ref().target);
                let functions = target_items.inherent_functions_for_type(receiver_ty.def)?;
                self.filter_functions_by_name(functions, method_name)
            }
        }
    }

    fn filter_functions_by_name(
        &self,
        functions: Vec<FunctionRef>,
        name: Option<&str>,
    ) -> Result<Vec<FunctionRef>, PackageStoreError> {
        let Some(name) = name else {
            return Ok(functions);
        };

        let item_query = ItemStoreQuery::new(self.source);
        let mut retained = Vec::new();
        for function in functions {
            let Some(function_data) = item_query.function_data(function)? else {
                continue;
            };
            if function_data.name == name {
                retained.push(function);
            }
        }
        Ok(retained)
    }

    fn push_candidate(
        candidates: &mut Vec<MemberMethodCandidateRef>,
        candidate: MemberMethodCandidateRef,
    ) {
        let Some(existing) = candidates
            .iter_mut()
            .find(|existing| existing.function() == candidate.function())
        else {
            candidates.push(candidate);
            return;
        };

        *existing = Self::merge_candidates(*existing, candidate);
    }

    fn merge_candidates(
        left: MemberMethodCandidateRef,
        right: MemberMethodCandidateRef,
    ) -> MemberMethodCandidateRef {
        match (left.origin(), right.origin()) {
            (MemberMethodOrigin::Inherent, _) => left,
            (_, MemberMethodOrigin::Inherent) => right,
            (
                MemberMethodOrigin::Trait {
                    applicability: left_applicability,
                },
                MemberMethodOrigin::Trait {
                    applicability: right_applicability,
                },
            ) => MemberMethodCandidateRef::trait_method(
                left.function(),
                left_applicability.or(right_applicability),
            ),
        }
    }
}
