//! Resolve type paths, `Self`, and transparent aliases in the active lowering context.

use rg_def_map::DefMapSource;
use rg_ir_model::{GenericDefRef, GenericParamRef, ItemOwner, TypeAliasRef};
use rg_item_tree::{GenericArg as ItemGenericArg, TypePath, TypePathAnchor, TypeRef};
use rg_semantic_ir::{GenericParamSource, ItemStoreSource, SelfTypeOwner, TypePathResolution};

use super::{ImplTraitMode, TypeLoweringAnchor, TypeLoweringSession, TypePathResolver};
use crate::{AdtTy, AliasTy, PrimitiveTy, ProjectionTy, Substitution, TraitApplication, Ty};

impl<'lower, 'query, D, I, R> TypeLoweringSession<'lower, 'query, D, I, R>
where
    D: DefMapSource,
    I: ItemStoreSource<'query, Error = D::Error>,
    R: TypePathResolver<Error = D::Error>,
{
    pub(crate) fn lower_type_path(
        &mut self,
        path: &TypePath,
        impl_trait_mode: ImplTraitMode,
        inference: Option<&dyn Fn() -> Ty>,
    ) -> Result<Ty, D::Error> {
        if path.anchor.is_some() {
            return self.lower_anchored_type_path(path, impl_trait_mode, inference);
        }

        // Trait declarations normally spell their own projections as `Self::Item`, not as the
        // fully-qualified `<Self as Trait>::Item`. Lower the prefix first so the trait-owned
        // `Self` parameter supplies both the associated-item identity and the full trait args.
        if path.segments.len() > 1 {
            let prefix = TypePath {
                source_span: path.source_span,
                absolute: path.absolute,
                anchor: None,
                segments: path.segments[..path.segments.len() - 1].to_vec(),
            };
            let prefix_param = prefix
                .single_name()
                .map(|name| self.param_by_name(name.as_str()))
                .transpose()?
                .flatten();
            let prefix_ty =
                self.lower_type_ref_with_mode(&TypeRef::Path(prefix), impl_trait_mode, inference)?;
            if let Some(GenericParamRef::Type(param)) = prefix_param
                && let Some(name) = path.segments.last().map(|segment| &segment.name)
            {
                let param_source = self
                    .query
                    .item_paths
                    .generics()
                    .generics(param.owner)?
                    .iter()
                    .find(|candidate| candidate.param() == GenericParamRef::Type(param))
                    .map(|candidate| candidate.source());
                if matches!(param_source, Some(GenericParamSource::TraitSelf))
                    && let GenericDefRef::Trait(trait_ref) = param.owner
                {
                    let generics = self
                        .query
                        .item_paths
                        .generics()
                        .generics(GenericDefRef::Trait(trait_ref))?;
                    let application = TraitApplication {
                        def: trait_ref,
                        args: self.subst.args_for(&generics),
                    };
                    if let Some(projection) = self.associated_type_projection(&application, name)? {
                        return Ok(Ty::Alias(AliasTy::Projection(projection)));
                    }
                }

                // A generic projection such as `I::Item` gets its trait identity from the bounds
                // visible at this owner. Keep only a unique semantic candidate: two traits with an
                // `Item` alias are genuinely ambiguous without fully-qualified syntax.
                if let Some(projection) =
                    self.param_associated_projection(param, prefix_ty, name)?
                {
                    return Ok(Ty::Alias(AliasTy::Projection(projection)));
                }
            }
        }

        if let Some(name) = path.single_name()
            && let Some(param) = self.param_by_name(name.as_str())?
            && let GenericParamRef::Type(param) = param
        {
            return Ok(self
                .subst
                .type_param(param)
                .cloned()
                .unwrap_or(Ty::Param(param)));
        }

        // `Self` keeps the owner's generic arguments, including when the type has defaults.
        // Impl receivers can also be primitive or structural, so recover the complete owner
        // type before falling back to definition-path lookup.
        if path
            .single_name()
            .is_some_and(|name| name.as_str() == "Self")
            && let Some(self_ty) = self.lower_self()?
        {
            return Ok(self_ty);
        }

        let Some(path_key) = path.as_def_map_path() else {
            return Ok(Ty::Unknown);
        };
        let resolution = self
            .query
            .resolver
            .resolve_type_path(self.anchor, &path_key)?;
        let syntax_args = path
            .segments
            .last()
            .map(|segment| segment.args.as_slice())
            .unwrap_or_default();

        match resolution {
            TypePathResolution::SelfType(def) => {
                if let Some(self_ty) = self.lower_self()? {
                    return Ok(self_ty);
                }
                let generics = self
                    .query
                    .item_paths
                    .generics()
                    .generics(GenericDefRef::TypeDef(def))?;
                let args = self.lower_generic_args(
                    &generics,
                    syntax_args,
                    &Substitution::new(),
                    impl_trait_mode,
                    inference,
                )?;
                Ok(Ty::adt(AdtTy { def, args }))
            }
            TypePathResolution::TypeDef(def) => {
                let generics = self
                    .query
                    .item_paths
                    .generics()
                    .generics(GenericDefRef::TypeDef(def))?;
                let args = self.lower_generic_args(
                    &generics,
                    syntax_args,
                    &Substitution::new(),
                    impl_trait_mode,
                    inference,
                )?;
                Ok(Ty::adt(AdtTy { def, args }))
            }
            TypePathResolution::TypeAlias(alias) => {
                self.lower_alias_with_mode(alias, syntax_args, impl_trait_mode, inference)
            }
            TypePathResolution::Trait(_) => Ok(Ty::Unknown),
            TypePathResolution::Unknown => Ok(path
                .single_name()
                .and_then(|name| PrimitiveTy::from_name(name.as_str()))
                .map(Ty::Primitive)
                .unwrap_or(Ty::Unknown)),
        }
    }

    /// Lower `Self` through its owner while retaining that owner's generic arguments.
    ///
    /// In `struct Wrapper<T = u32>`, `Self` means `Wrapper<T>`, including before `T` is known.
    /// An impl supplies its full receiver spelling, as in `impl<T> Wrapper<Vec<T>>`.
    fn lower_self(&mut self) -> Result<Option<Ty>, D::Error> {
        let TypeLoweringAnchor::Context(context) = self.anchor else {
            return Ok(None);
        };
        let impl_ref = match context.self_owner {
            Some(SelfTypeOwner::TypeDef(def)) => {
                let generics = self
                    .query
                    .item_paths
                    .generics()
                    .generics(GenericDefRef::TypeDef(def))?;
                return Ok(Some(Ty::adt(AdtTy {
                    def,
                    args: self.subst.args_for(&generics),
                })));
            }
            Some(SelfTypeOwner::Impl(impl_ref)) => impl_ref,
            // Trait `Self` is a generic parameter and is lowered before owner-type lookup.
            Some(SelfTypeOwner::Trait(_)) | None => return Ok(None),
        };
        let Some(data) = self.query.item_paths.items().impl_data(impl_ref)? else {
            return Ok(None);
        };

        let previous_owner = self.owner;
        self.owner = GenericDefRef::Impl(impl_ref);
        let ty = self.lower_type_ref(&data.self_ty);
        self.owner = previous_owner;
        ty.map(Some)
    }

    fn lower_anchored_type_path(
        &mut self,
        path: &TypePath,
        impl_trait_mode: ImplTraitMode,
        inference: Option<&dyn Fn() -> Ty>,
    ) -> Result<Ty, D::Error> {
        let Some(anchor) = &path.anchor else {
            return Ok(Ty::Unknown);
        };
        let Some(name) = path.segments.last().map(|segment| &segment.name) else {
            return Ok(Ty::Unknown);
        };

        let projection = match anchor {
            TypePathAnchor::Type(self_ty_ref) => {
                let param = match self_ty_ref.as_ref() {
                    TypeRef::Path(path) => path
                        .single_name()
                        .map(|name| self.param_by_name(name.as_str()))
                        .transpose()?
                        .flatten(),
                    _ => None,
                };
                let self_ty =
                    self.lower_type_ref_with_mode(self_ty_ref, impl_trait_mode, inference)?;
                let Some(GenericParamRef::Type(param)) = param else {
                    return Ok(Ty::Unknown);
                };
                self.param_associated_projection(param, self_ty, name)?
            }
            TypePathAnchor::QualifiedTrait { self_ty, trait_ty } => {
                let self_ty = self.lower_type_ref_with_mode(self_ty, impl_trait_mode, inference)?;
                let Some(trait_ref) =
                    self.lower_trait_ref_with_mode(trait_ty, self_ty, impl_trait_mode, inference)?
                else {
                    return Ok(Ty::Unknown);
                };
                self.associated_type_projection(&trait_ref.application, name)?
            }
        };
        let Some(projection) = projection else {
            return Ok(Ty::Unknown);
        };

        Ok(Ty::Alias(AliasTy::Projection(projection)))
    }

    pub(crate) fn lower_alias(
        &mut self,
        alias: TypeAliasRef,
        syntax_args: &[ItemGenericArg],
    ) -> Result<Ty, D::Error> {
        self.lower_alias_with_mode(alias, syntax_args, ImplTraitMode::Opaque, None)
    }

    fn lower_alias_with_mode(
        &mut self,
        alias: TypeAliasRef,
        syntax_args: &[ItemGenericArg],
        impl_trait_mode: ImplTraitMode,
        inference: Option<&dyn Fn() -> Ty>,
    ) -> Result<Ty, D::Error> {
        if self.alias_stack.contains(&alias) {
            return Ok(Ty::Unknown);
        }
        let Some(data) = self.query.item_paths.items().type_alias_data(alias)? else {
            return Ok(Ty::Unknown);
        };
        let alias_owner = GenericDefRef::TypeAlias(alias);
        let generics = self.query.item_paths.generics().generics(alias_owner)?;

        // Associated aliases inherit their trait/impl parameters. Those identities already occur
        // in the active substitution, while written args belong only to the alias's own section.
        let mut parent_seed = Substitution::new();
        let inherited_len = if alias_owner == self.owner {
            generics.len()
        } else {
            generics.parent_len()
        };
        for param in generics.iter().take(inherited_len) {
            if let Some(arg) = self.subst.get(param.param()) {
                parent_seed.push(param.param(), arg.clone());
            }
        }
        let args = self.lower_generic_args(
            &generics,
            syntax_args,
            &parent_seed,
            impl_trait_mode,
            inference,
        )?;

        let Some(aliased_ty) = data.signature.aliased_ty() else {
            if matches!(data.owner, ItemOwner::Trait(_)) {
                return Ok(Ty::Alias(AliasTy::Projection(ProjectionTy {
                    associated_ty: alias,
                    args,
                })));
            }
            return Ok(Ty::Unknown);
        };
        let Some(context) = self
            .query
            .item_paths
            .items()
            .type_path_context_for_owner(alias.origin, data.owner)?
        else {
            return Ok(Ty::Unknown);
        };

        let previous_owner = self.owner;
        let previous_anchor = self.anchor;
        let previous_subst =
            std::mem::replace(&mut self.subst, Substitution::from_args(&generics, &args));
        self.owner = alias_owner;
        self.anchor = TypeLoweringAnchor::Context(context);
        self.alias_stack.push(alias);
        let result = self.lower_type_ref(aliased_ty);
        self.alias_stack.pop();
        self.owner = previous_owner;
        self.anchor = previous_anchor;
        self.subst = previous_subst;
        result
    }
}
