//! Interpret source types in the working type representation.
//!
//! Queries start lowering operations and assemble their results. A session carries the state of
//! one source walk; recursive steps keep using that session so aliases and projections share cycle
//! tracking and anonymous `impl Trait` occurrences receive consistent identities.
//!
//! A session borrows an operation's storage. A body can supply its inference table for written
//! `_`, while declarations keep named parameters such as `T`. Independent queries export owned
//! results before releasing the storage; body inference retains working types until finalization.
//!
//! Definition HIR intentionally keeps `TypeRef`. This module is the only place that interprets
//! that syntax as semantic identity. Callers may customize path lookup for a body scope, but they
//! do not get a second recursive visitor or their own rules for parameters, aliases, and args.

mod query;
mod session;

use rg_def_map::DefMapSource;
use rg_ir_model::{GenericDefRef, Path, ScopeId};
use rg_semantic_ir::{ItemStoreSource, TypePathContext, TypePathResolution};

pub use self::{query::TypeLoweringQuery, session::TypeLoweringSession};
use crate::lookup::ItemPathQuery;

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
