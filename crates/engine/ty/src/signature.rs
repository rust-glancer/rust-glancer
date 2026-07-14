//! Request-scoped semantic signatures built by the shared type lowerer.
//!
//! These queries are the handoff from source-shaped declaration data to the type engine. A whole
//! declaration is lowered in one session, so its parameters, `impl Trait` occurrences, clauses,
//! and aliases use the same identities. Downstream algorithms read these results instead of
//! walking `TypeRef` again.

use rg_def_map::DefMapSource;
use rg_ir_model::items::{ParamKind, SelfParamKind};
use rg_ir_model::{
    ConstRef, EnumVariantRef, FieldRef, FunctionRef, GenericDefRef, GenericParamRef, ImplRef,
    ItemOwner, StaticRef, TraitDefRef, TypeAliasRef,
};
use rg_semantic_ir::{GenericParamSource, ItemStoreSource, TypePathContext};

use crate::{
    Clause, ItemPathQuery, OpaqueTy, Substitution, TraitRefLowering, Ty, TypeLoweringAnchor,
    TypeLoweringEnv, TypeLoweringQuery, TypePathResolver,
};

/// One function's parameters, return, and predicates under an owner-scoped binder.
///
/// Types and clauses retain their owner-scoped parameter refs. A caller chooses whether to keep
/// those identities or replace them with a call-specific substitution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallableSignature {
    pub params: Vec<Ty>,
    pub ret: Ty,
    pub clauses: Vec<Clause>,
}

/// Canonical impl self type, optional trait application, and predicates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImplHeader {
    pub owner: ImplRef,
    pub self_ty: Ty,
    pub trait_ref: Option<TraitRefLowering>,
    pub clauses: Vec<Clause>,
}

/// Trait `Self` and the predicates exposed to the trait solver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TraitHeader {
    pub owner: TraitDefRef,
    pub self_ty: Ty,
    pub clauses: Vec<Clause>,
}

/// Semantic declaration queries. Results borrow no syntax and are safe to pass between type
/// algorithms within the request that owns the underlying package transaction.
pub struct SemanticSignatureQuery<'query, D, I, R = ItemPathQuery<'query, D, I>> {
    item_paths: ItemPathQuery<'query, D, I>,
    resolver: R,
}

impl<'query, D, I> SemanticSignatureQuery<'query, D, I>
where
    D: DefMapSource + Clone,
    I: ItemStoreSource<'query, Error = D::Error>,
{
    pub fn new(def_maps: D, items: I) -> Self {
        Self {
            item_paths: ItemPathQuery::new(def_maps.clone(), items.clone()),
            resolver: ItemPathQuery::new(def_maps, items),
        }
    }
}

impl<'query, D, I, R> SemanticSignatureQuery<'query, D, I, R>
where
    D: DefMapSource,
    I: ItemStoreSource<'query, Error = D::Error>,
    R: TypePathResolver<Error = D::Error>,
{
    /// Build signature queries with the path semantics of the requesting layer.
    ///
    /// Body IR supplies lexical lookup here for body-local declarations. Ordinary item queries
    /// use `new`, whose resolver is definition-level lookup over the same semantic stores.
    pub fn with_resolver(def_maps: D, items: I, resolver: R) -> Self {
        Self {
            item_paths: ItemPathQuery::new(def_maps, items),
            resolver,
        }
    }

    /// Lower one complete function declaration into the shared semantic type vocabulary.
    ///
    /// Parameters are visited in source order before the return type. Keeping that walk in one
    /// session gives argument-position parameters and opaque return occurrences repeatable
    /// owner-local identities.
    pub fn function(&self, function: FunctionRef) -> Result<Option<CallableSignature>, D::Error> {
        let Some(data) = self.item_paths.items().function_data(function)? else {
            return Ok(None);
        };
        let Some(context) = self
            .item_paths
            .items()
            .type_path_context_for_function(function)?
        else {
            return Ok(None);
        };
        let owner = GenericDefRef::Function(function);
        let implicit_self_ty = self.self_param_ty(function, data.owner)?;
        let lowering = TypeLoweringQuery::new(&self.item_paths, &self.resolver);
        let mut session = lowering.session(TypeLoweringEnv::new(
            owner,
            TypeLoweringAnchor::Context(context),
        ))?;

        // One session walks parameters in source order so each APIT occurrence receives the same
        // owner-local ID in every query.
        let mut params = Vec::with_capacity(data.signature.params().len());
        for param in data.signature.params() {
            let ty = match &param.ty {
                Some(ty) => session.lower_parameter_type(ty)?,
                None => match param.kind {
                    ParamKind::SelfParam(SelfParamKind::Value) => implicit_self_ty.clone(),
                    ParamKind::SelfParam(SelfParamKind::Reference { mutability }) => {
                        Ty::reference(mutability, implicit_self_ty.clone())
                    }
                    ParamKind::SelfParam(SelfParamKind::Explicit) | ParamKind::Normal => {
                        Ty::Unknown
                    }
                },
            };
            params.push(ty);
        }
        let ret = data
            .signature
            .ret_ty()
            .map(|ty| session.lower_type_ref(ty))
            .transpose()?
            .unwrap_or(Ty::Unit);
        let clauses = session.lower_clauses()?;

        Ok(Some(CallableSignature {
            params,
            ret,
            clauses,
        }))
    }

    pub fn function_ty(&self, function: FunctionRef) -> Result<Option<Ty>, D::Error> {
        if self.item_paths.items().function_data(function)?.is_none() {
            return Ok(None);
        }
        let generics = self
            .item_paths
            .generics()
            .generics(GenericDefRef::Function(function))?;
        let args = Substitution::identity(&generics).args_for(&generics);
        Ok(Some(Ty::fn_def_with_args(function, args)))
    }

    /// Return the trait bounds carried by one function-owned type parameter.
    ///
    /// This is especially useful for argument-position `impl Trait`: the semantic type is a
    /// function-owned parameter, while its `impl Trait` spelling comes from these declaration
    /// predicates rather than from type identity.
    pub fn function_type_param_bounds(
        &self,
        param: rg_ir_model::TypeParamRef,
    ) -> Result<Vec<TraitRefLowering>, D::Error> {
        let GenericDefRef::Function(function) = param.owner else {
            return Ok(Vec::new());
        };
        let Some(signature) = self.function(function)? else {
            return Ok(Vec::new());
        };
        let subject = Ty::Param(param);
        let mut bounds = Vec::new();
        for clause in &signature.clauses {
            let Clause::Implemented(application) = clause else {
                continue;
            };
            if application.self_ty() != Some(&subject) {
                continue;
            }
            let associated_types = signature
                .clauses
                .iter()
                .filter_map(|clause| {
                    let Clause::AliasEq { alias, ty } = clause else {
                        return None;
                    };
                    (alias.args == application.args).then(|| crate::AssocTypeBinding {
                        associated_ty: alias.associated_ty,
                        ty: ty.clone(),
                    })
                })
                .collect();
            bounds.push(TraitRefLowering {
                application: application.clone(),
                associated_types,
            });
        }
        Ok(bounds)
    }

    pub fn field_ty(&self, field: FieldRef) -> Result<Option<Ty>, D::Error> {
        let Some(data) = self.item_paths.items().field_data(field)? else {
            return Ok(None);
        };
        let owner = GenericDefRef::TypeDef(field.owner);
        let lowering = TypeLoweringQuery::new(&self.item_paths, &self.resolver);
        lowering
            .lower(
                &data.field.ty,
                TypeLoweringEnv::new(
                    owner,
                    TypeLoweringAnchor::Context(TypePathContext::module(data.owner_module)),
                ),
            )
            .map(Some)
    }

    pub fn enum_variant_field_ty(
        &self,
        variant: EnumVariantRef,
        field_index: usize,
    ) -> Result<Option<Ty>, D::Error> {
        let Some(data) = self.item_paths.items().enum_variant_data(variant)? else {
            return Ok(None);
        };
        let Some(field) = data.variant.fields.fields().get(field_index) else {
            return Ok(None);
        };
        let owner = GenericDefRef::TypeDef(data.owner);
        let lowering = TypeLoweringQuery::new(&self.item_paths, &self.resolver);
        lowering
            .lower(
                &field.ty,
                TypeLoweringEnv::new(
                    owner,
                    TypeLoweringAnchor::Context(TypePathContext::module(data.owner_module)),
                ),
            )
            .map(Some)
    }

    pub fn impl_header(&self, impl_ref: ImplRef) -> Result<Option<ImplHeader>, D::Error> {
        impl_header_with(&self.item_paths, &self.resolver, impl_ref)
    }

    pub fn type_alias_ty(&self, alias: TypeAliasRef) -> Result<Option<Ty>, D::Error> {
        type_alias_ty_with(&self.item_paths, &self.resolver, alias)
    }

    /// Returns the predicates declared by one opaque occurrence.
    ///
    /// Bounds are queried declaration data, not part of opaque type equality. Replaying the
    /// owner's canonical lowering session keeps occurrence IDs and nested alias traversal aligned
    /// with the type that introduced the opaque identity.
    pub fn opaque_bounds(
        &self,
        opaque: &OpaqueTy,
    ) -> Result<Option<Vec<TraitRefLowering>>, D::Error> {
        let Some(bounds) =
            opaque_bounds_for_owner_with(&self.item_paths, &self.resolver, opaque.opaque.owner)?
                .into_iter()
                .find_map(|(candidate, bounds)| {
                    (candidate.opaque == opaque.opaque).then_some(bounds)
                })
        else {
            return Ok(None);
        };
        let generics = self.item_paths.generics().generics(opaque.opaque.owner)?;
        let subst = Substitution::from_args(&generics, &opaque.args);
        Ok(Some(
            bounds
                .iter()
                .map(|bound| subst.apply_trait_ref(bound))
                .collect(),
        ))
    }

    pub fn const_ty(&self, konst: ConstRef) -> Result<Option<Ty>, D::Error> {
        let Some(data) = self.item_paths.items().const_data(konst)? else {
            return Ok(None);
        };
        let Some(ty) = data.signature.ty() else {
            return Ok(Some(Ty::Unknown));
        };
        let Some(context) = self
            .item_paths
            .items()
            .type_path_context_for_owner(konst.origin, data.owner)?
        else {
            return Ok(None);
        };
        let lowering = TypeLoweringQuery::new(&self.item_paths, &self.resolver);
        lowering
            .lower(
                ty,
                TypeLoweringEnv::new(
                    GenericDefRef::Const(konst),
                    TypeLoweringAnchor::Context(context),
                ),
            )
            .map(Some)
    }

    pub fn static_ty(&self, static_ref: StaticRef) -> Result<Option<Ty>, D::Error> {
        let Some(data) = self.item_paths.items().static_data(static_ref)? else {
            return Ok(None);
        };
        let Some(ty) = &data.ty else {
            return Ok(Some(Ty::Unknown));
        };
        let lowering = TypeLoweringQuery::new(&self.item_paths, &self.resolver);
        lowering
            .lower(
                ty,
                TypeLoweringEnv::new(
                    GenericDefRef::Static(static_ref),
                    TypeLoweringAnchor::Context(TypePathContext::module(data.owner)),
                ),
            )
            .map(Some)
    }

    fn self_param_ty(&self, function: FunctionRef, item_owner: ItemOwner) -> Result<Ty, D::Error> {
        match item_owner {
            ItemOwner::Impl(id) => Ok(self
                .impl_header(ImplRef {
                    origin: function.origin,
                    id,
                })?
                .map(|header| header.self_ty)
                .unwrap_or(Ty::Unknown)),
            ItemOwner::Trait(_) => {
                let generics = self
                    .item_paths
                    .generics()
                    .generics(GenericDefRef::Function(function))?;
                Ok(generics
                    .param_by_name("Self")
                    .and_then(|param| match param {
                        GenericParamRef::Type(param) => Some(Ty::Param(param)),
                        GenericParamRef::Lifetime(_) | GenericParamRef::Const(_) => None,
                    })
                    .unwrap_or(Ty::Unknown))
            }
            ItemOwner::Module(_) => Ok(Ty::Unknown),
        }
    }
}

impl<'query, D, I> SemanticSignatureQuery<'query, D, I>
where
    D: DefMapSource,
    I: ItemStoreSource<'query, Error = D::Error>,
{
    pub(crate) fn trait_header_from(
        item_paths: &ItemPathQuery<'query, D, I>,
        trait_ref: TraitDefRef,
    ) -> Result<Option<TraitHeader>, D::Error> {
        trait_header_with(item_paths, item_paths, trait_ref)
    }

    pub(crate) fn type_alias_ty_from(
        item_paths: &ItemPathQuery<'query, D, I>,
        alias: TypeAliasRef,
    ) -> Result<Option<Ty>, D::Error> {
        type_alias_ty_with(item_paths, item_paths, alias)
    }

    pub(crate) fn opaque_bounds_for_owner_from(
        item_paths: &ItemPathQuery<'query, D, I>,
        owner: GenericDefRef,
    ) -> Result<Vec<(OpaqueTy, Vec<TraitRefLowering>)>, D::Error> {
        opaque_bounds_for_owner_with(item_paths, item_paths, owner)
    }
}

pub(crate) fn impl_header_with<'query, D, I, R>(
    item_paths: &ItemPathQuery<'query, D, I>,
    resolver: &R,
    impl_ref: ImplRef,
) -> Result<Option<ImplHeader>, D::Error>
where
    D: DefMapSource,
    I: ItemStoreSource<'query, Error = D::Error>,
    R: TypePathResolver<Error = D::Error>,
{
    let Some(data) = item_paths.items().impl_data(impl_ref)? else {
        return Ok(None);
    };
    let owner = GenericDefRef::Impl(impl_ref);
    let context = TypePathContext {
        module: data.owner,
        impl_ref: Some(impl_ref),
    };
    let lowering = TypeLoweringQuery::new(item_paths, resolver);
    let mut session = lowering.session(TypeLoweringEnv::new(
        owner,
        TypeLoweringAnchor::Context(context),
    ))?;
    let self_ty = session.lower_type_ref(&data.self_ty)?;
    let trait_ref = data
        .trait_ref
        .as_ref()
        .map(|trait_ty| session.lower_trait_ref(trait_ty, self_ty.clone()))
        .transpose()?
        .flatten();
    let clauses = session.lower_clauses()?;

    Ok(Some(ImplHeader {
        owner: impl_ref,
        self_ty,
        trait_ref,
        clauses,
    }))
}

fn trait_header_with<'query, D, I, R>(
    item_paths: &ItemPathQuery<'query, D, I>,
    resolver: &R,
    trait_ref: TraitDefRef,
) -> Result<Option<TraitHeader>, D::Error>
where
    D: DefMapSource,
    I: ItemStoreSource<'query, Error = D::Error>,
    R: TypePathResolver<Error = D::Error>,
{
    let Some(data) = item_paths.items().trait_data(trait_ref)? else {
        return Ok(None);
    };
    let owner = GenericDefRef::Trait(trait_ref);
    let generics = item_paths.generics().generics(owner)?;
    let Some(self_param) = generics.iter().find_map(|param| {
        matches!(param.source(), GenericParamSource::TraitSelf).then_some(param.param())
    }) else {
        return Ok(None);
    };
    let GenericParamRef::Type(self_param) = self_param else {
        return Ok(None);
    };
    let self_ty = Ty::Param(self_param);
    let lowering = TypeLoweringQuery::new(item_paths, resolver);
    let mut session = lowering.session(TypeLoweringEnv::new(
        owner,
        TypeLoweringAnchor::Context(TypePathContext::module(data.owner)),
    ))?;
    let mut super_traits = Vec::new();
    for bound in &data.super_traits {
        let rg_ir_model::items::TypeBound::Trait(trait_ty) = bound else {
            continue;
        };
        if let Some(super_trait) = session.lower_trait_ref(trait_ty, self_ty.clone())? {
            super_traits.push(super_trait);
        }
    }
    let mut clauses = session.lower_clauses()?;
    for super_trait in &super_traits {
        clauses.extend(super_trait.clone().into_clauses());
    }

    Ok(Some(TraitHeader {
        owner: trait_ref,
        self_ty,
        clauses,
    }))
}

fn type_alias_ty_with<'query, D, I, R>(
    item_paths: &ItemPathQuery<'query, D, I>,
    resolver: &R,
    alias: TypeAliasRef,
) -> Result<Option<Ty>, D::Error>
where
    D: DefMapSource,
    I: ItemStoreSource<'query, Error = D::Error>,
    R: TypePathResolver<Error = D::Error>,
{
    let Some(data) = item_paths.items().type_alias_data(alias)? else {
        return Ok(None);
    };
    let Some(context) = item_paths
        .items()
        .type_path_context_for_owner(alias.origin, data.owner)?
    else {
        return Ok(None);
    };
    let lowering = TypeLoweringQuery::new(item_paths, resolver);
    let mut session = lowering.session(TypeLoweringEnv::new(
        GenericDefRef::TypeAlias(alias),
        TypeLoweringAnchor::Context(context),
    ))?;
    session.lower_alias(alias, &[]).map(Some)
}

fn opaque_bounds_for_owner_with<'query, D, I, R>(
    item_paths: &ItemPathQuery<'query, D, I>,
    resolver: &R,
    owner: GenericDefRef,
) -> Result<Vec<(OpaqueTy, Vec<TraitRefLowering>)>, D::Error>
where
    D: DefMapSource,
    I: ItemStoreSource<'query, Error = D::Error>,
    R: TypePathResolver<Error = D::Error>,
{
    let Some(context) = item_paths
        .items()
        .type_path_context_for_generic_def(owner)?
    else {
        return Ok(Vec::new());
    };
    let lowering = TypeLoweringQuery::new(item_paths, resolver);
    let mut session = lowering.session(TypeLoweringEnv::new(
        owner,
        TypeLoweringAnchor::Context(context),
    ))?;

    match owner {
        GenericDefRef::Function(function) => {
            let Some(data) = item_paths.items().function_data(function)? else {
                return Ok(Vec::new());
            };
            for param in data.signature.params() {
                if let Some(ty) = &param.ty {
                    session.lower_parameter_type(ty)?;
                }
            }
            if let Some(ret) = data.signature.ret_ty() {
                session.lower_type_ref(ret)?;
            }
        }
        GenericDefRef::TypeAlias(alias) => {
            session.lower_alias(alias, &[])?;
        }
        GenericDefRef::Const(konst) => {
            if let Some(ty) = item_paths
                .items()
                .const_data(konst)?
                .and_then(|data| data.signature.ty())
            {
                session.lower_type_ref(ty)?;
            }
        }
        GenericDefRef::Static(static_ref) => {
            if let Some(ty) = item_paths
                .items()
                .static_data(static_ref)?
                .and_then(|data| data.ty.as_ref())
            {
                session.lower_type_ref(ty)?;
            }
        }
        GenericDefRef::TypeDef(_) | GenericDefRef::Trait(_) | GenericDefRef::Impl(_) => {}
    }

    Ok(session.into_opaque_bounds())
}
