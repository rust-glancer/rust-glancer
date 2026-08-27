//! Associated item lookup in value position.

use rg_def_map::DefMapSource;
use rg_ir_model::{
    AssocItemId, ConstRef, DefMapRef, EnumVariantRef, ItemOwner, Path, PrimitiveTy, ScopeId,
    TraitApplicability, TypeDefId, identity::DeclarationRef,
};
use rg_item_tree::TypeRef;
use rg_package_store::PackageStoreError;
use rg_semantic_ir::ItemStoreSource;
use rg_std::{ExpectedUnique, UniqueVec};
use rg_ty::{
    AdtTy, ExpectedTyExt, ReceiverImplMatches, Substitution, TraitSelection, Ty,
    inference::InferenceTable,
};
use rg_ty::{
    AssociatedItemCandidateRef, AssociatedItemQuery, AssociatedItemRef, TraitApplication,
    TypeLoweringAnchor, TypeLoweringEnv, TypeLoweringQuery,
};

use super::{BodyCallableCandidate, BodyReceiverImplMatches, traits::BodyQualifiedTraitSelection};

use crate::{
    BodyAssociatedPathPrefix, BodyPath, ir::resolved::BodyResolution,
    resolution::BodyResolutionContext,
};

/// Resolves `Type::item` paths in value position.
///
/// Covers enum variants, associated consts, and associated functions.
pub(crate) struct BodyAssociatedItemQuery<'query, D, I> {
    context: BodyResolutionContext<'query, D, I>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum BodyAssociatedItemCandidate {
    EnumVariant(EnumVariantRef, Ty),
    Const(ConstRef, Ty),
}

impl<'query, D, I> BodyAssociatedItemQuery<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    pub(crate) fn new(context: BodyResolutionContext<'query, D, I>) -> Self {
        Self { context }
    }

    /// Enumerate declarations reachable through a type-qualified path prefix.
    ///
    /// ```text
    /// fn build<T: Factory>() {
    ///     T::/* items from Factory and its supertraits */
    ///     Widget::<u8>::/* variants, inherent items, and matching trait items */
    /// }
    /// ```
    ///
    /// Unlike crate-level signature lookup, this query also sees types and impls declared inside
    /// the active body. The returned identities use the shared `rg_ty` candidate vocabulary, so
    /// local and crate-level declarations follow the same downstream lookup path.
    pub(crate) fn candidates_for_prefix(
        &self,
        scope: ScopeId,
        prefix: &BodyAssociatedPathPrefix,
    ) -> Result<Vec<AssociatedItemCandidateRef>, PackageStoreError> {
        let item_paths = self.context.item_paths();
        let lowering = TypeLoweringQuery::new(&item_paths, &self.context);
        let env = TypeLoweringEnv::new(
            self.context.body().owner().generic_def(),
            TypeLoweringAnchor::Scope(scope),
        );

        match prefix {
            BodyAssociatedPathPrefix::Type(prefix_ty_ref) => {
                // A type-shaped prefix may contribute from a nominal receiver, bounds on that
                // receiver, or the trait declaration named by the prefix.
                let prefix_ty = lowering.lower(prefix_ty_ref, env.clone())?;
                let mut session = lowering.session(env)?;
                let owner_traits = session.trait_applications_for_type(&prefix_ty)?;
                let direct_trait = session.lower_trait_ref(prefix_ty_ref, prefix_ty.clone())?;

                let mut candidates = self.candidates_for_ty(scope, &prefix_ty)?;
                candidates.extend(self.candidates_for_trait_applications(owner_traits)?);
                if let Some(direct_trait) = direct_trait {
                    candidates.extend(
                        self.candidates_for_trait_applications([direct_trait.application])?,
                    );
                }
                Ok(candidates)
            }
            BodyAssociatedPathPrefix::QualifiedTrait { self_ty, trait_ref } => {
                let self_ty = lowering.lower(self_ty, env.clone())?;
                let mut session = lowering.session(env)?;
                let Some(trait_ref) = session.lower_trait_ref(trait_ref, self_ty)? else {
                    return Ok(Vec::new());
                };
                self.candidates_for_trait_applications([trait_ref.application])
            }
        }
    }

    /// Enumerate declarations from one explicitly written trait and its supertraits.
    ///
    /// Associated type binding syntax must not fall back to inherent items on a nominal type that
    /// happens to resolve from the same path spelling.
    pub(crate) fn candidates_for_trait_ref(
        &self,
        scope: ScopeId,
        trait_ref: &TypeRef,
    ) -> Result<Vec<AssociatedItemCandidateRef>, PackageStoreError> {
        let item_paths = self.context.item_paths();
        let lowering = TypeLoweringQuery::new(&item_paths, &self.context);
        let env = TypeLoweringEnv::new(
            self.context.body().owner().generic_def(),
            TypeLoweringAnchor::Scope(scope),
        );
        let mut session = lowering.session(env)?;
        let Some(trait_ref) = session.lower_trait_ref(trait_ref, Ty::Unknown)? else {
            return Ok(Vec::new());
        };
        self.candidates_for_trait_applications([trait_ref.application])
    }

    /// Combine body-local impls with crate-indexed impls for one lowered prefix type.
    fn candidates_for_ty(
        &self,
        scope: ScopeId,
        ty: &Ty,
    ) -> Result<Vec<AssociatedItemCandidateRef>, PackageStoreError> {
        let query = AssociatedItemQuery::with_resolver(self.context.ty_context(), &self.context);
        let table = InferenceTable::new();
        let receiver = self
            .context
            .impls()
            .matches_for_receiver_with_associated_items(scope, ty, &table)?;
        let item_query = self.context.item_query();
        let mut candidates = Vec::new();
        for candidate in query.candidates_for_matches(receiver.receiver_ty(), receiver.matches())? {
            let shadows_current_item = match candidate.item() {
                AssociatedItemRef::Function(function) => {
                    item_query.function_data(function)?.is_some_and(|data| {
                        receiver
                            .saved_function_name_is_shadowed(function.origin, data.name.as_str())
                    })
                }
                AssociatedItemRef::Const(konst) => {
                    item_query.const_data(konst)?.is_some_and(|data| {
                        receiver.saved_const_name_is_shadowed(konst.origin, data.name.as_str())
                    })
                }
                AssociatedItemRef::TypeAlias(alias) => {
                    item_query.type_alias_data(alias)?.is_some_and(|data| {
                        receiver.saved_type_alias_name_is_shadowed(alias.origin, data.name.as_str())
                    })
                }
                AssociatedItemRef::EnumVariant(_) => false,
            };
            if !shadows_current_item {
                candidates.push(candidate);
            }
        }
        Ok(candidates)
    }

    fn candidates_for_trait_applications(
        &self,
        applications: impl IntoIterator<Item = TraitApplication>,
    ) -> Result<Vec<AssociatedItemCandidateRef>, PackageStoreError> {
        AssociatedItemQuery::with_resolver(self.context.ty_context(), &self.context)
            .candidates_for_trait_applications(applications, TraitApplicability::Yes)
    }

    /// Resolve an associated value path from a type prefix and a final item name.
    pub(crate) fn resolve_path(
        &self,
        scope: ScopeId,
        prefix: &Path,
        last_segment: &str,
    ) -> Result<Option<(BodyResolution, Ty)>, PackageStoreError> {
        // Associated item paths are resolved as "type prefix + value member". This keeps
        // `Action::Start` distinct from a module path while also handling `Widget::new` through
        // the same type-substitution rules used by method calls.
        let prefix_resolution = self
            .context
            .type_path_query()
            .resolve_in_scope(scope, prefix)?;
        let prefix_ty = Ty::from_type_path_resolution(prefix_resolution, Vec::new())
            .or_else(|| {
                prefix
                    .single_name()
                    .and_then(PrimitiveTy::from_name)
                    .map(Ty::Primitive)
            })
            .unwrap_or(Ty::Unknown);
        self.resolve_for_type(scope, &prefix_ty, last_segment)
    }

    /// Resolve an associated value path that may use rich body syntax.
    pub(crate) fn resolve_body_path(
        &self,
        scope: ScopeId,
        path: &BodyPath,
    ) -> Result<Option<(BodyResolution, Ty)>, PackageStoreError> {
        let Some((prefix, last_segment)) = path.split_associated_item_prefix_name() else {
            return Ok(None);
        };

        match prefix {
            BodyAssociatedPathPrefix::Type(prefix_ty_ref) => {
                let prefix_ty = self.context.type_refs(scope).resolve(&prefix_ty_ref)?;
                self.resolve_for_type(scope, &prefix_ty, last_segment)
            }
            BodyAssociatedPathPrefix::QualifiedTrait { self_ty, trait_ref } => {
                let Some(selection) = self
                    .context
                    .traits()
                    .qualified_selection(scope, &self_ty, &trait_ref)?
                else {
                    return Ok(None);
                };
                let consts = self.qualified_trait_const_candidates(&selection, last_segment)?;
                if !consts.is_empty() {
                    return Ok(Some(Self::const_resolution(consts)));
                }

                let table = InferenceTable::new();
                let functions =
                    self.qualified_trait_function_candidates(&selection, last_segment, &table)?;
                Ok((!functions.is_empty()).then_some(Self::function_resolution(functions)))
            }
        }
    }

    /// Resolve an associated value path after its type prefix has already been typed.
    pub(crate) fn resolve_for_type(
        &self,
        scope: ScopeId,
        prefix_ty: &Ty,
        last_segment: &str,
    ) -> Result<Option<(BodyResolution, Ty)>, PackageStoreError> {
        let table = InferenceTable::new();
        let const_receiver = self.context.impls().matches_for_receiver_with_const_name(
            scope,
            prefix_ty,
            last_segment,
            &table,
        )?;

        // First treat the final segment as an enum variant. Variants are not ordinary associated
        // functions in either Semantic IR or Body IR, but value paths use the same syntax for
        // `Action::Start` and `Widget::new`, so they need an explicit pass.
        let mut variants = Vec::new();
        for nominal_ty in const_receiver.receiver_ty().as_adts() {
            if let Some(candidate) =
                self.enum_variant_candidate_for_type(nominal_ty, last_segment)?
            {
                variants.push(candidate);
            }
        }

        if !variants.is_empty() {
            return Ok(Some(Self::enum_variant_resolution(variants)));
        }

        if let Some(candidate) =
            self.inherent_associated_const_candidate(&const_receiver, last_segment)?
        {
            return Ok(Some(Self::const_resolution([candidate])));
        }

        // Trait associated const lookup is receiver-driven: `Type::CONST` submits each matching
        // impl to the shared selection boundary, then reads the concrete const from that impl or
        // falls back to the trait declaration. An unavailable proof remains a tentative lookup
        // candidate for editor features; definite rejection removes it.
        let trait_consts = self.trait_associated_const_candidates(
            const_receiver.matches().traits(),
            const_receiver.receiver_ty(),
            last_segment,
            None,
        )?;

        if !trait_consts.is_empty() {
            return Ok(Some(Self::const_resolution(trait_consts)));
        }

        // Inherent associated functions are exact candidates. Trait-associated functions retain
        // tentative matches when incomplete generic information cannot prove or reject the impl.
        let function_receiver = self
            .context
            .impls()
            .matches_for_receiver_with_function_name(scope, prefix_ty, last_segment, &table)?;
        let functions = self.associated_function_candidates_for_matches(
            &function_receiver,
            last_segment,
            None,
        )?;

        Ok((!functions.is_empty()).then_some(Self::function_resolution(functions)))
    }

    /// Return associated functions selected by a typed prefix.
    pub(crate) fn function_candidates_for_type(
        &self,
        scope: ScopeId,
        prefix_ty: &Ty,
        name: &str,
        table: &InferenceTable,
    ) -> Result<UniqueVec<BodyCallableCandidate>, PackageStoreError> {
        let receiver = self
            .context
            .impls()
            .matches_for_receiver_with_function_name(scope, prefix_ty, name, table)?;
        self.associated_function_candidates_for_matches(&receiver, name, None)
    }

    /// Return associated function candidates selected by a rich body path.
    pub(crate) fn function_candidates_for_body_path(
        &self,
        scope: ScopeId,
        path: &BodyPath,
        table: &InferenceTable,
    ) -> Result<UniqueVec<BodyCallableCandidate>, PackageStoreError> {
        let Some((prefix, name)) = path.split_associated_item_prefix_name() else {
            return Ok(UniqueVec::new());
        };

        match prefix {
            BodyAssociatedPathPrefix::Type(prefix_ty_ref) => {
                let prefix_ty = self.context.type_refs(scope).resolve(&prefix_ty_ref)?;
                self.function_candidates_for_type(scope, &prefix_ty, name, table)
            }
            BodyAssociatedPathPrefix::QualifiedTrait { self_ty, trait_ref } => {
                let Some(selection) = self
                    .context
                    .traits()
                    .qualified_selection(scope, &self_ty, &trait_ref)?
                else {
                    return Ok(UniqueVec::new());
                };
                self.qualified_trait_function_candidates(&selection, name, table)
            }
        }
    }

    /// Collect variant declarations and their resulting enum type.
    fn enum_variant_resolution(
        candidates: impl IntoIterator<Item = BodyAssociatedItemCandidate>,
    ) -> (BodyResolution, Ty) {
        let mut variants = UniqueVec::new();
        let mut tys = ExpectedUnique::new();

        for candidate in candidates {
            let BodyAssociatedItemCandidate::EnumVariant(variant_ref, ty) = candidate else {
                continue;
            };
            variants.push(variant_ref);
            tys.push(ty);
        }

        (
            BodyResolution::Declarations(variants.into_iter().map(DeclarationRef::from).collect()),
            tys.into_ty(),
        )
    }

    /// Collect const declarations and collapse their types.
    fn const_resolution(
        candidates: impl IntoIterator<Item = BodyAssociatedItemCandidate>,
    ) -> (BodyResolution, Ty) {
        let mut consts = UniqueVec::new();
        let mut tys = ExpectedUnique::new();

        for candidate in candidates {
            let BodyAssociatedItemCandidate::Const(const_ref, ty) = candidate else {
                continue;
            };
            consts.push(const_ref);
            tys.push(ty);
        }

        (
            BodyResolution::Declarations(consts.into_iter().map(DeclarationRef::from).collect()),
            tys.into_ty(),
        )
    }

    /// Collect function declarations; call projection owns their result type.
    fn function_resolution(
        candidates: impl IntoIterator<Item = BodyCallableCandidate>,
    ) -> (BodyResolution, Ty) {
        let mut functions = UniqueVec::new();

        for function in candidates {
            functions.push(function.function());
        }

        (
            BodyResolution::Declarations(functions.into_iter().map(DeclarationRef::from).collect()),
            Ty::Unknown,
        )
    }

    /// Find a tuple or unit enum variant that can be used as a bare value.
    fn enum_variant_candidate_for_type(
        &self,
        ty: &AdtTy,
        name: &str,
    ) -> Result<Option<BodyAssociatedItemCandidate>, PackageStoreError> {
        if !matches!(ty.def.id, TypeDefId::Enum(_)) {
            return Ok(None);
        }

        let item_query = self.context.item_query();
        let Some(variant_ref) = item_query.enum_variant_ref_for_type_def(ty.def, name)? else {
            return Ok(None);
        };
        let Some(variant) = item_query.enum_variant_data(variant_ref)? else {
            return Ok(None);
        };
        if !variant.variant.fields.has_value_constructor() {
            return Ok(None);
        }

        Ok(Some(BodyAssociatedItemCandidate::EnumVariant(
            variant_ref,
            Ty::adt(ty.clone()),
        )))
    }

    /// Find the first matching inherent const in current-body then saved-project order.
    fn inherent_associated_const_candidate(
        &self,
        receiver: &BodyReceiverImplMatches,
        name: &str,
    ) -> Result<Option<BodyAssociatedItemCandidate>, PackageStoreError> {
        let item_query = self.context.item_query();
        for impl_match in receiver.matches().inherent() {
            let Some(impl_data) = item_query.impl_data(impl_match.impl_ref())? else {
                continue;
            };
            if let Some(candidate) = self.associated_const_from_items(
                impl_match.impl_ref().origin,
                &impl_data.items,
                receiver.receiver_ty(),
                name,
                Some(impl_match.subst()),
                None,
                None,
            )? {
                return Ok(Some(candidate));
            }
        }
        Ok(None)
    }

    /// Find consts from already-selected trait impls.
    fn trait_associated_const_candidates(
        &self,
        selections: &[TraitSelection],
        receiver_ty: &Ty,
        name: &str,
        extra_subst: Option<&Substitution>,
    ) -> Result<Vec<BodyAssociatedItemCandidate>, PackageStoreError> {
        let mut items = Vec::new();
        self.push_trait_associated_const_candidates(
            &mut items,
            selections,
            receiver_ty,
            name,
            extra_subst,
        )?;
        Ok(items)
    }

    /// Adapt matched functions into static associated-call candidates.
    fn associated_function_candidates_for_matches(
        &self,
        receiver: &BodyReceiverImplMatches,
        name: &str,
        extra_subst: Option<&Substitution>,
    ) -> Result<UniqueVec<BodyCallableCandidate>, PackageStoreError> {
        self.associated_function_candidates(
            receiver.receiver_ty(),
            receiver.matches(),
            Some(receiver),
            name,
            extra_subst,
        )
    }

    /// Adapt function declarations from a matched impl universe into static callables.
    fn associated_function_candidates(
        &self,
        receiver_ty: &Ty,
        matches: &ReceiverImplMatches,
        body_receiver: Option<&BodyReceiverImplMatches>,
        name: &str,
        extra_subst: Option<&Substitution>,
    ) -> Result<UniqueVec<BodyCallableCandidate>, PackageStoreError> {
        let matcher = self.context.impl_matcher();
        let mut functions = UniqueVec::new();
        let item_query = self.context.item_query();
        for function in matcher.function_candidates_for_matches(matches, Some(name))? {
            let Some(function_data) = item_query.function_data(function.function())? else {
                continue;
            };
            if function_data.name != name
                || function_data.has_self_receiver()
                || body_receiver.is_some_and(|receiver| {
                    receiver.saved_inherent_function_is_shadowed(&function, &function_data.name)
                })
            {
                continue;
            }
            let Some(candidate) = BodyCallableCandidate::from_receiver_function(
                &self.context,
                receiver_ty,
                function,
                extra_subst,
            )?
            else {
                continue;
            };
            functions.push(candidate);
        }
        Ok(functions)
    }

    /// Find static functions from the trait impls selected by `<Self as Trait>::item`.
    fn qualified_trait_function_candidates(
        &self,
        selection: &BodyQualifiedTraitSelection,
        name: &str,
        table: &InferenceTable,
    ) -> Result<UniqueVec<BodyCallableCandidate>, PackageStoreError> {
        let matcher = self.context.impl_matcher();
        let mut functions = UniqueVec::new();
        for receiver in selection.receivers() {
            let receiver_ty = Ty::adt(receiver.receiver_ty().clone());
            let matches = matcher.matches_for_receiver_from_impls(
                &receiver_ty,
                UniqueVec::new(),
                receiver.impls().clone(),
                table,
            )?;
            functions.extend(self.associated_function_candidates(
                &receiver_ty,
                &matches,
                None,
                name,
                Some(selection.subst()),
            )?);
        }
        Ok(functions)
    }

    /// Find consts from the trait impls selected by `<Self as Trait>::item`.
    fn qualified_trait_const_candidates(
        &self,
        selection: &BodyQualifiedTraitSelection,
        name: &str,
    ) -> Result<Vec<BodyAssociatedItemCandidate>, PackageStoreError> {
        let mut consts = Vec::new();
        let matcher = self.context.impl_matcher();
        let table = InferenceTable::new();
        for receiver in selection.receivers() {
            let receiver_ty = Ty::adt(receiver.receiver_ty().clone());
            let matches = matcher.matches_for_receiver_from_impls(
                &receiver_ty,
                UniqueVec::new(),
                receiver.impls().clone(),
                &table,
            )?;
            consts.extend(self.trait_associated_const_candidates(
                matches.traits(),
                &receiver_ty,
                name,
                Some(selection.subst()),
            )?);
        }
        Ok(consts)
    }

    /// Add consts from selected impl items, or their trait declarations.
    fn push_trait_associated_const_candidates(
        &self,
        items: &mut Vec<BodyAssociatedItemCandidate>,
        selections: &[TraitSelection],
        receiver_ty: &Ty,
        name: &str,
        extra_subst: Option<&Substitution>,
    ) -> Result<(), PackageStoreError> {
        let item_query = self.context.item_query();
        for selection in selections {
            let trait_impl = selection.trait_impl;
            let Some(impl_data) = item_query.impl_data(trait_impl.impl_ref)? else {
                continue;
            };

            // Impl consts are the concrete declaration for `Type::CONST`. When the impl omits the
            // item, use the trait declaration as a best-effort source for defaulted or incomplete
            // code; const signatures do not preserve whether a default body was written.
            let mut candidate = self.associated_const_from_items(
                trait_impl.impl_ref.origin,
                &impl_data.items,
                receiver_ty,
                name,
                Some(selection.subst.as_substitution()),
                Some(selection),
                extra_subst,
            )?;
            if candidate.is_none()
                && let Some(trait_data) = item_query.trait_data(trait_impl.trait_ref)?
            {
                candidate = self.associated_const_from_items(
                    trait_impl.trait_ref.origin,
                    &trait_data.items,
                    receiver_ty,
                    name,
                    None,
                    Some(selection),
                    extra_subst,
                )?;
            }

            let Some(candidate) = candidate else {
                continue;
            };
            if !items.iter().any(|existing| {
                matches!(
                    (existing, &candidate),
                    (
                        BodyAssociatedItemCandidate::Const(existing, _),
                        BodyAssociatedItemCandidate::Const(candidate, _)
                    ) if existing == candidate
                )
            }) {
                items.push(candidate);
            }
        }

        Ok(())
    }

    /// Find a const item by name and project its receiver type.
    #[allow(clippy::too_many_arguments)]
    fn associated_const_from_items(
        &self,
        origin: DefMapRef,
        assoc_items: &[AssocItemId],
        receiver_ty: &Ty,
        name: &str,
        impl_subst: Option<&Substitution>,
        trait_selection: Option<&TraitSelection>,
        extra_subst: Option<&Substitution>,
    ) -> Result<Option<BodyAssociatedItemCandidate>, PackageStoreError> {
        let item_query = self.context.item_query();
        for item in assoc_items {
            let AssocItemId::Const(id) = item else {
                continue;
            };
            let const_ref = ConstRef { origin, id: *id };
            let Some(const_data) = item_query.const_data(const_ref)? else {
                continue;
            };
            if const_data.name == name {
                return Ok(Some(BodyAssociatedItemCandidate::Const(
                    const_ref,
                    self.semantic_const_ty_for_ty(
                        const_ref,
                        const_data.owner,
                        receiver_ty,
                        impl_subst,
                        trait_selection,
                        extra_subst,
                    )?,
                )));
            }
        }

        Ok(None)
    }

    /// Project an associated const signature through any canonical receiver shape.
    fn semantic_const_ty_for_ty(
        &self,
        const_ref: ConstRef,
        owner: ItemOwner,
        receiver_ty: &Ty,
        impl_subst: Option<&Substitution>,
        trait_selection: Option<&TraitSelection>,
        extra_subst: Option<&Substitution>,
    ) -> Result<Ty, PackageStoreError> {
        let mut subst = self.context.generics().subst_for_selected_item_owner(
            const_ref.origin,
            owner,
            receiver_ty,
            impl_subst,
        )?;
        if let Some(selection) = trait_selection {
            // A concrete trait impl selects both impl-owned parameters and trait-owned arguments.
            // `impl Convert<u16> for u8` therefore projects a trait declaration mentioning `T`
            // to `u16`, while an impl-side const can still refer to its own generic parameters.
            subst.extend(selection.subst.as_substitution().clone());
            subst.extend(
                self.context
                    .generics()
                    .subst_for_trait_application(selection.application())?,
            );
        }
        if let Some(extra_subst) = extra_subst {
            subst.extend(extra_subst.clone());
        }
        let ty = self
            .context
            .signatures()
            .const_ty(const_ref)?
            .unwrap_or(Ty::Unknown);
        let ty = subst.apply(&ty);
        let Some(selection) = trait_selection else {
            return Ok(ty);
        };
        Ok(selection.table.finalize_without_numeric_defaults(&ty))
    }
}
