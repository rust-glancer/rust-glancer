//! Interpret source types in the working type representation.
//!
//! A session borrows an operation's storage. A body can supply its inference table for written
//! `_`, while declarations keep named parameters such as `T`. Signatures, bounds, aliases and
//! defaults all use this visitor. Independent queries export owned results before releasing the
//! storage; body inference retains the working types until it finalizes its facts.
//!
//! Definition HIR intentionally keeps `TypeRef`. This module is the only place that interprets
//! that syntax as semantic identity. Callers may customize path lookup for a body scope, but they
//! do not get a second recursive visitor or their own rules for parameters, aliases, and args.

mod bounds;
mod generic_args;
mod path;
mod projection;
mod signature;
mod type_ref;

use rg_def_map::DefMapSource;
use rg_ir_model::{
    GenericDefRef, GenericParamRef, Path, ScopeId, TraitDefRef, TypeAliasRef, TypeParamRef,
};
use rg_item_tree::{GenericArg as ItemGenericArg, TypeRef};
use rg_semantic_ir::{Generics, ItemStoreSource, TypePathContext, TypePathResolution};
use rg_text::Name;
use rustc_type_ir::TyKind;

use crate::{
    lookup::ItemPathQuery,
    solver::{
        GenericArgs, InferenceSubstitution as Substitution, InferenceTable, OpaqueTy,
        SemanticDeclarations, SolverInterner, TraitRefLowering, Ty,
    },
};

// Source syntax can be deeply nested even without aliases or projections. This is an emergency
// boundary for malformed/generated input; ordinary Rust types stay far below it.
const MAX_TYPE_LOWERING_DEPTH: usize = 128;

/// Name-lookup starting point for paths encountered during type lowering.
///
/// A path written in a body may resolve through its lexical `Scope`, including body-local items.
/// A path in an item signature instead uses the declaration's module and `Self` binding.
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

    pub fn session<'s>(
        &'lower self,
        cx: SolverInterner<'s>,
        env: TypeLoweringEnv,
    ) -> Result<TypeLoweringSession<'s, 'lower, 'query, D, I, R>, D::Error> {
        let generics = self.item_paths.generics().generics(env.owner)?;
        let subst = Substitution::identity(cx, generics.iter().map(|p| p.param()));

        Ok(TypeLoweringSession {
            query: self,
            cx,
            owner: env.owner,
            anchor: env.anchor,
            subst,
            alias_stack: Vec::new(),
            param_projection_stack: Vec::new(),
            associated_projection_stack: Vec::new(),
            type_ref_depth: 0,
            limit_reported: false,
            opaque_indices: Vec::new(),
            opaque_bounds: Vec::new(),
            argument_impl_trait_indices: Vec::new(),
        })
    }

    /// Own the temporary storage for an independent result. Declaration callbacks during this
    /// operation use this same storage; recursive source steps never create their own arenas.
    pub fn with_storage<T>(
        &self,
        run: impl for<'s> FnOnce(SolverInterner<'s>) -> Result<T, D::Error>,
    ) -> Result<T, D::Error> {
        let declarations = SemanticDeclarations::for_lowering(self.item_paths, self.resolver);
        declarations.with_storage(run)?
    }

    pub fn lower(&self, ty: &TypeRef, env: TypeLoweringEnv) -> Result<crate::Ty, D::Error> {
        self.with_storage(|cx| {
            let ty = self.session(cx, env)?.lower_type_ref(ty)?;
            Ok(cx.raise_ty(ty).unwrap_or(crate::Ty::Unknown))
        })
    }

    /// Interpret a body annotation in the caller's inference context. A written `_` belongs to
    /// this table, so later constraints on the result can fill it in.
    pub fn lower_inference_type<'s>(
        &self,
        ty: &TypeRef,
        env: TypeLoweringEnv,
        table: &InferenceTable<'s>,
    ) -> Result<Ty<'s>, D::Error> {
        self.session(table.interner(), env)?
            .lower_type_ref_with_inference(ty, table)
    }

    /// Interpret explicit arguments such as `collect::<Vec<_>>()` without exporting the
    /// caller's variables into an independent result.
    pub fn lower_inference_args<'s>(
        &self,
        generics: &Generics<'_>,
        args: &[ItemGenericArg],
        env: TypeLoweringEnv,
        table: &InferenceTable<'s>,
    ) -> Result<GenericArgs<'s>, D::Error> {
        self.session(table.interner(), env)?
            .lower_generic_args_for(generics, args, Some(table))
    }
}

/// Mutable state shared while one complete signature is lowered.
///
/// Keeping a session across all parameters is what gives anonymous `impl Trait` occurrences a
/// stable owner-local order. Alias and projection stacks keep recursive source types bounded,
/// while owner and lookup context change only for the duration of nested declarations.
pub struct TypeLoweringSession<'s, 'lower, 'query, D, I, R> {
    query: &'lower TypeLoweringQuery<'lower, 'query, D, I, R>,
    owner: GenericDefRef,
    anchor: TypeLoweringAnchor,
    cx: SolverInterner<'s>,
    subst: Substitution<'s>,
    alias_stack: Vec<TypeAliasRef>,
    param_projection_stack: Vec<(TypeParamRef, Name)>,
    associated_projection_stack: Vec<(TraitDefRef, Name)>,
    type_ref_depth: usize,
    limit_reported: bool,
    opaque_indices: Vec<(GenericDefRef, usize)>,
    opaque_bounds: Vec<(OpaqueTy<'s>, Vec<TraitRefLowering<'s>>)>,
    argument_impl_trait_indices: Vec<(GenericDefRef, usize)>,
}

impl<'s, 'lower, 'query, D, I, R> TypeLoweringSession<'s, 'lower, 'query, D, I, R>
where
    D: DefMapSource,
    I: ItemStoreSource<'query, Error = D::Error>,
    R: TypePathResolver<Error = D::Error>,
{
    /// Opaque identities and their lowered predicates encountered by this complete signature walk.
    pub(crate) fn into_opaque_bounds(self) -> Vec<(OpaqueTy<'s>, Vec<TraitRefLowering<'s>>)> {
        self.opaque_bounds
    }

    fn param_ty(&self, param: TypeParamRef) -> Ty<'s> {
        self.subst
            .get(GenericParamRef::Type(param))
            .and_then(|a| a.as_ty())
            .unwrap_or_else(|| {
                let params = self.cx.generics(param.owner.into()).params;
                self.cx
                    .param(GenericParamRef::Type(param), params)
                    .map(|p| Ty::new(self.cx, TyKind::Param(p)))
                    .unwrap_or_else(|| self.cx.unknown())
            })
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

    /// Make a fail-soft lowering boundary observable without flooding one signature walk.
    fn report_limit(&mut self, kind: &'static str, limit: Option<usize>) {
        if self.limit_reported {
            return;
        }
        self.limit_reported = true;
        crate::profile::metric::TYPE_LOWERING_LIMIT_EXHAUSTIONS.inc(kind);
        tracing::warn!(
            owner = ?self.owner,
            anchor = ?self.anchor,
            limit_kind = kind,
            limit,
            "type lowering stopped at a recursion limit; affected types remain unknown"
        );
    }

    fn param_by_name(&self, name: &str) -> Result<Option<GenericParamRef>, D::Error> {
        Ok(self
            .query
            .item_paths
            .generics()
            .generics(self.owner)?
            .param_by_name(name))
    }
}

/// Chooses the two different semantic meanings of `impl Trait` syntax.
///
/// Function parameters desugar to anonymous type parameters. Return types and other supported
/// positions introduce opaque type occurrences whose concrete type is intentionally hidden.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImplTraitMode {
    Argument,
    Opaque,
}
