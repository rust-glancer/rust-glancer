//! The single source-type to semantic-type lowering boundary.
//!
//! Definition HIR intentionally keeps `TypeRef`. This module is the only place that interprets
//! that syntax as semantic identity. Callers may customize path lookup for a body scope, but they
//! do not get a second recursive visitor or their own rules for parameters, aliases, and args.

use rg_def_map::DefMapSource;
use rg_ir_model::{
    GenericDefRef, GenericParamRef, ItemOwner, OpaqueTyId, OpaqueTyRef, Path, ScopeId, TraitDefRef,
    TypeAliasRef, TypeParamRef,
};
use rg_item_tree::{
    GenericArg as ItemGenericArg, TypeBound, TypePath, TypePathAnchor, TypeRef, WherePredicate,
};
use rg_semantic_ir::{GenericParamSource, ItemStoreSource, TypePathContext, TypePathResolution};
use rg_std::{ExpectedUnique, UniqueVec};

use crate::inference::InferenceTable;
use crate::{
    AdtTy, AliasTy, AssocTypeBinding, Clause, ConstValue, GenericArg, GenericArgs, ItemPathQuery,
    Lifetime, OpaqueTy, PrimitiveTy, ProjectionTy, Substitution, TraitApplication,
    TraitRefLowering, Ty,
};

/// Name-lookup starting point for paths encountered during type lowering.
///
/// A path written in a body may resolve through its lexical `Scope`, including body-local items.
/// A path in an item signature instead uses the declaration's module and optional impl `Context`.
/// Only this lookup policy varies; both cases continue through the same recursive type visitor.
#[derive(Debug, Clone, Copy)]
pub enum TypeLoweringAnchor {
    Scope(ScopeId),
    Context(TypePathContext),
}

/// The only site-specific operation accepted by semantic type lowering.
///
/// Body IR implements lexical lookup for `Scope`; ordinary item signatures use the definition
/// resolver for `Context`. All projection after this identity lookup remains in this module.
pub trait TypePathResolver {
    type Error;

    fn resolve_type_path(
        &self,
        anchor: TypeLoweringAnchor,
        path: &Path,
    ) -> Result<TypePathResolution, Self::Error>;
}

impl<R> TypePathResolver for &R
where
    R: TypePathResolver + ?Sized,
{
    type Error = R::Error;

    fn resolve_type_path(
        &self,
        anchor: TypeLoweringAnchor,
        path: &Path,
    ) -> Result<TypePathResolution, Self::Error> {
        R::resolve_type_path(*self, anchor, path)
    }
}

impl<'query, D, I> TypePathResolver for ItemPathQuery<'query, D, I>
where
    D: DefMapSource,
    I: ItemStoreSource<'query, Error = D::Error>,
{
    type Error = D::Error;

    fn resolve_type_path(
        &self,
        anchor: TypeLoweringAnchor,
        path: &Path,
    ) -> Result<TypePathResolution, Self::Error> {
        match anchor {
            TypeLoweringAnchor::Scope(_) => Ok(TypePathResolution::Unknown),
            TypeLoweringAnchor::Context(context) => {
                ItemPathQuery::resolve_type_path(self, context, path)
            }
        }
    }
}

/// Facts that give source syntax a meaning when one lowering session starts.
///
/// `owner` supplies identities for generic parameters and opaque occurrences. `anchor` decides
/// where names are looked up.
#[derive(Debug, Clone)]
pub struct TypeLoweringEnv {
    owner: GenericDefRef,
    anchor: TypeLoweringAnchor,
}

impl TypeLoweringEnv {
    pub fn new(owner: GenericDefRef, anchor: TypeLoweringAnchor) -> Self {
        Self { owner, anchor }
    }
}

/// Shared inputs for request-scoped lowering sessions.
///
/// The query itself is stateless. Callers create a [`TypeLoweringSession`] when several source
/// types belong to one signature and must share occurrence numbering and cycle tracking.
pub struct TypeLoweringQuery<'lower, 'query, D, I, R> {
    item_paths: &'lower ItemPathQuery<'query, D, I>,
    resolver: &'lower R,
}

impl<'lower, 'query, D, I, R> TypeLoweringQuery<'lower, 'query, D, I, R>
where
    D: DefMapSource,
    I: ItemStoreSource<'query, Error = D::Error>,
    R: TypePathResolver<Error = D::Error>,
{
    pub fn new(item_paths: &'lower ItemPathQuery<'query, D, I>, resolver: &'lower R) -> Self {
        Self {
            item_paths,
            resolver,
        }
    }

    pub fn session(
        &'lower self,
        env: TypeLoweringEnv,
    ) -> Result<TypeLoweringSession<'lower, 'query, D, I, R>, D::Error> {
        let generics = self.item_paths.generics().generics(env.owner)?;
        let subst = Substitution::identity(&generics);

        Ok(TypeLoweringSession {
            query: self,
            owner: env.owner,
            anchor: env.anchor,
            subst,
            alias_stack: Vec::new(),
            param_projection_stack: Vec::new(),
            opaque_indices: Vec::new(),
            opaque_bounds: Vec::new(),
            argument_impl_trait_indices: Vec::new(),
        })
    }

    pub fn lower(&'lower self, ty: &TypeRef, env: TypeLoweringEnv) -> Result<Ty, D::Error> {
        self.session(env)?.lower_type_ref(ty)
    }
}

/// Mutable state shared while one complete signature is lowered.
///
/// Keeping a session across all parameters is what gives anonymous `impl Trait` occurrences a
/// stable owner-local order. Alias and parameter-projection stacks keep recursive source types
/// bounded, while owner and lookup context change only for the duration of nested declarations.
pub struct TypeLoweringSession<'lower, 'query, D, I, R> {
    query: &'lower TypeLoweringQuery<'lower, 'query, D, I, R>,
    owner: GenericDefRef,
    anchor: TypeLoweringAnchor,
    subst: Substitution,
    alias_stack: Vec<TypeAliasRef>,
    param_projection_stack: Vec<(TypeParamRef, rg_text::Name)>,
    opaque_indices: Vec<(GenericDefRef, usize)>,
    opaque_bounds: Vec<(OpaqueTy, Vec<TraitRefLowering>)>,
    argument_impl_trait_indices: Vec<(GenericDefRef, usize)>,
}

impl<'lower, 'query, D, I, R> TypeLoweringSession<'lower, 'query, D, I, R>
where
    D: DefMapSource,
    I: ItemStoreSource<'query, Error = D::Error>,
    R: TypePathResolver<Error = D::Error>,
{
    /// Lower a source type, treating `impl Trait` as an opaque type occurrence.
    pub(crate) fn lower_type_ref(&mut self, ty: &TypeRef) -> Result<Ty, D::Error> {
        self.lower_type_ref_with_mode(ty, ImplTraitMode::Opaque, None)
    }

    /// Lower a body-written type while giving each explicit `_` a live inference identity.
    ///
    /// Path failures remain `Unknown`; only the syntax node dedicated to inference requests a
    /// variable. Keeping this policy inside the authoritative visitor prevents body inference
    /// from walking `TypeRef` a second time to rediscover holes.
    pub fn lower_type_ref_with_inference(
        &mut self,
        ty: &TypeRef,
        table: &mut InferenceTable,
    ) -> Result<Ty, D::Error> {
        self.lower_type_ref_with_mode(ty, ImplTraitMode::Opaque, Some(table))
    }

    /// Lower a function parameter type, where `impl Trait` introduces an anonymous type parameter.
    ///
    /// `fn visit(value: impl Display)` is generic over a hidden function parameter constrained by
    /// `Display`; it is not the opaque return type produced by `fn make() -> impl Display`.
    pub(crate) fn lower_parameter_type(&mut self, ty: &TypeRef) -> Result<Ty, D::Error> {
        self.lower_type_ref_with_mode(ty, ImplTraitMode::Argument, None)
    }

    /// Opaque identities and their lowered predicates encountered by this complete signature walk.
    pub(crate) fn into_opaque_bounds(self) -> Vec<(OpaqueTy, Vec<TraitRefLowering>)> {
        self.opaque_bounds
    }

    /// Lower a trait bound with `Self` already known.
    pub fn lower_trait_ref(
        &mut self,
        trait_ty: &TypeRef,
        self_ty: Ty,
    ) -> Result<Option<TraitRefLowering>, D::Error> {
        self.lower_trait_ref_with_mode(trait_ty, self_ty, ImplTraitMode::Opaque, None)
    }

    fn lower_trait_ref_with_mode(
        &mut self,
        trait_ty: &TypeRef,
        self_ty: Ty,
        impl_trait_mode: ImplTraitMode,
        inference: Option<&mut InferenceTable>,
    ) -> Result<Option<TraitRefLowering>, D::Error> {
        let TypeRef::Path(path) = trait_ty else {
            return Ok(None);
        };
        let Some(trait_ref) = self.resolve_trait_def(trait_ty)? else {
            return Ok(None);
        };

        self.lower_resolved_trait_ref_with_mode(
            path,
            trait_ref,
            self_ty,
            impl_trait_mode,
            inference,
        )
        .map(Some)
    }

    /// Resolve only the trait definition named by a bound, without interpreting its arguments.
    ///
    /// Associated shorthand lookup uses this identity first to ignore traits that cannot provide
    /// the requested associated type. Lowering every argument eagerly can recurse through the
    /// shorthand being resolved, as in `T: Other<T::Item>`.
    fn resolve_trait_def(&self, trait_ty: &TypeRef) -> Result<Option<TraitDefRef>, D::Error> {
        let TypeRef::Path(path) = trait_ty else {
            return Ok(None);
        };
        let Some(path_key) = path.as_def_map_path() else {
            return Ok(None);
        };
        let TypePathResolution::Trait(trait_ref) = self
            .query
            .resolver
            .resolve_type_path(self.anchor, &path_key)?
        else {
            return Ok(None);
        };
        Ok(Some(trait_ref))
    }

    fn lower_resolved_trait_ref_with_mode(
        &mut self,
        path: &TypePath,
        trait_ref: TraitDefRef,
        self_ty: Ty,
        impl_trait_mode: ImplTraitMode,
        mut inference: Option<&mut InferenceTable>,
    ) -> Result<TraitRefLowering, D::Error> {
        let generics = self
            .query
            .item_paths
            .generics()
            .generics(GenericDefRef::Trait(trait_ref))?;
        let mut seed = Substitution::new();
        if let Some(self_param) = generics.iter().find_map(|param| {
            matches!(param.source(), GenericParamSource::TraitSelf).then_some(param.param())
        }) {
            seed.push(self_param, GenericArg::Type(Box::new(self_ty)));
        }

        let syntax_args = path
            .segments
            .last()
            .map(|segment| segment.args.as_slice())
            .unwrap_or_default();
        let args = self.lower_generic_args(
            &generics,
            syntax_args,
            &seed,
            impl_trait_mode,
            inference.as_deref_mut(),
        )?;
        let application = TraitApplication {
            def: trait_ref,
            args,
        };
        let associated_types =
            self.lower_associated_bindings(&application, syntax_args, impl_trait_mode, inference)?;
        Ok(TraitRefLowering {
            application,
            associated_types,
        })
    }

    /// Lower written positional arguments against one definition's canonical parameter order.
    ///
    /// Parent arguments come from the active semantic substitution; the target's own arguments
    /// are consumed from syntax and omitted positions receive their normal semantic placeholder or
    /// default. Associated bindings belong to `lower_trait_ref`, not this positional list.
    pub fn lower_generic_args_for(
        &mut self,
        target: GenericDefRef,
        syntax_args: &[ItemGenericArg],
    ) -> Result<GenericArgs, D::Error> {
        let generics = self.query.item_paths.generics().generics(target)?;
        let mut parent_seed = Substitution::new();
        for param in generics.iter().take(generics.parent_len()) {
            if let Some(arg) = self.subst.get(param.param()) {
                parent_seed.push(param.param(), arg.clone());
            }
        }
        self.lower_generic_args(
            &generics,
            syntax_args,
            &parent_seed,
            ImplTraitMode::Opaque,
            None,
        )
    }

    /// Lower every trait predicate visible from this owner.
    pub(crate) fn lower_clauses(&mut self) -> Result<Vec<Clause>, D::Error> {
        let generics = self.query.item_paths.generics().generics(self.owner)?;
        let mut inline_bounds = Vec::new();
        let mut predicate_owners = UniqueVec::new();
        for param in generics.iter() {
            let owner = param.param().owner();
            predicate_owners.push(owner);
            let GenericParamRef::Type(param_ref) = param.param() else {
                continue;
            };
            let bounds = match param.source() {
                GenericParamSource::Type(param) => param.bounds.clone(),
                GenericParamSource::ArgumentImplTrait(bounds) => bounds.to_vec(),
                GenericParamSource::Lifetime(_)
                | GenericParamSource::Const(_)
                | GenericParamSource::TraitSelf => continue,
            };
            inline_bounds.push((param_ref.owner, Ty::Param(param_ref), bounds));
        }
        predicate_owners.push(self.owner);

        let mut where_predicates = Vec::new();
        for owner in predicate_owners {
            if let Some(item) = self
                .query
                .item_paths
                .items()
                .semantic_item_view(owner.into())?
                && let Some(params) = item.generic_params()
            {
                where_predicates.extend(
                    params
                        .where_predicates
                        .iter()
                        .cloned()
                        .map(|predicate| (owner, predicate)),
                );
            }
        }

        let mut clauses = Vec::new();
        for (owner, subject, bounds) in inline_bounds {
            let anchor = self.anchor_for_owner(owner)?;
            self.with_owner_anchor(owner, anchor, |session| {
                session.lower_bound_clauses(subject, &bounds, &mut clauses)
            })?;
        }
        for (owner, predicate) in where_predicates {
            let WherePredicate::Type { ty, bounds } = predicate else {
                continue;
            };
            let anchor = self.anchor_for_owner(owner)?;
            self.with_owner_anchor(owner, anchor, |session| {
                let subject = session.lower_type_ref(&ty)?;
                session.lower_bound_clauses(subject, &bounds, &mut clauses)
            })?;
        }
        Ok(clauses)
    }

    /// Resolve syntax owned by a parent declaration in that declaration's generic namespace.
    ///
    /// An associated function may shadow an impl parameter with the same name. Switching the
    /// owner while reading inherited bounds keeps those source names attached to their original
    /// owner-scoped identities. The active substitution remains shared because it already carries
    /// arguments for the full parent chain.
    fn with_owner_anchor<T>(
        &mut self,
        owner: GenericDefRef,
        anchor: TypeLoweringAnchor,
        lower: impl FnOnce(&mut Self) -> Result<T, D::Error>,
    ) -> Result<T, D::Error> {
        let previous_owner = self.owner;
        let previous_anchor = self.anchor;
        self.owner = owner;
        self.anchor = anchor;
        let result = lower(self);
        self.owner = previous_owner;
        self.anchor = previous_anchor;
        result
    }

    fn anchor_for_owner(&self, owner: GenericDefRef) -> Result<TypeLoweringAnchor, D::Error> {
        if owner == self.owner {
            return Ok(self.anchor);
        }
        Ok(self
            .query
            .item_paths
            .items()
            .type_path_context_for_generic_def(owner)?
            .map(TypeLoweringAnchor::Context)
            .unwrap_or(self.anchor))
    }

    fn lower_type_ref_with_mode(
        &mut self,
        ty: &TypeRef,
        impl_trait_mode: ImplTraitMode,
        mut inference: Option<&mut InferenceTable>,
    ) -> Result<Ty, D::Error> {
        match ty {
            TypeRef::Unknown(_) | TypeRef::DynTrait(_) => Ok(Ty::Unknown),
            TypeRef::Infer => Ok(inference
                .as_deref_mut()
                .map(InferenceTable::new_type_var)
                .unwrap_or(Ty::Unknown)),
            TypeRef::Never => Ok(Ty::Never),
            TypeRef::Unit => Ok(Ty::Unit),
            TypeRef::Tuple(types) => Ok(Ty::tuple(
                types
                    .iter()
                    .map(|ty| {
                        self.lower_type_ref_with_mode(ty, impl_trait_mode, inference.as_deref_mut())
                    })
                    .collect::<Result<_, _>>()?,
            )),
            TypeRef::Reference {
                lifetime,
                mutability,
                inner,
            } => {
                let lifetime = lifetime
                    .as_ref()
                    .map(|lifetime| self.lower_lifetime(lifetime))
                    .transpose()?
                    .unwrap_or(Lifetime::Erased);
                Ok(Ty::reference_with_lifetime(
                    lifetime,
                    *mutability,
                    self.lower_type_ref_with_mode(
                        inner,
                        impl_trait_mode,
                        inference.as_deref_mut(),
                    )?,
                ))
            }
            TypeRef::RawPointer { mutability, inner } => Ok(Ty::raw_pointer(
                *mutability,
                self.lower_type_ref_with_mode(inner, impl_trait_mode, inference.as_deref_mut())?,
            )),
            TypeRef::Slice(inner) => Ok(Ty::slice(self.lower_type_ref_with_mode(
                inner,
                impl_trait_mode,
                inference.as_deref_mut(),
            )?)),
            TypeRef::Array { inner, len } => Ok(Ty::array(
                self.lower_type_ref_with_mode(inner, impl_trait_mode, inference.as_deref_mut())?,
                self.lower_const(len.as_deref())?,
            )),
            TypeRef::FnPointer { params, ret } => Ok(Ty::fn_pointer(
                params
                    .iter()
                    .map(|param| {
                        self.lower_type_ref_with_mode(
                            param,
                            impl_trait_mode,
                            inference.as_deref_mut(),
                        )
                    })
                    .collect::<Result<_, _>>()?,
                self.lower_type_ref_with_mode(ret, impl_trait_mode, inference.as_deref_mut())?,
            )),
            TypeRef::ImplTrait(_) if impl_trait_mode == ImplTraitMode::Argument => self
                .next_argument_impl_trait_param()?
                .map(Ty::Param)
                .map_or(Ok(Ty::Unknown), Ok),
            TypeRef::ImplTrait(bounds) => {
                let opaque = OpaqueTyRef {
                    owner: self.owner,
                    id: OpaqueTyId(self.next_opaque_index()),
                };
                let generics = self.query.item_paths.generics().generics(self.owner)?;
                let opaque = OpaqueTy {
                    opaque,
                    args: self.subst.args_for(&generics),
                };
                let self_ty = Ty::Alias(AliasTy::Opaque(opaque.clone()));
                let mut lowered_bounds = Vec::new();
                for bound in bounds {
                    let TypeBound::Trait(trait_ty) = bound else {
                        continue;
                    };
                    if let Some(bound) = self.lower_trait_ref(trait_ty, self_ty.clone())? {
                        lowered_bounds.push(bound);
                    }
                }
                self.opaque_bounds.push((opaque, lowered_bounds));
                Ok(self_ty)
            }
            TypeRef::Path(path) => self.lower_type_path(path, impl_trait_mode, inference),
        }
    }

    fn lower_type_path(
        &mut self,
        path: &TypePath,
        impl_trait_mode: ImplTraitMode,
        mut inference: Option<&mut InferenceTable>,
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
            let prefix_ty = self.lower_type_ref_with_mode(
                &TypeRef::Path(prefix),
                impl_trait_mode,
                inference.as_deref_mut(),
            )?;
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
                if let Some(self_ty) = self.lower_inherent_self()? {
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
                    inference.as_deref_mut(),
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
                    inference.as_deref_mut(),
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

    /// Lower inherent `Self` through the impl header that defines it.
    ///
    /// `Self` and the concrete spelling must produce the same `Adt` and argument list. Rebuilding
    /// an ADT directly from the resolved definition would lose relationships such as
    /// `impl<T> Wrapper<T> { fn get(&self) -> Self }`.
    fn lower_inherent_self(&mut self) -> Result<Option<Ty>, D::Error> {
        let TypeLoweringAnchor::Context(context) = self.anchor else {
            return Ok(None);
        };
        let Some(impl_ref) = context.impl_ref else {
            return Ok(None);
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
        mut inference: Option<&mut InferenceTable>,
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
                let self_ty = self.lower_type_ref_with_mode(
                    self_ty_ref,
                    impl_trait_mode,
                    inference.as_deref_mut(),
                )?;
                let Some(GenericParamRef::Type(param)) = param else {
                    return Ok(Ty::Unknown);
                };
                self.param_associated_projection(param, self_ty, name)?
            }
            TypePathAnchor::QualifiedTrait { self_ty, trait_ty } => {
                let self_ty = self.lower_type_ref_with_mode(
                    self_ty,
                    impl_trait_mode,
                    inference.as_deref_mut(),
                )?;
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
        inference: Option<&mut InferenceTable>,
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

    fn lower_bound_clauses(
        &mut self,
        subject: Ty,
        bounds: &[TypeBound],
        clauses: &mut Vec<Clause>,
    ) -> Result<(), D::Error> {
        for bound in bounds {
            let TypeBound::Trait(trait_ty) = bound else {
                continue;
            };
            if let Some(trait_ref) = self.lower_trait_ref(trait_ty, subject.clone())? {
                clauses.extend(trait_ref.into_clauses());
            }
        }
        Ok(())
    }

    fn lower_generic_args(
        &mut self,
        generics: &rg_semantic_ir::Generics<'_>,
        syntax_args: &[ItemGenericArg],
        seed: &Substitution,
        impl_trait_mode: ImplTraitMode,
        mut inference: Option<&mut InferenceTable>,
    ) -> Result<GenericArgs, D::Error> {
        let positional = syntax_args
            .iter()
            .filter(|arg| {
                !matches!(
                    arg,
                    ItemGenericArg::AssocType { .. } | ItemGenericArg::Unsupported(_)
                )
            })
            .collect::<Vec<_>>();
        let mut syntax_index = 0;
        let mut args = Vec::with_capacity(generics.len());
        let mut resolved = seed.clone();

        for (param_index, param) in generics.iter().enumerate() {
            if let Some(arg) = seed.get(param.param()) {
                args.push(arg.clone());
                resolved.push(param.param(), arg.clone());
                continue;
            }
            if param_index < generics.parent_len() {
                let arg = Substitution::unknown_arg(param.param());
                resolved.push(param.param(), arg.clone());
                args.push(arg);
                continue;
            }

            let syntax = positional.get(syntax_index).copied();
            let arg = match (param.param(), syntax) {
                (GenericParamRef::Lifetime(_), Some(ItemGenericArg::Lifetime(name))) => {
                    syntax_index += 1;
                    GenericArg::Lifetime(self.lower_lifetime(name)?)
                }
                // Rust permits omitted lifetime args without shifting following type/const args.
                (GenericParamRef::Lifetime(_), _) => GenericArg::Lifetime(Lifetime::Erased),
                (GenericParamRef::Type(_), Some(ItemGenericArg::Type(ty))) => {
                    syntax_index += 1;
                    GenericArg::Type(Box::new(self.lower_type_ref_with_mode(
                        ty,
                        impl_trait_mode,
                        inference.as_deref_mut(),
                    )?))
                }
                (GenericParamRef::Type(_), Some(ItemGenericArg::FnTraitArgs { params, .. })) => {
                    syntax_index += 1;
                    GenericArg::Type(Box::new(Ty::tuple(
                        params
                            .iter()
                            .map(|ty| {
                                self.lower_type_ref_with_mode(
                                    ty,
                                    impl_trait_mode,
                                    inference.as_deref_mut(),
                                )
                            })
                            .collect::<Result<_, _>>()?,
                    )))
                }
                (GenericParamRef::Const(_), Some(ItemGenericArg::Const(value))) => {
                    syntax_index += 1;
                    GenericArg::Const(self.lower_const(Some(value))?)
                }
                (GenericParamRef::Type(_), _)
                    if matches!(
                        param.source(),
                        GenericParamSource::Type(source) if source.default.is_some()
                    ) =>
                {
                    let GenericParamSource::Type(source) = param.source() else {
                        unreachable!("guard accepts only source type parameters")
                    };
                    GenericArg::Type(Box::new(
                        self.lower_default_type(
                            generics.owner(),
                            source
                                .default
                                .as_ref()
                                .expect("guard requires a type default"),
                            &resolved,
                        )?,
                    ))
                }
                (GenericParamRef::Const(_), _)
                    if matches!(
                        param.source(),
                        GenericParamSource::Const(source) if source.default.is_some()
                    ) =>
                {
                    let GenericParamSource::Const(source) = param.source() else {
                        unreachable!("guard accepts only source const parameters")
                    };
                    GenericArg::Const(
                        self.lower_default_const(
                            generics.owner(),
                            source
                                .default
                                .as_deref()
                                .expect("guard requires a const default"),
                            &resolved,
                        )?,
                    )
                }
                (param, _) => Substitution::unknown_arg(param),
            };
            resolved.push(param.param(), arg.clone());
            args.push(arg);
        }

        Ok(args.into())
    }

    fn lower_default_type(
        &mut self,
        owner: GenericDefRef,
        ty: &TypeRef,
        subst: &Substitution,
    ) -> Result<Ty, D::Error> {
        let Some(context) = self
            .query
            .item_paths
            .items()
            .type_path_context_for_generic_def(owner)?
        else {
            return Ok(Ty::Unknown);
        };
        let previous_owner = self.owner;
        let previous_anchor = self.anchor;
        let previous_subst = std::mem::replace(&mut self.subst, subst.clone());
        self.owner = owner;
        self.anchor = TypeLoweringAnchor::Context(context);
        let result = self.lower_type_ref(ty);
        self.owner = previous_owner;
        self.anchor = previous_anchor;
        self.subst = previous_subst;
        result
    }

    fn lower_default_const(
        &mut self,
        owner: GenericDefRef,
        text: &str,
        subst: &Substitution,
    ) -> Result<ConstValue, D::Error> {
        let Some(context) = self
            .query
            .item_paths
            .items()
            .type_path_context_for_generic_def(owner)?
        else {
            return Ok(ConstValue::Unknown);
        };
        let previous_owner = self.owner;
        let previous_anchor = self.anchor;
        let previous_subst = std::mem::replace(&mut self.subst, subst.clone());
        self.owner = owner;
        self.anchor = TypeLoweringAnchor::Context(context);
        let result = self.lower_const(Some(text));
        self.owner = previous_owner;
        self.anchor = previous_anchor;
        self.subst = previous_subst;
        result
    }

    fn lower_associated_bindings(
        &mut self,
        application: &TraitApplication,
        syntax_args: &[ItemGenericArg],
        impl_trait_mode: ImplTraitMode,
        mut inference: Option<&mut InferenceTable>,
    ) -> Result<Vec<AssocTypeBinding>, D::Error> {
        let mut bindings = Vec::new();
        for arg in syntax_args {
            let output_name;
            let (name, ty) = match arg {
                ItemGenericArg::AssocType { name, ty } => (name, ty.as_ref()),
                ItemGenericArg::FnTraitArgs { ret, .. } => {
                    output_name = rg_text::Name::new("Output");
                    (&output_name, Some(ret.as_ref()))
                }
                ItemGenericArg::Type(_)
                | ItemGenericArg::Lifetime(_)
                | ItemGenericArg::Const(_)
                | ItemGenericArg::Unsupported(_) => continue,
            };
            let Some(alias) = self.associated_type_projection(application, name)? else {
                continue;
            };
            // `AssocTypeBinding` belongs to the surrounding trait application. A transformed
            // supertrait projection needs its own argument list, which that compact goal shape
            // cannot represent yet. Keeping it unresolved is safer than attaching the equality
            // to the wrong application; `Fn`/`FnMut` inherit `Output` with the same arguments and
            // therefore take the supported path.
            if alias.args != application.args {
                continue;
            }
            let Some(ty) = ty else {
                continue;
            };
            bindings.push(AssocTypeBinding {
                associated_ty: alias.associated_ty,
                ty: self.lower_type_ref_with_mode(ty, impl_trait_mode, inference.as_deref_mut())?,
            });
        }
        Ok(bindings)
    }

    /// Find an associated type on a trait or one of its supertraits.
    ///
    /// Associated items are inherited semantically even though they remain owned by the trait
    /// that declared them. In particular, callable syntax on `Fn` and `FnMut` constrains
    /// `FnOnce::Output`, so direct-item lookup is not enough.
    fn associated_type_projection(
        &mut self,
        application: &TraitApplication,
        name: &rg_text::Name,
    ) -> Result<Option<ProjectionTy>, D::Error> {
        self.associated_type_projection_inner(application, name, &[])
    }

    fn associated_type_projection_inner(
        &mut self,
        application: &TraitApplication,
        name: &rg_text::Name,
        lineage: &[rg_ir_model::TraitDefRef],
    ) -> Result<Option<ProjectionTy>, D::Error> {
        if lineage.contains(&application.def) {
            return Ok(None);
        }
        let Some(data) = self.query.item_paths.items().trait_data(application.def)? else {
            return Ok(None);
        };
        if let Some(alias) = self
            .query
            .item_paths
            .items()
            .declared_associated_type_by_name(application.def, name.as_str())?
        {
            return Ok(Some(ProjectionTy {
                associated_ty: alias,
                args: application.args.clone(),
            }));
        }

        let super_traits = data.super_traits.clone();
        let owner = GenericDefRef::Trait(application.def);
        let generics = self.query.item_paths.generics().generics(owner)?;
        let Some(self_param) = generics.iter().find_map(|param| {
            matches!(param.source(), GenericParamSource::TraitSelf).then_some(param.param())
        }) else {
            return Ok(None);
        };
        let GenericParamRef::Type(self_param) = self_param else {
            return Ok(None);
        };
        let anchor = self.anchor_for_owner(owner)?;
        let application_subst = Substitution::from_args(&generics, &application.args);
        let mut next_lineage = lineage.to_vec();
        next_lineage.push(application.def);

        for bound in super_traits {
            let TypeBound::Trait(trait_ty) = bound else {
                continue;
            };

            // Supertrait syntax is written in the declaring trait's generic namespace. Lower it
            // against identity parameters first, then apply the concrete current application.
            let previous_subst =
                std::mem::replace(&mut self.subst, Substitution::identity(&generics));
            let lowered = self.with_owner_anchor(owner, anchor, |session| {
                session.lower_trait_ref(&trait_ty, Ty::Param(self_param))
            });
            self.subst = previous_subst;
            let Some(super_trait) = lowered? else {
                continue;
            };
            let super_application =
                application_subst.apply_trait_application(&super_trait.application);
            if let Some(alias) =
                self.associated_type_projection_inner(&super_application, name, &next_lineage)?
            {
                return Ok(Some(alias));
            }
        }
        Ok(None)
    }

    /// Check whether a trait exposes an associated type directly or through a supertrait.
    ///
    /// This walk intentionally follows identities only. Its caller uses the answer to decide
    /// whether lowering a bound's generic arguments can contribute to `T::Assoc` resolution.
    fn trait_exposes_associated_type(
        &mut self,
        trait_ref: TraitDefRef,
        name: &rg_text::Name,
    ) -> Result<bool, D::Error> {
        self.trait_exposes_associated_type_inner(trait_ref, name, &[])
    }

    fn trait_exposes_associated_type_inner(
        &mut self,
        trait_ref: TraitDefRef,
        name: &rg_text::Name,
        lineage: &[TraitDefRef],
    ) -> Result<bool, D::Error> {
        if lineage.contains(&trait_ref) {
            return Ok(false);
        }
        let Some(data) = self.query.item_paths.items().trait_data(trait_ref)? else {
            return Ok(false);
        };
        if self
            .query
            .item_paths
            .items()
            .declared_associated_type_by_name(trait_ref, name.as_str())?
            .is_some()
        {
            return Ok(true);
        }

        let super_traits = data.super_traits.clone();
        let owner = GenericDefRef::Trait(trait_ref);
        let anchor = self.anchor_for_owner(owner)?;
        let mut next_lineage = lineage.to_vec();
        next_lineage.push(trait_ref);
        self.with_owner_anchor(owner, anchor, |session| {
            for bound in super_traits {
                let TypeBound::Trait(trait_ty) = bound else {
                    continue;
                };
                let Some(super_trait) = session.resolve_trait_def(&trait_ty)? else {
                    continue;
                };
                if session.trait_exposes_associated_type_inner(super_trait, name, &next_lineage)? {
                    return Ok(true);
                }
            }
            Ok(false)
        })
    }

    /// Resolve `T::Assoc` from the unique trait bound on the owner-scoped parameter `T`.
    ///
    /// Bound arguments may contain the same shorthand while candidates are inspected. Re-entering
    /// one active `(T, Assoc)` request would require an infinitely recursive semantic type, so that
    /// inner occurrence stays unresolved instead of growing the Rust stack.
    fn param_associated_projection(
        &mut self,
        param: TypeParamRef,
        self_ty: Ty,
        assoc_name: &rg_text::Name,
    ) -> Result<Option<ProjectionTy>, D::Error> {
        if self
            .param_projection_stack
            .iter()
            .any(|(candidate, name)| *candidate == param && name == assoc_name)
        {
            return Ok(None);
        }

        self.param_projection_stack
            .push((param, assoc_name.clone()));
        let result = self.param_associated_projection_inner(param, self_ty, assoc_name);
        let popped = self
            .param_projection_stack
            .pop()
            .expect("projection request was pushed above");
        debug_assert_eq!(popped.0, param);
        debug_assert_eq!(popped.1, *assoc_name);
        result
    }

    fn param_associated_projection_inner(
        &mut self,
        param: TypeParamRef,
        self_ty: Ty,
        assoc_name: &rg_text::Name,
    ) -> Result<Option<ProjectionTy>, D::Error> {
        let generics = self.query.item_paths.generics().generics(self.owner)?;
        let mut bound_groups = Vec::new();
        if let Some(candidate) = generics
            .iter()
            .find(|candidate| candidate.param() == GenericParamRef::Type(param))
        {
            let bounds = match candidate.source() {
                GenericParamSource::Type(source) => source.bounds.clone(),
                GenericParamSource::ArgumentImplTrait(bounds) => bounds.to_vec(),
                GenericParamSource::Lifetime(_)
                | GenericParamSource::Const(_)
                | GenericParamSource::TraitSelf => Vec::new(),
            };
            bound_groups.push((param.owner, bounds));
        }

        // Child owners may constrain an inherited parameter in their own where-clause. Inspect
        // every declaration owner represented in the full generic list, while identity lookup
        // still lets a child parameter shadow a parent with the same source name.
        let mut predicate_owners = UniqueVec::new();
        for candidate in generics.iter() {
            predicate_owners.push(candidate.param().owner());
        }
        predicate_owners.push(self.owner);
        for owner in predicate_owners {
            let predicates = self
                .query
                .item_paths
                .items()
                .semantic_item_view(owner.into())?
                .and_then(|item| item.generic_params())
                .map(|params| params.where_predicates.clone())
                .unwrap_or_default();
            for predicate in predicates {
                let WherePredicate::Type {
                    ty,
                    bounds: predicate_bounds,
                } = predicate
                else {
                    continue;
                };
                let TypeRef::Path(path) = ty else {
                    continue;
                };
                let Some(name) = path.single_name() else {
                    continue;
                };
                let predicate_param = self
                    .query
                    .item_paths
                    .generics()
                    .generics(owner)?
                    .param_by_name(name.as_str());
                if predicate_param == Some(GenericParamRef::Type(param)) {
                    bound_groups.push((owner, predicate_bounds));
                }
            }
        }

        let mut selected = ExpectedUnique::new();
        for (owner, bounds) in bound_groups {
            let anchor = self.anchor_for_owner(owner)?;
            let unambiguous = self.with_owner_anchor(owner, anchor, |session| {
                for bound in bounds {
                    let TypeBound::Trait(trait_ty) = bound else {
                        continue;
                    };
                    let TypeRef::Path(path) = &trait_ty else {
                        continue;
                    };
                    let Some(trait_def) = session.resolve_trait_def(&trait_ty)? else {
                        continue;
                    };
                    // Candidate discovery is an identity operation. Only a trait that actually
                    // exposes this name needs its argument syntax lowered into a full application.
                    if !session.trait_exposes_associated_type(trait_def, assoc_name)? {
                        continue;
                    }
                    let trait_ref = session.lower_resolved_trait_ref_with_mode(
                        path,
                        trait_def,
                        self_ty.clone(),
                        ImplTraitMode::Opaque,
                        None,
                    )?;
                    let Some(candidate) =
                        session.associated_type_projection(&trait_ref.application, assoc_name)?
                    else {
                        continue;
                    };
                    selected.push(candidate);
                    if selected.is_ambiguous() {
                        return Ok(false);
                    }
                }
                Ok(true)
            })?;
            if !unambiguous {
                return Ok(None);
            }
        }

        Ok(selected.into_option())
    }

    fn param_by_name(&self, name: &str) -> Result<Option<GenericParamRef>, D::Error> {
        Ok(self
            .query
            .item_paths
            .generics()
            .generics(self.owner)?
            .param_by_name(name))
    }

    fn lower_lifetime(&self, name: &rg_text::Name) -> Result<Lifetime, D::Error> {
        if name.as_str() == "'static" {
            return Ok(Lifetime::Static);
        }
        Ok(match self.param_by_name(name.as_str())? {
            Some(GenericParamRef::Lifetime(param)) => {
                match self.subst.get(GenericParamRef::Lifetime(param)) {
                    Some(GenericArg::Lifetime(lifetime)) => *lifetime,
                    Some(GenericArg::Type(_)) | Some(GenericArg::Const(_)) | None => {
                        Lifetime::Param(param)
                    }
                }
            }
            _ => Lifetime::Erased,
        })
    }

    fn lower_const(&self, text: Option<&str>) -> Result<ConstValue, D::Error> {
        let Some(text) = text else {
            return Ok(ConstValue::Unknown);
        };
        Ok(match self.param_by_name(text)? {
            Some(GenericParamRef::Const(param)) => {
                match self.subst.get(GenericParamRef::Const(param)) {
                    Some(GenericArg::Const(value)) => *value,
                    Some(GenericArg::Type(_)) | Some(GenericArg::Lifetime(_)) | None => {
                        ConstValue::Param(param)
                    }
                }
            }
            _ => ConstValue::from_syntax(text),
        })
    }

    /// Allocate the next opaque occurrence ordinal for the active owner.
    ///
    /// Alias bodies can temporarily change `self.owner` during the same session, so each owner
    /// keeps its own counter. Replaying a complete signature walk then produces the same refs.
    fn next_opaque_index(&mut self) -> usize {
        if let Some((_, index)) = self
            .opaque_indices
            .iter_mut()
            .find(|(owner, _)| *owner == self.owner)
        {
            let current = *index;
            *index += 1;
            current
        } else {
            self.opaque_indices.push((self.owner, 1));
            0
        }
    }

    /// Match the next parameter-position `impl Trait` syntax node to its precomputed parameter ref.
    fn next_argument_impl_trait_param(&mut self) -> Result<Option<TypeParamRef>, D::Error> {
        let index = if let Some((_, index)) = self
            .argument_impl_trait_indices
            .iter_mut()
            .find(|(owner, _)| *owner == self.owner)
        {
            let current = *index;
            *index += 1;
            current
        } else {
            self.argument_impl_trait_indices.push((self.owner, 1));
            0
        };

        Ok(self
            .query
            .item_paths
            .generics()
            .generics(self.owner)?
            .iter_self()
            .filter_map(|param| {
                matches!(param.source(), GenericParamSource::ArgumentImplTrait(_))
                    .then_some(param.param())
            })
            .nth(index)
            .and_then(|param| match param {
                GenericParamRef::Type(param) => Some(param),
                GenericParamRef::Lifetime(_) | GenericParamRef::Const(_) => None,
            }))
    }
}

/// Chooses the two different semantic meanings of `impl Trait` syntax.
///
/// Function parameters desugar to anonymous type parameters. Return types and other supported
/// positions introduce opaque type occurrences whose concrete type is intentionally hidden.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ImplTraitMode {
    Argument,
    Opaque,
}
