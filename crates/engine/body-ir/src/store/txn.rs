//! Read transactions over frozen Body IR package data.

use rg_def_map::{DefMapReadTxn, PackageSlot, Path};
use rg_ir_model::{BodyRef, FieldRef, ScopeId, TargetRef, TypePathResolution};
use rg_package_store::{PackageStoreError, PackageStoreReadTxn};
use rg_semantic_ir::{
    ItemPathQuery, ItemStoreQuery, SemanticIrReadTxn, TypePathContext, ty_from_type_ref_in_context,
};
use rg_ty::{Ty, TypeSubst};

use crate::{
    BodyData, BodyResolution, PackageBodies, TargetBodies,
    resolution::{BodyQuerySource, BodyTypePathResolver, BodyValuePathResolver},
};

/// Read-only Body IR access for one query transaction.
#[derive(Debug, Clone)]
pub struct BodyIrReadTxn<'db> {
    packages: PackageStoreReadTxn<'db, PackageBodies>,
}

impl<'db> BodyIrReadTxn<'db> {
    pub(crate) fn from_package_store(packages: PackageStoreReadTxn<'db, PackageBodies>) -> Self {
        Self { packages }
    }

    pub fn package(&self, package: PackageSlot) -> Result<&PackageBodies, PackageStoreError> {
        self.packages.read(package)
    }

    pub fn target_bodies(
        &self,
        target: TargetRef,
    ) -> Result<Option<&TargetBodies>, PackageStoreError> {
        let package = self.package(target.package)?;
        Ok(package.target(target.target))
    }

    /// Returns one body by project-wide body reference.
    pub fn body_data(&self, body_ref: BodyRef) -> Result<Option<&BodyData>, PackageStoreError> {
        Ok(self
            .target_bodies(body_ref.target)?
            .and_then(|target_bodies| target_bodies.body(body_ref.body)))
    }

    /// Resolves a type path from a body-local lexical scope.
    ///
    /// This is a query-time counterpart to body lowering: local items in the body scope are checked
    /// before falling back to semantic type resolution.
    pub fn resolve_type_path_in_scope(
        &self,
        def_map: &DefMapReadTxn<'db>,
        semantic_ir: &SemanticIrReadTxn<'db>,
        body_ref: BodyRef,
        scope: ScopeId,
        path: &Path,
    ) -> Result<TypePathResolution, PackageStoreError> {
        let body = self.body_data(body_ref)?;
        let Some(body) = body else {
            return Ok(TypePathResolution::Unknown);
        };

        let source = BodyQuerySource::new(def_map, semantic_ir, body_ref, body);
        BodyTypePathResolver::new(source).resolve_in_scope(scope, path)
    }

    /// Resolves a value path from a body-local lexical scope.
    ///
    /// Analysis uses this for cursor prefixes such as associated functions and enum variants,
    /// where a selected path segment can differ from the surrounding expression's final result.
    pub fn resolve_value_path_in_scope(
        &self,
        def_map: &DefMapReadTxn<'db>,
        semantic_ir: &SemanticIrReadTxn<'db>,
        body_ref: BodyRef,
        scope: ScopeId,
        path: &Path,
    ) -> Result<(BodyResolution, Ty), PackageStoreError> {
        let body = self.body_data(body_ref)?;
        let Some(body) = body else {
            return Ok((BodyResolution::Unknown, Ty::Unknown));
        };

        let source = BodyQuerySource::new(def_map, semantic_ir, body_ref, body);
        BodyValuePathResolver::new(source, None).resolve_nonlocal_path_expr(scope, path)
    }

    /// Converts one Semantic IR field declaration type into Body IR's small type vocabulary.
    pub fn ty_for_field(
        &self,
        def_map: &DefMapReadTxn<'db>,
        semantic_ir: &SemanticIrReadTxn<'db>,
        field_ref: FieldRef,
    ) -> Result<Option<Ty>, PackageStoreError> {
        // Field declarations live in Semantic IR, but Analysis expects Body IR's small type
        // vocabulary. Use the owning module as the type-path context for the field signature.
        let Some(field_data) = ItemStoreQuery::new(semantic_ir).field_data(field_ref)? else {
            return Ok(None);
        };
        let item_paths = ItemPathQuery::new(def_map, semantic_ir);
        Ok(Some(ty_from_type_ref_in_context(
            &item_paths,
            &field_data.field.ty,
            TypePathContext::module(field_data.owner_module),
            Ty::Unknown,
            &TypeSubst::new(),
        )?))
    }
}
