//! Receiver matching for inherent impls.
//!
//! Nominal receivers such as `Vec<User>` have a type-definition key that leads directly to their
//! impls. Builtin-shaped receivers do not. In `String::new().contains("x")`, autoderef first visits
//! the nominal `String` and then the unkeyed `str`; `contains` is found by matching that `str`
//! receiver against the headers of core's structural inherent impls.

use rg_def_map::DefMapSource;
use rg_ir_model::{AssocItemId, FunctionRef, ImplRef, ItemOwner, TraitApplicability};
use rg_item_tree::LangItem;
use rg_semantic_ir::{ImplData, ItemStoreSource};

use crate::{AdtTy, Clause, Substitution, Ty, TypePathResolver};

use super::ImplMatcher;

impl<'query, D, I, R> ImplMatcher<'query, D, I, R>
where
    D: DefMapSource,
    I: ItemStoreSource<'query, Error = D::Error>,
    R: TypePathResolver<Error = D::Error>,
{
    pub fn function_applies_to_receiver(
        &self,
        function_ref: FunctionRef,
        receiver_ty: &AdtTy,
    ) -> Result<bool, D::Error> {
        let Some(function_data) = self
            .context
            .item_paths()
            .items()
            .function_data(function_ref)?
        else {
            return Ok(false);
        };
        let ItemOwner::Impl(impl_id) = function_data.owner else {
            return Ok(true);
        };
        let impl_ref = ImplRef {
            origin: function_ref.origin,
            id: impl_id,
        };
        let Some(impl_data) = self.context.item_paths().items().impl_data(impl_ref)? else {
            return Ok(false);
        };
        self.impl_applies_to_receiver(impl_ref, impl_data, receiver_ty)
    }

    pub fn impl_applies_to_receiver(
        &self,
        impl_ref: ImplRef,
        impl_data: &ImplData,
        receiver_ty: &AdtTy,
    ) -> Result<bool, D::Error> {
        if !impl_data.resolved_self_ty.is(&receiver_ty.def) {
            return Ok(false);
        }
        Ok(self
            .impl_self_subst_for_impl(impl_ref, &Ty::adt(receiver_ty.clone()))?
            .is_some_and(|(_, applicability)| applicability.is_applicable()))
    }

    /// Expand applicable unkeyed inherent impls into their associated items and substitutions.
    ///
    /// The impl match belongs here because every consumer must agree that, for example,
    /// `impl u32` contributes both `u32::MAX` and `u32::from_be_bytes`, while `impl<T> [T]`
    /// contributes items with `T` bound from the concrete slice receiver. Callers can then select
    /// constants, static functions, or self-receiver methods without repeating structural lookup.
    pub(crate) fn unkeyed_inherent_item_candidates(
        &self,
        receiver_ty: &Ty,
    ) -> Result<Vec<(ImplRef, AssocItemId, Substitution)>, D::Error> {
        if !receiver_ty.has_unkeyed_self_head() {
            return Ok(Vec::new());
        }

        let item_query = self.context.item_paths().items();
        let mut candidates = Vec::new();

        for impl_ref in self.context.item_lookup().structural_inherent_impls() {
            let Some(impl_data) = item_query.impl_data(impl_ref)? else {
                continue;
            };
            let Some(subst) = self.unkeyed_inherent_impl_subst(impl_ref, impl_data, receiver_ty)?
            else {
                continue;
            };

            candidates.extend(
                impl_data
                    .items
                    .iter()
                    .copied()
                    .map(|item| (impl_ref, item, subst.clone())),
            );
        }

        Ok(candidates)
    }

    /// Expand applicable unkeyed inherent impls into self-receiver functions and substitutions.
    ///
    /// Receiver-based query layers share this operation because the impl data, canonical header
    /// match, and function filtering must agree even though each layer wraps the result differently.
    /// For core's `impl<T> [T]`, the `first` candidate for a `[User]` receiver is returned together
    /// with `T = User`. Ref-only member lookup keeps the function; body lookup also keeps that
    /// substitution so it can instantiate the method signature.
    pub fn unkeyed_inherent_function_candidates(
        &self,
        receiver_ty: &Ty,
        method_name: Option<&str>,
    ) -> Result<Vec<(FunctionRef, Substitution)>, D::Error> {
        let item_query = self.context.item_paths().items();
        let mut candidates = Vec::new();

        for (impl_ref, item, subst) in self.unkeyed_inherent_item_candidates(receiver_ty)? {
            let AssocItemId::Function(id) = item else {
                continue;
            };
            let function = FunctionRef {
                origin: impl_ref.origin,
                id,
            };
            let Some(function_data) = item_query.function_data(function)? else {
                continue;
            };
            if !function_data.has_self_receiver()
                || method_name.is_some_and(|name| function_data.name != name)
            {
                continue;
            }
            candidates.push((function, subst));
        }

        Ok(candidates)
    }

    /// Match an unkeyed inherent impl without accepting unresolved clauses or uncertain shapes.
    fn unkeyed_inherent_impl_subst(
        &self,
        impl_ref: ImplRef,
        impl_data: &ImplData,
        receiver_ty: &Ty,
    ) -> Result<Option<Substitution>, D::Error> {
        if impl_data.trait_ref.is_some() {
            return Ok(None);
        }
        let Some(header) = self.impl_header(impl_ref)? else {
            return Ok(None);
        };
        // `PointeeSized` has no ordinary impl declarations: rustc provides it for every type that
        // can occur behind a pointer. Core writes `impl<T: PointeeSized> *const T` for the main
        // raw-pointer methods, so this is the one predicate we can establish from compiler
        // identity alone. Other predicates still require trait selection and remain unavailable
        // to unkeyed inherent lookup.
        let pointee_sized = self
            .context
            .item_lookup()
            .lang_trait(LangItem::PointeeSized);
        if header.clauses.iter().any(|clause| {
            !matches!(
                clause,
                Clause::Implemented(application)
                    if Some(application.def) == pointee_sized
            )
        }) {
            return Ok(None);
        }
        let Some((subst, applicability)) = Self::impl_self_subst(&header, receiver_ty) else {
            return Ok(None);
        };
        Ok((applicability == TraitApplicability::Yes).then_some(subst))
    }
}
