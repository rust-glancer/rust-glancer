//! Trait lookup that depends on the current body's names and source overlay.
//!
//! This query handles two related Rust lookup forms:
//!
//! - implicit lookup, such as `value.render()`, asks which traits are lexically in scope;
//! - a qualified path, such as `<Widget as Render<Color>>::Output`, resolves the written trait
//!   application and keeps only impls whose receiver and trait arguments match it.
//!
//! The stable scope and declaration-surface cache lives in the sibling `trait_cache` module. This
//! file owns the semantic collection and qualified-path matching that fill or consume those facts.

use std::{collections::HashSet, sync::Arc};

use rg_def_map::DefMapSource;
use rg_ir_model::{
    DefMapRef, GenericDefRef, LocalDefRef, ModuleId, ModuleRef, ScopeId, SemanticItemRef,
    TraitDefRef, TraitImplRef,
};
use rg_item_tree::TypeRef;
use rg_package_store::PackageStoreError;
use rg_semantic_ir::ItemStoreSource;
use rg_std::UniqueVec;
use rg_ty::{AdtTy, GenericArg, Substitution, TraitApplication, Ty};

use crate::resolution::BodyResolutionContext;

/// Resolves lexical trait scope and qualified trait prefixes in body context.
pub(crate) struct BodyTraitQuery<'query, D, I> {
    context: BodyResolutionContext<'query, D, I>,
}

/// A written `<Self as Trait<Args>>` prefix after matching it to concrete receiver impls.
///
/// For `<Vec<u8> as Convert<u16>>::Output`, `subst` retains the written trait arguments and each
/// receiver entry retains the completed `Vec<u8>` plus impls whose trait application agrees with
/// `Convert<u16>`. Consumers can then resolve the associated item without repeating prefix lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BodyQualifiedTraitSelection {
    subst: Substitution,
    receivers: Vec<BodyQualifiedTraitReceiverSelection>,
}

/// One nominal interpretation of `Self` and the qualified impls that matched it.
///
/// A type path can conservatively resolve to more than one ADT while source is incomplete, so the
/// outer selection keeps a list of these receiver-specific groups instead of pretending the path
/// was unique.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BodyQualifiedTraitReceiverSelection {
    receiver_ty: AdtTy,
    impls: UniqueVec<TraitImplRef>,
}

/// Lowered trait application and substitutions taken directly from the written prefix.
///
/// This is the intermediate `Render<Color>` part of `<Widget as Render<Color>>`; receiver-specific
/// impl filtering is deliberately performed after the prefix itself has resolved.
struct ResolvedTraitPrefix {
    application: TraitApplication,
    subst: Substitution,
}

impl BodyQualifiedTraitSelection {
    /// Return substitutions from the written trait prefix, such as `T = User`.
    pub(crate) fn subst(&self) -> &Substitution {
        &self.subst
    }

    /// Return receiver types and impls selected by the qualified trait prefix.
    pub(crate) fn receivers(&self) -> &[BodyQualifiedTraitReceiverSelection] {
        &self.receivers
    }
}

impl BodyQualifiedTraitReceiverSelection {
    /// Return the `Self` type from `<Self as Trait>`.
    pub(crate) fn receiver_ty(&self) -> &AdtTy {
        &self.receiver_ty
    }

    /// Return impls matching the written trait path and receiver.
    pub(crate) fn impls(&self) -> &UniqueVec<TraitImplRef> {
        &self.impls
    }
}

impl<'query, D, I> BodyTraitQuery<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    pub(crate) fn new(context: BodyResolutionContext<'query, D, I>) -> Self {
        Self { context }
    }

    /// Return the traits Rust makes available for implicit associated-item lookup at this scope.
    ///
    /// ```text
    /// use api::Render as Painting;
    /// use api::Inspect as _;
    ///
    /// value.render();  // `Render`, under the local path name `Painting`
    /// value.inspect(); // unnamed import, available only to method lookup
    /// ```
    ///
    /// Each lexical layer contributes traits independently. This differs from path lookup: an
    /// inner `struct Paint;` can hide the path spelling without removing an outer imported trait
    /// from method lookup. The unnamed lane is independent because `as _` occupies no path name.
    pub(crate) fn traits_in_scope(
        &self,
        scope: ScopeId,
    ) -> Result<Arc<HashSet<TraitDefRef>>, PackageStoreError> {
        self.context
            .trait_cache()
            .scope_or_try_init(scope, || self.collect_traits_in_scope(scope))
    }

    fn collect_traits_in_scope(
        &self,
        scope: ScopeId,
    ) -> Result<HashSet<TraitDefRef>, PackageStoreError> {
        let def_maps = self.context.def_map_query();
        let body_scope = ModuleRef {
            origin: DefMapRef::Body(self.context.body_ref()),
            module: ModuleId(scope.0),
        };
        let mut traits = HashSet::new();

        self.push_local_traits(&mut traits, def_maps.traits_in_lexical_scope(body_scope)?)?;

        // Saved source overlays can resolve first through a body-owned module and then through
        // the original crate module. Both modules may contribute trait candidates even when they
        // use the same path spelling, matching the accumulation across ordinary lexical layers.
        let owner_module = self.context.body().owner_module();
        self.push_local_traits(
            &mut traits,
            def_maps.traits_in_unqualified_scope(owner_module)?,
        )?;

        let fallback_module = self.context.body().fallback_module();
        if fallback_module != owner_module {
            self.push_local_traits(
                &mut traits,
                def_maps.traits_in_unqualified_scope(fallback_module)?,
            )?;
        }

        Ok(traits)
    }

    fn push_local_traits(
        &self,
        traits: &mut HashSet<TraitDefRef>,
        local_defs: impl IntoIterator<Item = LocalDefRef>,
    ) -> Result<(), PackageStoreError> {
        for local_def in local_defs {
            self.push_local_trait(traits, local_def)?;
        }
        Ok(())
    }

    fn push_local_trait(
        &self,
        traits: &mut HashSet<TraitDefRef>,
        local_def: LocalDefRef,
    ) -> Result<(), PackageStoreError> {
        if let Some(SemanticItemRef::Trait(trait_ref)) = self
            .context
            .item_query()
            .semantic_item_for_local_def(local_def)?
        {
            traits.insert(trait_ref);
        }
        Ok(())
    }

    /// Resolve `<Self as Trait<Args>>` into receiver-specific trait impls.
    pub(crate) fn qualified_selection(
        &self,
        scope: ScopeId,
        self_ty_ref: &TypeRef,
        trait_ty_ref: &TypeRef,
    ) -> Result<Option<BodyQualifiedTraitSelection>, PackageStoreError> {
        let self_ty = self.resolve_type_ref(scope, self_ty_ref)?;
        let Some(trait_prefix) = self.resolve_trait_prefix(scope, trait_ty_ref, self_ty.clone())?
        else {
            return Ok(None);
        };

        let mut receivers = Vec::new();
        for receiver_ty in self_ty.as_adts() {
            let receiver_ty = self
                .context
                .generics()
                .complete_omitted_nominal_args(receiver_ty)?;
            let impls =
                self.qualified_trait_impls_for_type(&receiver_ty, &trait_prefix.application)?;
            if !impls.is_empty() {
                receivers.push(BodyQualifiedTraitReceiverSelection { receiver_ty, impls });
            }
        }

        Ok(
            (!receivers.is_empty()).then_some(BodyQualifiedTraitSelection {
                subst: trait_prefix.subst,
                receivers,
            }),
        )
    }

    /// Resolve a type syntax where the qualified path is written.
    fn resolve_type_ref(&self, scope: ScopeId, ty: &TypeRef) -> Result<Ty, PackageStoreError> {
        self.context.type_refs(scope).resolve(ty)
    }

    /// Resolve `Trait<Args>` from `<Self as Trait<Args>>`.
    fn resolve_trait_prefix(
        &self,
        scope: ScopeId,
        trait_ty_ref: &TypeRef,
        self_ty: Ty,
    ) -> Result<Option<ResolvedTraitPrefix>, PackageStoreError> {
        let Some(lowering) = self
            .context
            .type_refs(scope)
            .resolve_trait_ref(trait_ty_ref, self_ty)?
        else {
            return Ok(None);
        };
        let generics = self
            .context
            .item_paths()
            .generics()
            .generics(GenericDefRef::Trait(lowering.application.def))?;
        Ok(Some(ResolvedTraitPrefix {
            subst: Substitution::from_args(&generics, &lowering.application.args),
            application: lowering.application,
        }))
    }

    /// Keep impls whose trait definition and concrete trait args match the written prefix.
    fn qualified_trait_impls_for_type(
        &self,
        ty: &AdtTy,
        application: &TraitApplication,
    ) -> Result<UniqueVec<TraitImplRef>, PackageStoreError> {
        let mut impls = UniqueVec::new();
        self.push_matching_qualified_trait_impls(
            &mut impls,
            self.context
                .body_local_items()
                .trait_impls_for_type(ty.def)?
                .iter()
                .copied(),
            ty,
            application,
        )?;

        if ty.def.origin.as_crate_ref().is_some() {
            let semantic_impls = self
                .context
                .item_lookup_query()
                .trait_impls_for_type(ty.def);
            self.push_matching_qualified_trait_impls(&mut impls, semantic_impls, ty, application)?;
        }

        Ok(impls)
    }

    fn push_matching_qualified_trait_impls(
        &self,
        impls: &mut UniqueVec<TraitImplRef>,
        candidates: impl IntoIterator<Item = TraitImplRef>,
        ty: &AdtTy,
        application: &TraitApplication,
    ) -> Result<(), PackageStoreError> {
        for candidate in candidates {
            if candidate.trait_ref != application.def {
                continue;
            }
            if self.trait_impl_args_match_written_args(candidate, ty, application)? {
                impls.push(candidate);
            }
        }
        Ok(())
    }

    /// Compare concrete args in the impl header with `<Self as Trait<Args>>`.
    ///
    /// This is a syntax-driven filter: it resolves impl-header args after applying receiver subst,
    /// then compares them to the args written in the qualified path.
    fn trait_impl_args_match_written_args(
        &self,
        trait_impl: TraitImplRef,
        receiver_ty: &AdtTy,
        written: &TraitApplication,
    ) -> Result<bool, PackageStoreError> {
        let matcher = self.context.impl_matcher();
        let Some(header) = matcher.impl_header(trait_impl.impl_ref)? else {
            return Ok(false);
        };
        let Some((impl_subst, _applicability)) =
            matcher.impl_self_subst_for_impl(trait_impl.impl_ref, &Ty::adt(receiver_ty.clone()))?
        else {
            return Ok(false);
        };
        let Some(impl_trait) = header.trait_ref else {
            return Ok(false);
        };
        let impl_application = impl_subst.apply_trait_application(&impl_trait.application);
        Ok(impl_application.def == written.def
            && Self::generic_args_match(&written.args, &impl_application.args))
    }

    /// Treat unknown args as compatible; incomplete code should not create false negatives.
    fn generic_args_match(written_args: &[GenericArg], impl_args: &[GenericArg]) -> bool {
        written_args.len() == impl_args.len()
            && written_args
                .iter()
                .zip(impl_args)
                .all(|(written_arg, impl_arg)| {
                    written_arg == impl_arg || written_arg.has_unknown() || impl_arg.has_unknown()
                })
    }
}
