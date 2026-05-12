//! Read transactions over frozen Body IR package data.

use rg_def_map::{DefMapReadTxn, PackageSlot, Path, TargetRef};
use rg_package_store::{PackageRead, PackageStoreError, PackageStoreReadTxn};
use rg_semantic_ir::{FieldRef, FunctionRef, SemanticIrReadTxn, TraitApplicability};

use crate::{
    BodyData, BodyFieldData, BodyFieldRef, BodyFunctionData, BodyFunctionRef, BodyItemRef,
    BodyLocalNominalTy, BodyNominalTy, BodyRef, BodyResolution, BodyTy, BodyTypePathResolution,
    PackageBodies, ScopeId, TargetBodies, resolution,
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

    pub fn package(
        &self,
        package: PackageSlot,
    ) -> Result<PackageRead<'_, PackageBodies>, PackageStoreError> {
        self.packages.read(package)
    }

    pub fn target_bodies(
        &self,
        target: TargetRef,
    ) -> Result<Option<&TargetBodies>, PackageStoreError> {
        let package = self.package(target.package)?;
        Ok(package.into_ref().target(target.target))
    }

    /// Returns the body associated with a semantic function, if that function has a body.
    pub fn body_for_function(
        &self,
        function: FunctionRef,
    ) -> Result<Option<BodyRef>, PackageStoreError> {
        let Some(target_bodies) = self.target_bodies(function.target)? else {
            return Ok(None);
        };
        let Some(body) = target_bodies.body_for_function(function.id) else {
            return Ok(None);
        };

        Ok(Some(BodyRef {
            target: function.target,
            body,
        }))
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
    ) -> Result<BodyTypePathResolution, PackageStoreError> {
        let body = self.body_data(body_ref)?;
        resolution::resolve_type_path_in_scope(body, def_map, semantic_ir, body_ref, scope, path)
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
    ) -> Result<(BodyResolution, BodyTy), PackageStoreError> {
        let body = self.body_data(body_ref)?;
        resolution::resolve_value_path_in_scope(body, def_map, semantic_ir, body_ref, scope, path)
    }

    /// Converts one Semantic IR field declaration type into Body IR's small type vocabulary.
    pub fn ty_for_field(
        &self,
        def_map: &DefMapReadTxn<'db>,
        semantic_ir: &SemanticIrReadTxn<'db>,
        field_ref: FieldRef,
    ) -> Result<Option<BodyTy>, PackageStoreError> {
        resolution::ty_for_field(def_map, semantic_ir, field_ref)
    }

    /// Checks whether a semantic function is a plausible method candidate for a receiver type.
    pub fn semantic_function_applies_to_receiver(
        &self,
        def_map: &DefMapReadTxn<'db>,
        semantic_ir: &SemanticIrReadTxn<'db>,
        function_ref: FunctionRef,
        receiver_ty: &BodyNominalTy,
    ) -> Result<bool, PackageStoreError> {
        resolution::semantic_function_applies_to_receiver(
            def_map,
            semantic_ir,
            function_ref,
            receiver_ty,
        )
    }

    /// Returns trait-associated function candidates for a semantic receiver type.
    pub fn semantic_trait_function_candidates_for_receiver(
        &self,
        def_map: &DefMapReadTxn<'db>,
        semantic_ir: &SemanticIrReadTxn<'db>,
        receiver_ty: &BodyNominalTy,
    ) -> Result<Vec<(FunctionRef, TraitApplicability)>, PackageStoreError> {
        resolution::semantic_trait_function_candidates_for_receiver(
            def_map,
            semantic_ir,
            receiver_ty,
        )
    }

    /// Checks whether a body-local function is a plausible method candidate for a receiver type.
    pub fn local_function_applies_to_receiver(
        &self,
        def_map: &DefMapReadTxn<'db>,
        semantic_ir: &SemanticIrReadTxn<'db>,
        function_ref: BodyFunctionRef,
        receiver_ty: &BodyLocalNominalTy,
    ) -> Result<bool, PackageStoreError> {
        let body = self.body_data(function_ref.body)?;
        resolution::local_function_applies_to_receiver(
            body,
            def_map,
            semantic_ir,
            function_ref,
            receiver_ty,
        )
    }

    /// Returns all body-local fields declared for a body-local type item.
    pub fn fields_for_local_type(
        &self,
        item_ref: BodyItemRef,
    ) -> Result<Vec<BodyFieldRef>, PackageStoreError> {
        let Some(body) = self.body_data(item_ref.body)? else {
            return Ok(Vec::new());
        };
        let Some(item) = body.local_item(item_ref.item) else {
            return Ok(Vec::new());
        };

        let fields = (0..item.fields.fields().len())
            .map(|index| BodyFieldRef {
                item: item_ref,
                index,
            })
            .collect();
        Ok(fields)
    }

    /// Returns declaration data for one body-local field.
    pub fn local_field_data(
        &self,
        field_ref: BodyFieldRef,
    ) -> Result<Option<BodyFieldData<'_>>, PackageStoreError> {
        let Some(body) = self.body_data(field_ref.item.body)? else {
            return Ok(None);
        };
        let Some(item) = body.local_item(field_ref.item.item) else {
            return Ok(None);
        };
        let Some(field) = item.field(field_ref.index) else {
            return Ok(None);
        };

        Ok(Some(BodyFieldData { item, field }))
    }

    /// Returns inherent body-local impl functions declared for a body-local type item.
    pub fn inherent_functions_for_local_type(
        &self,
        item_ref: BodyItemRef,
    ) -> Result<Vec<BodyFunctionRef>, PackageStoreError> {
        let Some(body) = self.body_data(item_ref.body)? else {
            return Ok(Vec::new());
        };

        Ok(body.inherent_functions_for_local_type(item_ref.body, item_ref))
    }

    /// Returns declaration data for one body-local function.
    pub fn local_function_data(
        &self,
        function_ref: BodyFunctionRef,
    ) -> Result<Option<&BodyFunctionData>, PackageStoreError> {
        Ok(self
            .body_data(function_ref.body)?
            .and_then(|body| body.local_function(function_ref.function)))
    }
}
