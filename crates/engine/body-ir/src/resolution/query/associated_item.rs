//! Associated item lookup in value position.

use rg_ir_model::{
    AssocItemId, BodyAssociatedPathPrefix, BodyPath, ConstRef, DefMapRef, EnumVariantRef,
    FunctionRef, ImplRef, ItemOwner, Path, ScopeId, TraitImplRef, TypeDefId,
    identity::DeclarationRef,
};
use rg_ir_storage::{DefMapSource, ItemStoreSource, TypePathContext};
use rg_package_store::PackageStoreError;
use rg_std::{ExpectedUnique, UniqueVec};
use rg_ty::{ExpectedTyExt, GenericArg, NominalTy, Ty, TypeSubst};

use super::traits::BodyQualifiedTraitSelection;

use crate::{
    ir::resolved::BodyResolution,
    resolution::{BodyResolutionContext, TypeRefUseSite},
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
    subst: TypeSubst,
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
    pub(crate) fn subst(&self) -> &TypeSubst {
        &self.subst
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
                let prefix_ty = self
                    .context
                    .type_refs(TypeRefUseSite::Scope(scope))
                    .resolve(&prefix_ty_ref)?;
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
                let functions =
                    self.qualified_trait_function_candidates(&selection, last_segment)?;
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

        // Trait associated const lookup is still receiver-driven: `Type::CONST` first proves that
        // `Type` has a visible trait impl, then reads the matching const item from that impl or
        // falls back to the trait declaration. This mirrors trait method lookup without pretending
        // to be a full trait solver.
        let mut trait_consts = Vec::new();
        for nominal_ty in &receiver_tys {
            trait_consts
                .extend(self.trait_associated_const_candidates_for_type(nominal_ty, last_segment)?);
        }

        if !trait_consts.is_empty() {
            return Ok(Some(Self::const_resolution(trait_consts)));
        }

        // Inherent associated functions are exact candidates. Trait-associated functions are kept
        // deliberately optimistic, following the same "prefer useful candidates over false
        // negatives" policy as dot completion.
        let mut functions = Vec::new();
        for nominal_ty in &receiver_tys {
            functions
                .extend(self.associated_function_candidates_for_type(nominal_ty, last_segment)?);
        }

        Ok((!functions.is_empty()).then_some(Self::function_resolution(functions)))
    }

    /// Return associated functions selected by a typed prefix.
    pub(crate) fn function_candidates_for_type(
        &self,
        prefix_ty: &Ty,
        name: &str,
    ) -> Result<Vec<BodyAssociatedFunctionCandidate>, PackageStoreError> {
        let mut functions = Vec::new();
        for nominal_ty in self.receiver_tys_for_prefix(prefix_ty)? {
            functions.extend(self.associated_function_candidates_for_type(&nominal_ty, name)?);
        }
        Ok(functions)
    }

    /// Return associated function candidates selected by a rich body path.
    pub(crate) fn function_candidates_for_body_path(
        &self,
        scope: ScopeId,
        path: &BodyPath,
    ) -> Result<Vec<BodyAssociatedFunctionCandidate>, PackageStoreError> {
        let Some((prefix, name)) = path.split_associated_item_prefix_name() else {
            return Ok(Vec::new());
        };

        match prefix {
            BodyAssociatedPathPrefix::Type(prefix_ty_ref) => {
                let prefix_ty = self
                    .context
                    .type_refs(TypeRefUseSite::Scope(scope))
                    .resolve(&prefix_ty_ref)?;
                self.function_candidates_for_type(&prefix_ty, name)
            }
            BodyAssociatedPathPrefix::QualifiedTrait { self_ty, trait_ref } => {
                let Some(selection) = self
                    .context
                    .traits()
                    .qualified_selection(scope, &self_ty, &trait_ref)?
                else {
                    return Ok(Vec::new());
                };
                self.qualified_trait_function_candidates(&selection, name)
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

    /// Find an enum variant constructor for an enum receiver type.
    fn enum_variant_candidate_for_type(
        &self,
        ty: &NominalTy,
        name: &str,
    ) -> Result<Option<BodyAssociatedItemCandidate>, PackageStoreError> {
        if !matches!(ty.def.id, TypeDefId::Enum(_)) {
            return Ok(None);
        }

        Ok(self
            .context
            .item_query()
            .enum_variant_ref_for_type_def(ty.def, name)?
            .map(|variant_ref| {
                BodyAssociatedItemCandidate::EnumVariant(variant_ref, Ty::nominal(ty.clone()))
            }))
    }

    /// Find an inherent associated const in body-local then target impls.
    fn inherent_associated_const_candidate_for_type(
        &self,
        ty: &NominalTy,
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
                .target_items()
                .inherent_impls_for_type(ty.def)?,
            ty,
            name,
        )
    }

    /// Find associated consts from applicable trait impls.
    fn trait_associated_const_candidates_for_type(
        &self,
        ty: &NominalTy,
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

        let target_items = self.context.target_items();
        let semantic_trait_impls = match self.context.semantic_index() {
            Some(index) => index
                .trait_impls_for_type(ty.def)
                .cloned()
                .unwrap_or_default(),
            None => target_items.trait_impls_for_type(ty.def)?,
        };
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
        ty: &NominalTy,
        name: &str,
    ) -> Result<Vec<BodyAssociatedFunctionCandidate>, PackageStoreError> {
        let body_items = self.context.body_local_items();
        let matcher = self.context.impl_matcher();
        let mut functions = Vec::new();

        for function_ref in body_items.inherent_functions_for_type(ty.def)? {
            if matcher.function_applies_to_receiver(function_ref, ty)? {
                self.push_associated_function(&mut functions, ty, function_ref, name)?;
            }
        }

        if ty.def.origin.as_target_ref().is_some() {
            for function_ref in self.semantic_inherent_function_items_for_type(ty, name)? {
                if matcher.function_applies_to_receiver(function_ref, ty)? {
                    self.push_associated_function(&mut functions, ty, function_ref, name)?;
                }
            }
        }

        let body_trait_impls = body_items.trait_impls_for_type(ty.def)?;
        for (function_ref, _) in matcher.trait_function_candidates_from_impls(
            self.context.semantic_index(),
            body_trait_impls,
            ty,
            Some(name),
        )? {
            self.push_associated_function(&mut functions, ty, function_ref, name)?;
        }

        if ty.def.origin.as_target_ref().is_some() {
            for (function_ref, _) in matcher.trait_function_candidates_for_receiver(
                self.context.semantic_index(),
                ty,
                Some(name),
            )? {
                self.push_associated_function(&mut functions, ty, function_ref, name)?;
            }
        }

        Ok(functions)
    }

    /// Find static functions from the trait impls selected by `<Self as Trait>::item`.
    fn qualified_trait_function_candidates(
        &self,
        selection: &BodyQualifiedTraitSelection,
        name: &str,
    ) -> Result<Vec<BodyAssociatedFunctionCandidate>, PackageStoreError> {
        let mut functions = Vec::new();
        for receiver in selection.receivers() {
            for (function_ref, _) in self
                .context
                .impl_matcher()
                .trait_function_candidates_from_impls(
                    self.context.semantic_index(),
                    receiver.impls().clone(),
                    receiver.receiver_ty(),
                    Some(name),
                )?
            {
                self.push_associated_function_with_subst(
                    &mut functions,
                    receiver.receiver_ty(),
                    function_ref,
                    name,
                    Some(selection.subst()),
                )?;
            }
        }
        Ok(functions)
    }

    /// Read target-visible inherent functions, using the index when available.
    fn semantic_inherent_function_items_for_type(
        &self,
        ty: &NominalTy,
        name: &str,
    ) -> Result<UniqueVec<FunctionRef>, PackageStoreError> {
        match self.context.semantic_index() {
            Some(index) => Ok(index
                .inherent_functions_for_type_and_name(ty.def, name)
                .cloned()
                .unwrap_or_default()),
            None => self
                .context
                .target_items()
                .inherent_functions_for_type(ty.def),
        }
    }

    /// Add a function only if it is static and has the requested name.
    fn push_associated_function(
        &self,
        functions: &mut Vec<BodyAssociatedFunctionCandidate>,
        receiver_ty: &NominalTy,
        function_ref: FunctionRef,
        name: &str,
    ) -> Result<(), PackageStoreError> {
        self.push_associated_function_with_subst(functions, receiver_ty, function_ref, name, None)
    }

    /// Add a function with extra substitutions from an explicit trait qualification.
    fn push_associated_function_with_subst(
        &self,
        functions: &mut Vec<BodyAssociatedFunctionCandidate>,
        receiver_ty: &NominalTy,
        function_ref: FunctionRef,
        name: &str,
        extra_subst: Option<&TypeSubst>,
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
                self_ty: Ty::nominal(receiver_ty.clone()),
                subst,
            };
            if !functions.contains(&candidate) {
                functions.push(candidate);
            }
        }
        Ok(())
    }

    /// Preserve written args and treat omitted type args as inferable unknowns.
    fn receiver_tys_for_prefix(&self, prefix_ty: &Ty) -> Result<Vec<NominalTy>, PackageStoreError> {
        prefix_ty
            .as_nominals()
            .iter()
            .map(|ty| self.receiver_ty_for_prefix(ty))
            .collect()
    }

    fn receiver_ty_for_prefix(&self, ty: &NominalTy) -> Result<NominalTy, PackageStoreError> {
        if !ty.args.is_empty() {
            return Ok(ty.clone());
        }
        let Some(generics) = self
            .context
            .item_query()
            .generic_params_for_type_def(ty.def)?
        else {
            return Ok(ty.clone());
        };
        if generics.types.is_empty() {
            return Ok(ty.clone());
        }

        Ok(NominalTy {
            def: ty.def,
            args: generics
                .types
                .iter()
                .map(|_| GenericArg::Type(Box::new(Ty::Unknown)))
                .collect(),
        })
    }

    /// Add consts from applicable impl items, or their trait declarations.
    fn push_trait_associated_const_candidates_for_impls(
        &self,
        items: &mut Vec<BodyAssociatedItemCandidate>,
        trait_impls: UniqueVec<TraitImplRef>,
        ty: &NominalTy,
        name: &str,
    ) -> Result<(), PackageStoreError> {
        let item_query = self.context.item_query();
        let matcher = self.context.impl_matcher();
        for trait_impl in trait_impls {
            if !matcher
                .trait_impl_applicability(trait_impl, ty)?
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
        ty: &NominalTy,
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
        receiver_ty: &NominalTy,
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
        receiver_ty: &NominalTy,
    ) -> Result<Ty, PackageStoreError> {
        let item_query = self.context.item_query();
        let Some(const_data) = item_query.const_data(const_ref)? else {
            return Ok(Ty::Unknown);
        };
        let Some(ty) = const_data.signature.ty() else {
            return Ok(Ty::Unknown);
        };

        if ty.is_self_type() {
            return Ok(Ty::nominal(receiver_ty.clone()));
        }

        let subst = self.context.generics().subst_for_receiver_owner(
            const_ref.origin,
            owner,
            receiver_ty,
        )?;

        let context = self
            .context
            .item_query()
            .type_path_context_for_owner(const_ref.origin, owner)?
            .unwrap_or_else(|| TypePathContext::module(self.context.body().owner_module()));
        self.context
            .type_refs(TypeRefUseSite::OwnerContext(context))
            .with_subst(&subst)
            .resolve(ty)
    }
}
