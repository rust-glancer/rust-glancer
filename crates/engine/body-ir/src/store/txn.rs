//! Read transactions over frozen Body IR package data.

use rg_def_map::{DefMapReadTxn, PackageSlot, Path};
use rg_ir_model::{
    BodyRef, DefMapRef, FieldRef, FunctionRef, ScopeId, TargetRef, TraitApplicability,
    TraitImplRef, TypePathResolution,
};
use rg_package_store::{PackageStoreError, PackageStoreReadTxn};
use rg_semantic_ir::{ItemPathQuery, ItemStoreQuery, SemanticIrReadTxn, TypePathContext};
use rg_ty::{NominalTy, Ty, TypeSubst};

use crate::{
    BodyData, BodyResolution, PackageBodies, TargetBodies,
    resolution::{
        self, BodyImplMatcher, BodyQuerySource, BodyTypePathResolver, BodyValuePathResolver,
        ty_from_type_ref_in_context,
    },
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

    /// Checks whether a semantic function is a plausible method candidate for a receiver type.
    pub fn semantic_function_applies_to_receiver(
        &self,
        def_map: &DefMapReadTxn<'db>,
        semantic_ir: &SemanticIrReadTxn<'db>,
        function_ref: FunctionRef,
        receiver_ty: &NominalTy,
    ) -> Result<bool, PackageStoreError> {
        if let DefMapRef::Body(body_ref) = function_ref.origin {
            let Some(body) = self.body_data(body_ref)? else {
                return Ok(false);
            };
            let source = BodyQuerySource::new(def_map, semantic_ir, body_ref, body);
            return resolution::function_applies_to_receiver(
                ItemPathQuery::new(source, source),
                function_ref,
                receiver_ty,
            );
        }

        resolution::function_applies_to_receiver(
            ItemPathQuery::new(def_map, semantic_ir),
            function_ref,
            receiver_ty,
        )
    }

    /// Returns trait-associated function candidates for a semantic receiver type.
    pub fn semantic_trait_function_candidates_for_receiver(
        &self,
        def_map: &DefMapReadTxn<'db>,
        semantic_ir: &SemanticIrReadTxn<'db>,
        receiver_ty: &NominalTy,
    ) -> Result<Vec<(FunctionRef, TraitApplicability)>, PackageStoreError> {
        if let DefMapRef::Body(body_ref) = receiver_ty.def.origin {
            let Some(body) = self.body_data(body_ref)? else {
                return Ok(Vec::new());
            };
            let source = BodyQuerySource::new(def_map, semantic_ir, body_ref, body);
            return resolution::trait_function_candidates_for_receiver(
                None,
                ItemPathQuery::new(source, source),
                receiver_ty,
                None,
            );
        }

        resolution::trait_function_candidates_for_receiver(
            None,
            ItemPathQuery::new(def_map, semantic_ir),
            receiver_ty,
            None,
        )
    }

    /// Checks whether a semantic trait impl is a plausible candidate for a receiver type.
    pub fn semantic_trait_impl_applies_to_receiver(
        &self,
        def_map: &DefMapReadTxn<'db>,
        semantic_ir: &SemanticIrReadTxn<'db>,
        trait_impl: TraitImplRef,
        receiver_ty: &NominalTy,
    ) -> Result<bool, PackageStoreError> {
        if let DefMapRef::Body(body_ref) = trait_impl.impl_ref.origin {
            let Some(body) = self.body_data(body_ref)? else {
                return Ok(false);
            };
            let source = BodyQuerySource::new(def_map, semantic_ir, body_ref, body);
            return Ok(BodyImplMatcher::new(ItemPathQuery::new(source, source))
                .semantic_trait_impl_applicability(trait_impl, receiver_ty)?
                .is_applicable());
        }

        Ok(
            BodyImplMatcher::new(ItemPathQuery::new(def_map, semantic_ir))
                .semantic_trait_impl_applicability(trait_impl, receiver_ty)?
                .is_applicable(),
        )
    }
}
