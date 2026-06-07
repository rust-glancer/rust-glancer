//! Resolution and very small type inference for Body IR.
//!
//! The resolver consumes the already-lowered body store. It answers only cheap questions:
//! local-vs-item path resolution and simple types that are already present in signatures.

mod body;
mod body_items;
mod expr;
mod normalize;
mod pat;
mod pat_binding;
mod query_source;
mod receiver_items;
mod type_path;
mod type_ref;

use rg_ir_model::{BodyRef, Path, ScopeId, TypePathResolution};
use rg_ir_storage::{DefMapSource, ItemStoreSource};
use rg_package_store::PackageStoreError;
use rg_ty::{MemberMethodCandidateRef, Ty};

use crate::{BodyResolution, ResolvedBodyData};

pub(crate) use self::{
    body::BodyResolver, body::BodyValuePathResolver, body_items::BodyLocalItemQuery,
    query_source::BodyQuerySource, receiver_items::BodyReceiverFunctionQuery,
    type_path::BodyTypePathResolver, type_ref::TypeRefUseSite,
};

/// Query-time lookup from one body-local lexical scope.
///
/// Body scopes have extra lookup rules for bindings and synthetic modules, but the item and
/// DefMap storage they consult comes from providers that route semantic-shaped refs.
#[derive(Clone, Copy)]
pub struct BodyScopeQuery<'a, D, I> {
    source: BodyQuerySource<'a, D, I>,
}

impl<'a, D, I> BodyScopeQuery<'a, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'a, Error = PackageStoreError> + Copy,
{
    pub fn new(def_maps: D, item_stores: I, body_ref: BodyRef, body: &'a ResolvedBodyData) -> Self {
        Self {
            source: BodyQuerySource::new(def_maps, item_stores, body_ref, body),
        }
    }

    pub fn resolve_type_path_in_scope(
        &self,
        scope: ScopeId,
        path: &Path,
    ) -> Result<TypePathResolution, PackageStoreError> {
        BodyTypePathResolver::new(self.source).resolve_in_scope(scope, path)
    }

    pub fn resolve_value_path_in_scope(
        &self,
        scope: ScopeId,
        path: &Path,
    ) -> Result<(BodyResolution, Ty), PackageStoreError> {
        BodyValuePathResolver::new(self.source, None).resolve_nonlocal_path_expr(scope, path)
    }

    pub fn method_candidates_for_ty(
        &self,
        ty: &Ty,
    ) -> Result<Vec<MemberMethodCandidateRef>, PackageStoreError> {
        BodyReceiverFunctionQuery::new(self.source, None).method_candidates_for_ty(ty)
    }
}

// TODO: Should not be here
pub(super) fn push_unique<T: PartialEq>(items: &mut Vec<T>, item: T) {
    // Resolution often merges candidates from local, inherent, and trait sources. Keeping order
    // while deduplicating makes snapshots stable without pretending this is a ranking policy.
    if !items.contains(&item) {
        items.push(item);
    }
}
