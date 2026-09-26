//! Continue one lowering walk through source types, bounds, aliases, and generic arguments.
//!
//! Nested declarations temporarily change the owner and substitution inside the same session.
//! Keeping the walk state lets an alias reached through a projection see the aliases already
//! being expanded, and keeps anonymous type identities in source order.

mod bounds;
mod generic_args;
mod path;
mod projection;
mod type_ref;

use rg_def_map::DefMapSource;
use rg_ir_model::{GenericDefRef, GenericParamRef, TraitDefRef, TypeAliasRef, TypeParamRef};
use rg_semantic_ir::ItemStoreSource;
use rg_text::Name;
use rustc_type_ir::TyKind;

use super::{TypeLoweringAnchor, TypeLoweringEnv, TypePathResolver};
use crate::{
    lookup::ItemPathQuery,
    solver::{
        InferenceSubstitution as Substitution, OpaqueTy, SolverInterner, TraitRefLowering, Ty,
    },
};

// Source syntax can be deeply nested even without aliases or projections. This is an emergency
// boundary for malformed/generated input; ordinary Rust types stay far below it.
const MAX_TYPE_LOWERING_DEPTH: usize = 128;

/// State retained while one lowering operation walks its source types.
///
/// For a function signature, keep the same session across all parameters and the return type so
/// anonymous `impl Trait` occurrences get a stable owner-local order. An independent annotation
/// or associated-type lookup can start its own session. Recursive steps always reuse this state:
/// alias and projection stacks detect cycles, while nested declarations temporarily change the
/// owner, lookup context, and substitution.
///
/// The session borrows the sources, resolver, and solver storage directly. Finishing a source
/// walk releases its counters and stacks; the resulting types still belong to the solver storage.
pub struct TypeLoweringSession<'s, 'lower, 'query, D, I, R> {
    item_paths: &'lower ItemPathQuery<'query, D, I>,
    resolver: &'lower R,
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
    pub(crate) fn new(
        item_paths: &'lower ItemPathQuery<'query, D, I>,
        resolver: &'lower R,
        cx: SolverInterner<'s>,
        env: TypeLoweringEnv,
    ) -> Result<Self, D::Error> {
        let generics = item_paths.generics().generics(env.owner)?;
        let subst = Substitution::identity(cx, generics.iter().map(|p| p.param()));

        Ok(Self {
            item_paths,
            resolver,
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

    /// Opaque identities and their lowered predicates encountered by this complete signature walk.
    pub(crate) fn into_opaque_bounds(self) -> Vec<(OpaqueTy<'s>, Vec<TraitRefLowering<'s>>)> {
        self.opaque_bounds
    }

    pub(crate) fn param_ty(&self, param: TypeParamRef) -> Ty<'s> {
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
