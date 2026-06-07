use rg_ir_model::{BodyRef, Path, ScopeId, TypePathResolution};
use rg_ir_storage::{DefMapSource, ItemStoreSource};
use rg_package_store::PackageStoreError;
use rg_ty::{MemberMethodCandidateRef, Ty};

use crate::{BodyResolution, ResolvedBodyData};

use crate::resolution::{BodyResolutionContext, BodyValuePathResolver};

/// Query-time lookup from one body-local lexical scope.
///
/// Body scopes have extra lookup rules for bindings and synthetic modules, but the item and
/// DefMap storage they consult comes from providers that route semantic-shaped refs.
#[derive(Clone, Copy)]
pub struct BodyScopeQuery<'a, D, I> {
    context: BodyResolutionContext<'a, D, I>,
}

impl<'a, D, I> BodyScopeQuery<'a, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'a, Error = PackageStoreError> + Copy,
{
    pub fn new(def_maps: D, item_stores: I, body_ref: BodyRef, body: &'a ResolvedBodyData) -> Self {
        Self {
            context: BodyResolutionContext::new(def_maps, item_stores, body_ref, body, None),
        }
    }

    pub fn resolve_type_path_in_scope(
        &self,
        scope: ScopeId,
        path: &Path,
    ) -> Result<TypePathResolution, PackageStoreError> {
        self.context
            .type_path_resolver()
            .resolve_in_scope(scope, path)
    }

    pub fn resolve_value_path_in_scope(
        &self,
        scope: ScopeId,
        path: &Path,
    ) -> Result<(BodyResolution, Ty), PackageStoreError> {
        BodyValuePathResolver::new(self.context).resolve_nonlocal_path_expr(scope, path)
    }

    pub fn method_candidates_for_ty(
        &self,
        ty: &Ty,
    ) -> Result<Vec<MemberMethodCandidateRef>, PackageStoreError> {
        self.context
            .receiver_functions()
            .method_candidates_for_ty(ty)
    }
}
