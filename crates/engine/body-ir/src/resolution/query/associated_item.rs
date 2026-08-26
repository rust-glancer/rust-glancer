//! Associated item lookup in value position.

use rg_def_map::DefMapSource;
use rg_ir_model::{
    AssocItemId, ConstRef, DefMapRef, EnumVariantRef, FunctionRef, ImplRef, ItemOwner, Path,
    ScopeId, TraitApplicability, TraitImplRef, TypeDefId, identity::DeclarationRef,
};
use rg_item_tree::TypeRef;
use rg_package_store::PackageStoreError;
use rg_semantic_ir::ItemStoreSource;
use rg_std::{ExpectedUnique, UniqueVec};
use rg_ty::{AdtTy, ExpectedTyExt, Substitution, TraitSelection, Ty, inference::InferenceTable};
use rg_ty::{
    AssociatedItemCandidateRef, AssociatedItemQuery, AssociatedItemRef, TraitApplication,
    TypeLoweringAnchor, TypeLoweringEnv, TypeLoweringQuery,
};

use super::traits::BodyQualifiedTraitSelection;

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

/// Associated function selected through a concrete `Type::function` prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BodyAssociatedFunctionCandidate {
    function: FunctionRef,
    self_ty: Ty,
    subst: Substitution,
    trait_selection: Option<TraitSelection>,
}

impl BodyAssociatedFunctionCandidate {
    /// Return the selected associated function.
    pub(crate) fn function(&self) -> FunctionRef {
        self.function
    }

    /// Return the `Self` type used to select the function.
    pub(crate) fn self_ty(&self) -> &Ty {
        &self.self_ty
    }

    /// Return substitutions derived from the selected `Self` type.
    pub(crate) fn subst(&self) -> &Substitution {
        &self.subst
    }

    /// Return the trait-selection evidence for this function, if it came from a trait impl.
    pub(crate) fn trait_selection(&self) -> Option<&TraitSelection> {
        self.trait_selection.as_ref()
    }
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

                let mut candidates = self.candidates_for_ty(&prefix_ty)?;
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
        ty: &Ty,
    ) -> Result<Vec<AssociatedItemCandidateRef>, PackageStoreError> {
        let query = AssociatedItemQuery::with_resolver(self.context.ty_context(), &self.context);
        let item_query = self.context.item_query();
        let mut candidates = Vec::new();
        for receiver_ty in ty.as_adts() {
            let receiver_ty = self
                .context
                .generics()
                .complete_omitted_nominal_args(receiver_ty)?;
            let has_crate_index = receiver_ty.def.origin.as_crate_ref().is_some();
            let body_items = self.context.body_local_items();
            let body_inherent_names = body_items.inherent_item_names_for_type(receiver_ty.def)?;

            candidates.extend(query.candidates_for_nominal_from_impls(
                &receiver_ty,
                body_items.inherent_impls_for_type(receiver_ty.def)?,
                body_items.trait_impls_for_type(receiver_ty.def)?,
                !has_crate_index,
            )?);

            // Body-origin types have no crate-level index, so the local query above also supplies
            // their enum variants. Crate-origin types get variants from this indexed universe;
            // the local query contributes only impls declared inside the body.
            if has_crate_index {
                for candidate in query.candidates_for_nominal(&receiver_ty)? {
                    let shadows_body_item = match candidate.item() {
                        AssociatedItemRef::Function(function) => {
                            item_query.function_data(function)?.is_some_and(|data| {
                                body_inherent_names.contains_function(data.name.as_str())
                            })
                        }
                        AssociatedItemRef::Const(konst) => {
                            item_query.const_data(konst)?.is_some_and(|data| {
                                body_inherent_names.contains_const(data.name.as_str())
                            })
                        }
                        AssociatedItemRef::TypeAlias(alias) => {
                            item_query.type_alias_data(alias)?.is_some_and(|data| {
                                body_inherent_names.contains_type_alias(data.name.as_str())
                            })
                        }
                        AssociatedItemRef::EnumVariant(_) => false,
                    };
                    if !shadows_body_item {
                        candidates.push(candidate);
                    }
                }
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
        let prefix_ty =
            Ty::from_type_path_resolution(prefix_resolution, Vec::new()).unwrap_or(Ty::Unknown);
        self.resolve_for_type(&prefix_ty, last_segment)
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
                self.resolve_for_type(&prefix_ty, last_segment)
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
        prefix_ty: &Ty,
        last_segment: &str,
    ) -> Result<Option<(BodyResolution, Ty)>, PackageStoreError> {
        let receiver_tys = self.receiver_tys_for_prefix(prefix_ty)?;

        // First treat the final segment as an enum variant. Variants are not ordinary associated
        // functions in either Semantic IR or Body IR, but value paths use the same syntax for
        // `Action::Start` and `Widget::new`, so they need an explicit pass.
        let mut variants = Vec::new();
        for nominal_ty in &receiver_tys {
            if let Some(candidate) =
                self.enum_variant_candidate_for_type(nominal_ty, last_segment)?
            {
                variants.push(candidate);
            }
        }

        if !variants.is_empty() {
            return Ok(Some(Self::enum_variant_resolution(variants)));
        }

        for nominal_ty in &receiver_tys {
            if let Some(candidate) =
                self.inherent_associated_const_candidate_for_type(nominal_ty, last_segment)?
            {
                return Ok(Some(Self::const_resolution([candidate])));
            }
        }

        // Trait associated const lookup is receiver-driven: `Type::CONST` submits each matching
        // impl to the shared selection boundary, then reads the concrete const from that impl or
        // falls back to the trait declaration. An unavailable proof remains a tentative lookup
        // candidate for editor features; definite rejection removes it.
        let mut trait_consts = Vec::new();
        for nominal_ty in &receiver_tys {
            trait_consts
                .extend(self.trait_associated_const_candidates_for_type(nominal_ty, last_segment)?);
        }

        if !trait_consts.is_empty() {
            return Ok(Some(Self::const_resolution(trait_consts)));
        }

        // Inherent associated functions are exact candidates. Trait-associated functions retain
        // tentative matches when incomplete generic information cannot prove or reject the impl.
        let mut functions = UniqueVec::new();
        let table = InferenceTable::new();
        for nominal_ty in &receiver_tys {
            functions.extend(self.associated_function_candidates_for_type(
                nominal_ty,
                last_segment,
                &table,
            )?);
        }

        Ok((!functions.is_empty()).then_some(Self::function_resolution(functions)))
    }

    /// Return associated functions selected by a typed prefix.
    pub(crate) fn function_candidates_for_type(
        &self,
        prefix_ty: &Ty,
        name: &str,
        table: &InferenceTable,
    ) -> Result<UniqueVec<BodyAssociatedFunctionCandidate>, PackageStoreError> {
        let mut functions = UniqueVec::new();
        for nominal_ty in self.receiver_tys_for_prefix(prefix_ty)? {
            functions.extend(self.associated_function_candidates_for_type(
                &nominal_ty,
                name,
                table,
            )?);
        }
        Ok(functions)
    }

    /// Return associated function candidates selected by a rich body path.
    pub(crate) fn function_candidates_for_body_path(
        &self,
        scope: ScopeId,
        path: &BodyPath,
        table: &InferenceTable,
    ) -> Result<UniqueVec<BodyAssociatedFunctionCandidate>, PackageStoreError> {
        let Some((prefix, name)) = path.split_associated_item_prefix_name() else {
            return Ok(UniqueVec::new());
        };

        match prefix {
            BodyAssociatedPathPrefix::Type(prefix_ty_ref) => {
                let prefix_ty = self.context.type_refs(scope).resolve(&prefix_ty_ref)?;
                self.function_candidates_for_type(&prefix_ty, name, table)
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
        candidates: impl IntoIterator<Item = BodyAssociatedFunctionCandidate>,
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

    /// Find an inherent associated const in body-local then crate impls.
    fn inherent_associated_const_candidate_for_type(
        &self,
        ty: &AdtTy,
        name: &str,
    ) -> Result<Option<BodyAssociatedItemCandidate>, PackageStoreError> {
        if let Some(item) = self.associated_const_candidate_for_impls(
            self.context
                .body_local_items()
                .inherent_impls_for_type(ty.def)?,
            ty,
            name,
        )? {
            return Ok(Some(item));
        }

        if ty.def.origin == DefMapRef::Body(self.context.body_ref()) {
            return Ok(None);
        }

        self.associated_const_candidate_for_impls(
            self.context
                .item_lookup_query()
                .inherent_impls_for_type(ty.def),
            ty,
            name,
        )
    }

    /// Find associated consts from applicable trait impls.
    fn trait_associated_const_candidates_for_type(
        &self,
        ty: &AdtTy,
        name: &str,
    ) -> Result<Vec<BodyAssociatedItemCandidate>, PackageStoreError> {
        let mut items = Vec::new();

        self.push_trait_associated_const_candidates_for_impls(
            &mut items,
            self.context
                .body_local_items()
                .trait_impls_for_type(ty.def)?,
            ty,
            name,
        )?;

        if ty.def.origin == DefMapRef::Body(self.context.body_ref()) {
            return Ok(items);
        }

        let semantic_trait_impls = self
            .context
            .item_lookup_query()
            .trait_impls_for_type(ty.def);
        self.push_trait_associated_const_candidates_for_impls(
            &mut items,
            semantic_trait_impls,
            ty,
            name,
        )?;

        Ok(items)
    }

    /// Find static associated functions from inherent and trait impls.
    fn associated_function_candidates_for_type(
        &self,
        ty: &AdtTy,
        name: &str,
        table: &InferenceTable,
    ) -> Result<UniqueVec<BodyAssociatedFunctionCandidate>, PackageStoreError> {
        let body_items = self.context.body_local_items();
        let matcher = self.context.impl_matcher();
        let mut functions = UniqueVec::new();

        for function_ref in body_items.inherent_functions_for_type(ty.def)? {
            if matcher.function_applies_to_receiver(function_ref, ty)? {
                self.push_associated_function(&mut functions, ty, function_ref, name)?;
            }
        }

        if ty.def.origin.as_crate_ref().is_some() {
            let body_inherent_names = body_items.inherent_item_names_for_type(ty.def)?;
            if !body_inherent_names.contains_function(name) {
                for function_ref in self.semantic_inherent_fn_defs_for_type(ty, name)? {
                    if matcher.function_applies_to_receiver(function_ref, ty)? {
                        self.push_associated_function(&mut functions, ty, function_ref, name)?;
                    }
                }
            }
        }

        let body_trait_impls = body_items.trait_impls_for_type(ty.def)?;
        for (function_ref, selection) in
            matcher.trait_function_candidates_from_impls(body_trait_impls, ty, Some(name), table)?
        {
            self.push_associated_function_with_subst(
                &mut functions,
                ty,
                function_ref,
                name,
                None,
                Some(selection),
            )?;
        }

        if ty.def.origin.as_crate_ref().is_some() {
            for (function_ref, selection) in
                matcher.trait_function_candidates_for_receiver(ty, Some(name), table)?
            {
                self.push_associated_function_with_subst(
                    &mut functions,
                    ty,
                    function_ref,
                    name,
                    None,
                    Some(selection),
                )?;
            }
        }

        Ok(functions)
    }

    /// Find static functions from the trait impls selected by `<Self as Trait>::item`.
    fn qualified_trait_function_candidates(
        &self,
        selection: &BodyQualifiedTraitSelection,
        name: &str,
        table: &InferenceTable,
    ) -> Result<UniqueVec<BodyAssociatedFunctionCandidate>, PackageStoreError> {
        let mut functions = UniqueVec::new();
        for receiver in selection.receivers() {
            for (function_ref, trait_selection) in self
                .context
                .impl_matcher()
                .trait_function_candidates_from_impls(
                    receiver.impls().clone(),
                    receiver.receiver_ty(),
                    Some(name),
                    table,
                )?
            {
                self.push_associated_function_with_subst(
                    &mut functions,
                    receiver.receiver_ty(),
                    function_ref,
                    name,
                    Some(selection.subst()),
                    Some(trait_selection),
                )?;
            }
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
        for receiver in selection.receivers() {
            self.push_trait_associated_const_candidates_for_impls(
                &mut consts,
                receiver.impls().clone(),
                receiver.receiver_ty(),
                name,
            )?;
        }
        Ok(consts)
    }

    /// Read crate-visible inherent functions from the semantic lookup query.
    fn semantic_inherent_fn_defs_for_type(
        &self,
        ty: &AdtTy,
        name: &str,
    ) -> Result<UniqueVec<FunctionRef>, PackageStoreError> {
        Ok(self
            .context
            .item_lookup_query()
            .inherent_functions_for_type_and_name(ty.def, name))
    }

    /// Add a function only if it is static and has the requested name.
    fn push_associated_function(
        &self,
        functions: &mut UniqueVec<BodyAssociatedFunctionCandidate>,
        receiver_ty: &AdtTy,
        function_ref: FunctionRef,
        name: &str,
    ) -> Result<(), PackageStoreError> {
        self.push_associated_function_with_subst(
            functions,
            receiver_ty,
            function_ref,
            name,
            None,
            None,
        )
    }

    /// Add a function with extra substitutions from an explicit trait qualification.
    fn push_associated_function_with_subst(
        &self,
        functions: &mut UniqueVec<BodyAssociatedFunctionCandidate>,
        receiver_ty: &AdtTy,
        function_ref: FunctionRef,
        name: &str,
        extra_subst: Option<&Substitution>,
        trait_selection: Option<TraitSelection>,
    ) -> Result<(), PackageStoreError> {
        let Some(function_data) = self.context.item_query().function_data(function_ref)? else {
            return Ok(());
        };
        if function_data.name == name && !function_data.has_self_receiver() {
            let mut subst = self.context.generics().subst_for_receiver_owner(
                function_ref.origin,
                function_data.owner,
                receiver_ty,
            )?;
            if let Some(extra_subst) = extra_subst {
                subst.extend(extra_subst.clone());
            }
            let candidate = BodyAssociatedFunctionCandidate {
                function: function_ref,
                self_ty: Ty::adt(receiver_ty.clone()),
                subst,
                trait_selection,
            };
            functions.push(candidate);
        }
        Ok(())
    }

    /// Preserve written args and treat omitted type args as inferable unknowns.
    fn receiver_tys_for_prefix(&self, prefix_ty: &Ty) -> Result<Vec<AdtTy>, PackageStoreError> {
        prefix_ty
            .as_adts()
            .iter()
            .map(|ty| self.context.generics().complete_omitted_nominal_args(ty))
            .collect()
    }

    /// Add consts from applicable impl items, or their trait declarations.
    fn push_trait_associated_const_candidates_for_impls(
        &self,
        items: &mut Vec<BodyAssociatedItemCandidate>,
        trait_impls: UniqueVec<TraitImplRef>,
        ty: &AdtTy,
        name: &str,
    ) -> Result<(), PackageStoreError> {
        let item_query = self.context.item_query();
        let matcher = self.context.impl_matcher();
        let table = InferenceTable::new();
        for trait_impl in trait_impls {
            if !matcher
                .trait_impl_applicability(trait_impl, ty, &table)?
                .is_applicable()
            {
                continue;
            }

            let Some(impl_data) = item_query.impl_data(trait_impl.impl_ref)? else {
                continue;
            };

            // Impl consts are the concrete declaration for `Type::CONST`. When the impl omits the
            // item, use the trait declaration as a best-effort source for defaulted or incomplete
            // code; const signatures do not preserve whether a default body was written.
            let mut candidate = self.associated_const_from_items(
                trait_impl.impl_ref.origin,
                &impl_data.items,
                ty,
                name,
            )?;
            if candidate.is_none()
                && let Some(trait_data) = item_query.trait_data(trait_impl.trait_ref)?
            {
                candidate = self.associated_const_from_items(
                    trait_impl.trait_ref.origin,
                    &trait_data.items,
                    ty,
                    name,
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

    /// Find the first matching associated const across inherent impls.
    fn associated_const_candidate_for_impls(
        &self,
        impls: UniqueVec<ImplRef>,
        ty: &AdtTy,
        name: &str,
    ) -> Result<Option<BodyAssociatedItemCandidate>, PackageStoreError> {
        let item_query = self.context.item_query();
        for impl_ref in impls {
            let Some(impl_data) = item_query.impl_data(impl_ref)? else {
                continue;
            };
            if !self
                .context
                .impl_matcher()
                .impl_applies_to_receiver(impl_ref, impl_data, ty)?
            {
                continue;
            }

            if let Some(item) =
                self.associated_const_from_items(impl_ref.origin, &impl_data.items, ty, name)?
            {
                return Ok(Some(item));
            }
        }

        Ok(None)
    }

    /// Find a const item by name and project its receiver type.
    fn associated_const_from_items(
        &self,
        origin: DefMapRef,
        assoc_items: &[AssocItemId],
        receiver_ty: &AdtTy,
        name: &str,
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
                    self.semantic_const_ty_for_receiver(const_ref, const_data.owner, receiver_ty)?,
                )));
            }
        }

        Ok(None)
    }

    /// Resolve an associated const type for a concrete receiver.
    fn semantic_const_ty_for_receiver(
        &self,
        const_ref: ConstRef,
        owner: ItemOwner,
        receiver_ty: &AdtTy,
    ) -> Result<Ty, PackageStoreError> {
        let subst = self.context.generics().subst_for_receiver_owner(
            const_ref.origin,
            owner,
            receiver_ty,
        )?;
        let ty = self
            .context
            .signatures()
            .const_ty(const_ref)?
            .unwrap_or(Ty::Unknown);
        Ok(subst.apply(&ty))
    }
}
