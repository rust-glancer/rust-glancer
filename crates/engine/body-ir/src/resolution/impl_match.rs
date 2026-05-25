//! Shallow impl matching for receiver-based Body IR resolution.
//!
//! The matchers own the small amount of generic reasoning used by method lookup and associated
//! items. They compare explicit impl self types against known receiver types and produce the
//! substitutions that make associated signatures readable in the receiver context.

use rg_def_map::DefMapReadTxn;
use rg_item_tree::{GenericArg, GenericParams, TypeRef};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::{
    FunctionRef, ImplRef, ItemOwner, SemanticIrReadTxn, TraitApplicability, TraitImplRef,
    TypePathContext,
};
use rg_text::Name;

use crate::{
    ir::body::BodyData,
    ir::ids::{BodyFunctionRef, BodyRef},
    ir::item::{BodyFunctionOwner, BodyImplData},
    ir::ty::{BodyGenericArg, BodyLocalNominalTy, BodyNominalTy, BodyTy, BodyTyRepr},
};

use super::{
    ty::{
        TypeSubst, body_generic_arg_ty, generic_arg_type_ref, ty_from_type_ref_in_context,
        type_param_name_from_type_ref,
    },
    type_path::BodyTypePathResolver,
};

/// Result of matching one trait impl header against a receiver type.
pub(super) struct TraitImplMatch {
    applicability: TraitApplicability,
    subst: TypeSubst,
}

impl TraitImplMatch {
    /// Creates a match result from the computed confidence and receiver substitutions.
    fn new(applicability: TraitApplicability, subst: TypeSubst) -> Self {
        Self {
            applicability,
            subst,
        }
    }

    /// Confidence that the impl header applies to the receiver.
    pub(super) fn applicability(&self) -> TraitApplicability {
        self.applicability
    }

    /// Splits the result into the match confidence and substitutions for associated signatures.
    pub(super) fn into_parts(self) -> (TraitApplicability, TypeSubst) {
        (self.applicability, self.subst)
    }
}

/// Matcher for module-level impls stored in Semantic IR.
#[derive(Clone, Copy)]
pub(super) struct BodyImplMatcher<'query, 'db> {
    def_map: &'query DefMapReadTxn<'db>,
    semantic_ir: &'query SemanticIrReadTxn<'db>,
}

impl<'query, 'db> BodyImplMatcher<'query, 'db> {
    /// Creates a matcher for impl headers stored in Semantic IR.
    pub(super) fn new(
        def_map: &'query DefMapReadTxn<'db>,
        semantic_ir: &'query SemanticIrReadTxn<'db>,
    ) -> Self {
        Self {
            def_map,
            semantic_ir,
        }
    }

    /// Checks whether a function owned by an inherent impl can be called on the receiver type.
    ///
    /// Trait functions are accepted here because the trait impl candidate already carries the
    /// receiver-specific filtering before a trait function reaches this point.
    pub(super) fn semantic_function_applies_to_receiver(
        &self,
        function_ref: FunctionRef,
        receiver_ty: &BodyNominalTy,
    ) -> Result<bool, PackageStoreError> {
        // Trait items are shared by all impl candidates in the best-effort model. Inherent impl
        // items, however, must at least match the receiver's resolved self type.
        let Some(function_data) = self.semantic_ir.function_data(function_ref)? else {
            return Ok(false);
        };
        let ItemOwner::Impl(impl_id) = function_data.owner else {
            return Ok(true);
        };
        let impl_ref = ImplRef {
            target: function_ref.target,
            id: impl_id,
        };
        let Some(impl_data) = self.semantic_ir.impl_data(impl_ref)? else {
            return Ok(false);
        };
        if !impl_data.resolved_self_tys.contains(&receiver_ty.def) {
            return Ok(false);
        }

        self.impl_self_args_match_receiver(impl_ref, impl_data, receiver_ty)
    }

    /// Matches one semantic trait impl against a receiver.
    ///
    /// For `impl<T> Trait for Wrapper<T>` and receiver `Wrapper<User>`, this returns an
    /// `TraitImplMatch` whose substitutions include `T -> User`.
    pub(super) fn semantic_trait_impl_match(
        &self,
        trait_impl: TraitImplRef,
        receiver_ty: &BodyNominalTy,
    ) -> Result<Option<TraitImplMatch>, PackageStoreError> {
        let Some(impl_data) = self.semantic_ir.impl_data(trait_impl.impl_ref)? else {
            return Ok(None);
        };
        if !impl_data.resolved_self_tys.contains(&receiver_ty.def)
            || !impl_data
                .resolved_trait_refs
                .contains(&trait_impl.trait_ref)
        {
            return Ok(None);
        }

        let header_applicability = if Self::impl_header_is_definitely_direct(impl_data) {
            TraitApplicability::Yes
        } else {
            TraitApplicability::Maybe
        };
        let applicability = header_applicability.and(self.impl_self_args_applicability(
            trait_impl.impl_ref,
            impl_data,
            receiver_ty,
        )?);
        if !applicability.is_applicable() {
            return Ok(None);
        }

        Ok(Some(TraitImplMatch::new(
            applicability,
            self.semantic_impl_self_subst_for_impl(impl_data, receiver_ty),
        )))
    }

    /// Matches one semantic trait impl for contexts that perform a real type adjustment.
    ///
    /// This is stricter than method candidate matching: only direct impl type parameters such as
    /// `Wrapper<T>` are bindable. Nested generic patterns like `Wrapper<Option<T>>`, where clauses,
    /// bounded params, lifetimes, and const generics are rejected until Body IR has a real solver.
    pub(super) fn semantic_trait_impl_structural_match(
        &self,
        trait_impl: TraitImplRef,
        receiver_ty: &BodyNominalTy,
    ) -> Result<Option<TypeSubst>, PackageStoreError> {
        let Some(impl_data) = self.semantic_ir.impl_data(trait_impl.impl_ref)? else {
            return Ok(None);
        };
        if !impl_data.resolved_self_tys.contains(&receiver_ty.def)
            || !impl_data
                .resolved_trait_refs
                .contains(&trait_impl.trait_ref)
        {
            return Ok(None);
        }

        self.impl_self_structural_subst(trait_impl.impl_ref, impl_data, receiver_ty)
    }

    /// Returns only the yes/maybe/no part of `semantic_trait_impl_match`.
    pub(super) fn semantic_trait_impl_applicability(
        &self,
        trait_impl: TraitImplRef,
        receiver_ty: &BodyNominalTy,
    ) -> Result<TraitApplicability, PackageStoreError> {
        Ok(self
            .semantic_trait_impl_match(trait_impl, receiver_ty)?
            .map(|trait_impl_match| trait_impl_match.applicability())
            .unwrap_or(TraitApplicability::No))
    }

    /// Builds impl-header substitutions for a method's owning impl.
    ///
    /// For `impl<U> Wrapper<U> { fn get(&self) -> U }` called on `Wrapper<User>`, this returns
    /// `U -> User` so the method return type can be resolved as `User`.
    pub(super) fn semantic_impl_self_subst(
        &self,
        function_ref: FunctionRef,
        receiver_ty: &BodyNominalTy,
    ) -> TypeSubst {
        // Convert the impl header into substitutions for method signatures. For
        // `impl<U> Wrapper<U>`, a `Wrapper<User>` receiver gives `U -> User`.
        let Ok(Some(function_data)) = self.semantic_ir.function_data(function_ref) else {
            return TypeSubst::new();
        };
        let ItemOwner::Impl(impl_id) = function_data.owner else {
            return TypeSubst::new();
        };
        let Ok(Some(impl_data)) = self.semantic_ir.impl_data(ImplRef {
            target: function_ref.target,
            id: impl_id,
        }) else {
            return TypeSubst::new();
        };

        self.semantic_impl_self_subst_for_impl(impl_data, receiver_ty)
    }

    /// Builds impl-header substitutions from already-loaded semantic impl data.
    ///
    /// This is the associated-item form of `semantic_impl_self_subst`, useful when the caller has
    /// an impl candidate rather than a function reference.
    pub(super) fn semantic_impl_self_subst_for_impl(
        &self,
        impl_data: &rg_semantic_ir::ImplData,
        receiver_ty: &BodyNominalTy,
    ) -> TypeSubst {
        Self::impl_self_subst(&impl_data.generics, &impl_data.self_ty, &receiver_ty.args)
    }

    /// Performs the boolean self-type argument check used by inherent impl methods.
    ///
    /// Type parameters in the impl self type act as wildcards, but concrete arguments must match:
    /// `impl Wrapper<User>` applies to `Wrapper<User>`, not `Wrapper<Project>`.
    fn impl_self_args_match_receiver(
        &self,
        impl_ref: ImplRef,
        impl_data: &rg_semantic_ir::ImplData,
        receiver_ty: &BodyNominalTy,
    ) -> Result<bool, PackageStoreError> {
        // Type parameters in the impl self type act as wildcards. Concrete args such as
        // `impl Wrapper<User>` must equal the receiver's known args.
        let TypeRef::Path(self_ty) = &impl_data.self_ty else {
            return Ok(true);
        };
        let Some(segment) = self_ty.segments.last() else {
            return Ok(true);
        };

        let Some(impl_type_args) = Self::item_tree_type_args(&segment.args) else {
            return Ok(false);
        };
        let Some(receiver_type_args) = Self::body_type_args(&receiver_ty.args) else {
            return Ok(false);
        };
        if impl_type_args.len() != receiver_type_args.len() {
            return Ok(false);
        }

        let impl_type_params = Self::impl_type_param_names(&impl_data.generics);
        for (impl_arg, receiver_arg) in impl_type_args.into_iter().zip(receiver_type_args) {
            if type_param_name_from_type_ref(impl_arg)
                .as_deref()
                .is_some_and(|name| impl_type_params.contains(&name))
            {
                continue;
            }

            let context = TypePathContext {
                module: impl_data.owner,
                impl_ref: Some(impl_ref),
            };
            let impl_arg_ty = ty_from_type_ref_in_context(
                self.def_map,
                self.semantic_ir,
                impl_arg,
                context,
                BodyTyRepr::syntax(impl_arg.clone()),
                &TypeSubst::new(),
            )?;
            if impl_arg_ty != receiver_arg {
                return Ok(false);
            }
        }

        Ok(true)
    }

    /// Performs the trait-impl self-type argument check with uncertainty preserved.
    ///
    /// Generic or syntax-only pieces produce `Maybe` instead of being rejected, because callers can
    /// still show useful trait-method candidates when a full proof is outside Body IR's model.
    fn impl_self_args_applicability(
        &self,
        impl_ref: ImplRef,
        impl_data: &rg_semantic_ir::ImplData,
        receiver_ty: &BodyNominalTy,
    ) -> Result<TraitApplicability, PackageStoreError> {
        // This mirrors inherent impl matching, but returns `Maybe` instead of rejecting patterns
        // that contain generic parameters or unsupported pieces we intentionally do not solve.
        let TypeRef::Path(self_ty) = &impl_data.self_ty else {
            return Ok(TraitApplicability::Maybe);
        };
        let Some(segment) = self_ty.segments.last() else {
            return Ok(TraitApplicability::Maybe);
        };

        let Some(impl_type_args) = Self::item_tree_type_args(&segment.args) else {
            return Ok(TraitApplicability::Maybe);
        };
        let Some(receiver_type_args) = Self::body_type_args(&receiver_ty.args) else {
            return Ok(TraitApplicability::Maybe);
        };
        if impl_type_args.len() != receiver_type_args.len() {
            return Ok(TraitApplicability::Maybe);
        }

        let impl_type_params = Self::impl_type_param_names(&impl_data.generics);
        let mut applicability = TraitApplicability::Yes;

        for (impl_arg, receiver_arg) in impl_type_args.into_iter().zip(receiver_type_args) {
            if impl_arg.mentions_type_param(&impl_type_params) {
                applicability = applicability.and(TraitApplicability::Maybe);
                continue;
            }

            let context = TypePathContext {
                module: impl_data.owner,
                impl_ref: Some(impl_ref),
            };
            let impl_arg_ty = ty_from_type_ref_in_context(
                self.def_map,
                self.semantic_ir,
                impl_arg,
                context,
                BodyTyRepr::syntax(impl_arg.clone()),
                &TypeSubst::new(),
            )?;
            if Self::type_arg_comparison_is_uncertain(&impl_arg_ty)
                || Self::type_arg_comparison_is_uncertain(&receiver_arg)
            {
                applicability = applicability.and(TraitApplicability::Maybe);
                continue;
            }

            if impl_arg_ty != receiver_arg {
                return Ok(TraitApplicability::No);
            }
        }

        Ok(applicability)
    }

    /// Builds substitutions only when the impl self type structurally matches the receiver.
    ///
    /// This intentionally rejects optimistic cases that are acceptable for trait-method UI
    /// candidates. Type adjustments such as `Deref` must not turn an uncertain impl into a real
    /// receiver type.
    fn impl_self_structural_subst(
        &self,
        impl_ref: ImplRef,
        impl_data: &rg_semantic_ir::ImplData,
        receiver_ty: &BodyNominalTy,
    ) -> Result<Option<TypeSubst>, PackageStoreError> {
        if !Self::impl_header_has_only_plain_type_params(impl_data) {
            return Ok(None);
        }

        let TypeRef::Path(self_ty) = &impl_data.self_ty else {
            return Ok(None);
        };
        let Some(segment) = self_ty.segments.last() else {
            return Ok(None);
        };
        if segment.args.len() != receiver_ty.args.len() {
            return Ok(None);
        }

        let impl_type_params = Self::impl_type_param_names(&impl_data.generics);
        let mut subst = TypeSubst::new();

        for (impl_arg, receiver_arg) in segment.args.iter().zip(&receiver_ty.args) {
            let Some(impl_arg) = generic_arg_type_ref(impl_arg) else {
                return Ok(None);
            };
            let Some(receiver_arg) = body_generic_arg_ty(receiver_arg) else {
                return Ok(None);
            };

            if let Some(name) = type_param_name_from_type_ref(impl_arg)
                && impl_type_params.contains(&name.as_str())
            {
                if !Self::push_structural_subst(&mut subst, name, receiver_arg) {
                    return Ok(None);
                }
                continue;
            }

            if impl_arg.mentions_type_param(&impl_type_params) {
                return Ok(None);
            }

            let context = TypePathContext {
                module: impl_data.owner,
                impl_ref: Some(impl_ref),
            };
            let impl_arg_ty = ty_from_type_ref_in_context(
                self.def_map,
                self.semantic_ir,
                impl_arg,
                context,
                BodyTyRepr::syntax(impl_arg.clone()),
                &TypeSubst::new(),
            )?;
            if Self::type_arg_comparison_is_uncertain(&impl_arg_ty)
                || Self::type_arg_comparison_is_uncertain(&receiver_arg)
            {
                return Ok(None);
            }

            if impl_arg_ty != receiver_arg {
                return Ok(None);
            }
        }

        Ok(Some(subst))
    }

    /// Extracts substitutions by aligning impl self type args with receiver type args.
    ///
    /// For `impl<T> Wrapper<T>` matched with `Wrapper<User>`, this returns `T -> User`.
    fn impl_self_subst(
        generics: &GenericParams,
        self_ty: &TypeRef,
        receiver_args: &[crate::ir::ty::BodyGenericArg],
    ) -> TypeSubst {
        let TypeRef::Path(self_ty) = self_ty else {
            return TypeSubst::new();
        };
        let Some(segment) = self_ty.segments.last() else {
            return TypeSubst::new();
        };

        let impl_type_params = Self::impl_type_param_names(generics);
        let receiver_type_args = receiver_args
            .iter()
            .filter_map(body_generic_arg_ty)
            .collect::<Vec<_>>();

        segment
            .args
            .iter()
            .filter_map(generic_arg_type_ref)
            .zip(receiver_type_args)
            .filter_map(|(impl_arg, receiver_arg)| {
                let name = type_param_name_from_type_ref(impl_arg)?;
                impl_type_params
                    .contains(&name.as_str())
                    .then_some((name, receiver_arg))
            })
            .collect()
    }

    /// Records a strict direct-param substitution, rejecting conflicting repeated params.
    fn push_structural_subst(subst: &mut TypeSubst, name: Name, ty: BodyTy) -> bool {
        if let Some((_, existing_ty)) = subst
            .iter()
            .find(|(existing_name, _)| existing_name == &name)
        {
            return existing_ty == &ty;
        }

        subst.push((name, ty));
        true
    }

    /// Lists the type parameters declared by an impl header.
    fn impl_type_param_names(generics: &GenericParams) -> Vec<&str> {
        generics
            .types
            .iter()
            .map(|param| param.name.as_str())
            .collect()
    }

    /// Returns item-tree type args only when no lifetime/const/assoc args were written.
    fn item_tree_type_args(args: &[GenericArg]) -> Option<Vec<&TypeRef>> {
        let mut type_args = Vec::new();
        for arg in args {
            type_args.push(generic_arg_type_ref(arg)?);
        }
        Some(type_args)
    }

    /// Returns Body IR type args only when no lifetime/const/assoc args were preserved.
    fn body_type_args(args: &[BodyGenericArg]) -> Option<Vec<BodyTy>> {
        let mut type_args = Vec::new();
        for arg in args {
            type_args.push(body_generic_arg_ty(arg)?);
        }
        Some(type_args)
    }

    /// Returns whether the impl header has no constraints that require solving.
    fn impl_header_has_only_plain_type_params(impl_data: &rg_semantic_ir::ImplData) -> bool {
        impl_data.generics.lifetimes.is_empty()
            && impl_data.generics.consts.is_empty()
            && impl_data.generics.where_predicates.is_empty()
            && impl_data
                .generics
                .types
                .iter()
                .all(|param| param.bounds.is_empty() && param.default.is_none())
            && impl_data
                .trait_ref
                .as_ref()
                .is_none_or(|trait_ref| !trait_ref.has_generic_args())
    }

    /// Returns whether the impl header has no generic or where-clause uncertainty.
    fn impl_header_is_definitely_direct(impl_data: &rg_semantic_ir::ImplData) -> bool {
        impl_data.generics.lifetimes.is_empty()
            && impl_data.generics.types.is_empty()
            && impl_data.generics.consts.is_empty()
            && impl_data.generics.where_predicates.is_empty()
            && impl_data
                .trait_ref
                .as_ref()
                .is_none_or(|trait_ref| !trait_ref.has_generic_args())
    }

    /// Returns whether comparing this type as a generic argument would overstate certainty.
    fn type_arg_comparison_is_uncertain(ty: &BodyTy) -> bool {
        match ty {
            BodyTy::Repr(BodyTyRepr::Syntax(_)) | BodyTy::Unknown => true,
            BodyTy::Reference { inner, .. } => Self::type_arg_comparison_is_uncertain(inner),
            BodyTy::Unit
            | BodyTy::Never
            | BodyTy::Primitive(_)
            | BodyTy::Repr(
                BodyTyRepr::LocalNominal(_) | BodyTyRepr::Nominal(_) | BodyTyRepr::SelfTy(_),
            ) => false,
        }
    }
}

/// Matcher for impls declared inside a body.
pub(super) struct LocalImplMatcher<'query, 'db, 'body> {
    base: BodyImplMatcher<'query, 'db>,
    body_ref: BodyRef,
    body: &'body BodyData,
}

impl<'query, 'db, 'body> LocalImplMatcher<'query, 'db, 'body> {
    /// Creates a matcher for impls declared inside one body.
    pub(super) fn new(
        def_map: &'query DefMapReadTxn<'db>,
        semantic_ir: &'query SemanticIrReadTxn<'db>,
        body_ref: BodyRef,
        body: &'body BodyData,
    ) -> Self {
        Self {
            base: BodyImplMatcher::new(def_map, semantic_ir),
            body_ref,
            body,
        }
    }

    /// Checks whether a body-local function belongs to an impl that can be called on the receiver.
    ///
    /// For a local `impl<T> Wrapper<T>`, a method candidate is accepted for `Wrapper<User>` and
    /// rejected for unrelated local types.
    pub(super) fn local_function_applies_to_receiver(
        &self,
        function_ref: BodyFunctionRef,
        receiver_ty: &BodyLocalNominalTy,
    ) -> Result<bool, PackageStoreError> {
        // Body-local inherent impls are selected by exact local item identity, then refined by the
        // same shallow generic-argument compatibility rule used for module-level impls.
        if function_ref.body != receiver_ty.item.body {
            return Ok(false);
        }
        let Some(function_data) = self.body.local_function(function_ref.function) else {
            return Ok(false);
        };
        let BodyFunctionOwner::LocalImpl(impl_id) = function_data.owner else {
            return Ok(false);
        };
        let Some(impl_data) = self.body.local_impl(impl_id) else {
            return Ok(false);
        };

        self.local_impl_applies_to_receiver(impl_data, receiver_ty)
    }

    /// Checks whether a body-local inherent impl applies to the receiver type.
    ///
    /// Body-local trait impls are skipped here; this matcher only models inherent local impls.
    pub(super) fn local_impl_applies_to_receiver(
        &self,
        impl_data: &BodyImplData,
        receiver_ty: &BodyLocalNominalTy,
    ) -> Result<bool, PackageStoreError> {
        // Body-local trait impls are an explicit non-goal. They are rare enough that modeling
        // their lookup would add more complexity than useful LSP signal at this stage.
        if impl_data.self_item != Some(receiver_ty.item) || impl_data.trait_ref.is_some() {
            return Ok(false);
        }

        self.local_impl_self_args_match_receiver(impl_data, receiver_ty)
    }

    /// Builds impl-header substitutions for a body-local method's owning impl.
    ///
    /// For local `impl<U> Wrapper<U> { fn get(&self) -> U }` called on `Wrapper<User>`, this
    /// returns `U -> User`.
    pub(super) fn local_impl_self_subst(
        &self,
        function_ref: BodyFunctionRef,
        receiver_ty: &BodyLocalNominalTy,
    ) -> TypeSubst {
        // Convert body-local impl generics into method-signature substitutions. For
        // `impl<U> Wrapper<U>`, a `Wrapper<User>` receiver gives `U -> User`.
        if function_ref.body != receiver_ty.item.body {
            return TypeSubst::new();
        }
        let Some(function_data) = self.body.local_function(function_ref.function) else {
            return TypeSubst::new();
        };
        let BodyFunctionOwner::LocalImpl(impl_id) = function_data.owner else {
            return TypeSubst::new();
        };
        let Some(impl_data) = self.body.local_impl(impl_id) else {
            return TypeSubst::new();
        };

        self.local_impl_self_subst_for_impl(impl_data, receiver_ty)
    }

    /// Builds substitutions from already-loaded body-local impl data.
    ///
    /// This is used by local associated items whose type depends on impl generics, such as
    /// `impl<T> Wrapper<T> { type Item = T; }`.
    pub(super) fn local_impl_self_subst_for_impl(
        &self,
        impl_data: &BodyImplData,
        receiver_ty: &BodyLocalNominalTy,
    ) -> TypeSubst {
        if impl_data.self_item != Some(receiver_ty.item) {
            return TypeSubst::new();
        }

        BodyImplMatcher::impl_self_subst(&impl_data.generics, &impl_data.self_ty, &receiver_ty.args)
    }

    /// Performs the boolean self-type argument check used by body-local inherent impls.
    ///
    /// Type parameters in the impl self type act as wildcards; concrete arguments must equal the
    /// receiver's known arguments.
    fn local_impl_self_args_match_receiver(
        &self,
        impl_data: &BodyImplData,
        receiver_ty: &BodyLocalNominalTy,
    ) -> Result<bool, PackageStoreError> {
        // Local impl matching is intentionally shallow. Type parameters act as wildcards, while
        // concrete args such as `impl Wrapper<User>` must equal the receiver's known args.
        let TypeRef::Path(self_ty) = &impl_data.self_ty else {
            return Ok(true);
        };
        let Some(segment) = self_ty.segments.last() else {
            return Ok(true);
        };

        let Some(impl_type_args) = BodyImplMatcher::item_tree_type_args(&segment.args) else {
            return Ok(false);
        };
        let Some(receiver_type_args) = BodyImplMatcher::body_type_args(&receiver_ty.args) else {
            return Ok(false);
        };
        if impl_type_args.len() != receiver_type_args.len() {
            return Ok(false);
        }

        let impl_type_params = BodyImplMatcher::impl_type_param_names(&impl_data.generics);
        let resolver = BodyTypePathResolver::new(
            self.base.def_map,
            self.base.semantic_ir,
            self.body_ref,
            self.body,
        );
        for (impl_arg, receiver_arg) in impl_type_args.into_iter().zip(receiver_type_args) {
            if type_param_name_from_type_ref(impl_arg)
                .as_deref()
                .is_some_and(|name| impl_type_params.contains(&name))
            {
                continue;
            }

            let impl_arg_ty = resolver.ty_from_type_ref_in_scope_with_subst(
                impl_arg,
                impl_data.scope,
                &TypeSubst::new(),
            )?;
            if impl_arg_ty != receiver_arg {
                return Ok(false);
            }
        }

        Ok(true)
    }
}
