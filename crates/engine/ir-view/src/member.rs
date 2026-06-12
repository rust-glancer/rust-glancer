//! Member data projections for editor-facing queries.
//!
//! `rg_ty::MemberQuery` returns stable refs. Completion, hover, and declaration details also need
//! borrowed item data, docs, display paths, and body-local method lookup. This view keeps that
//! cross-layer projection behind the view facade instead of exposing body-resolution internals to
//! analysis queries.

use rg_ir_model::Path;
use rg_ir_model::items::{Documentation, FieldKey, ParamItem};
use rg_ir_model::{
    BodyRef, EnumVariantRef, FieldRef, FunctionRef, ItemOwner, ScopeId, TargetRef, TypeDefId,
    TypePathResolution,
    hir::items::{EnumVariantData, FieldData, FunctionData},
};
use rg_ir_storage::{ItemStoreQuery, TargetItemQuery};
use rg_ty::MemberMethodOrigin;
use rg_ty::{ItemPathQuery, MemberMethodCandidateRef, MemberQuery, Ty};

use crate::{IndexedViewDb, SymbolKind, body::BodyResolutionView, item::path::PathView};

/// Borrowed data for one resolved field, independent from the storage layer it came from.
#[derive(Debug, Clone, Copy)]
pub struct MemberField<'a> {
    field: FieldRef,
    data: FieldData<'a>,
}

impl<'a> MemberField<'a> {
    pub fn field_ref(&self) -> FieldRef {
        self.field
    }

    pub fn key(&self) -> Option<&'a FieldKey> {
        self.data.field.key.as_ref()
    }

    pub fn data(&self) -> FieldData<'a> {
        self.data
    }

    pub fn display_path(&self, paths: &PathView<'_, '_>) -> anyhow::Result<Option<String>> {
        paths.type_def_path(self.field.owner)
    }

    pub fn docs_text(&self) -> Option<String> {
        self.docs().map(Documentation::text)
    }

    fn docs(&self) -> Option<&'a Documentation> {
        self.data.field.docs.as_ref()
    }
}

/// Borrowed data for one resolved function, independent from the storage layer it came from.
#[derive(Debug, Clone, Copy)]
pub struct MemberFunction<'a> {
    function: FunctionRef,
    data: &'a FunctionData,
}

impl<'a> MemberFunction<'a> {
    pub fn function_ref(&self) -> FunctionRef {
        self.function
    }

    pub fn name(&self) -> &'a str {
        self.data.name.as_str()
    }

    pub fn params(&self) -> &'a [ParamItem] {
        self.data.signature.params()
    }

    pub fn data(&self) -> &'a FunctionData {
        self.data
    }

    pub fn display_path(&self, paths: &PathView<'_, '_>) -> anyhow::Result<Option<String>> {
        paths.function_path(self.function)
    }

    pub fn symbol_kind(&self) -> SymbolKind {
        match self.data.owner {
            ItemOwner::Module(_) => SymbolKind::Function,
            ItemOwner::Trait(_) | ItemOwner::Impl(_) => SymbolKind::Method,
        }
    }

    pub fn docs_text(&self) -> Option<String> {
        self.docs().map(Documentation::text)
    }

    pub fn has_self_receiver(&self) -> bool {
        self.data.has_self_receiver()
    }

    fn docs(&self) -> Option<&'a Documentation> {
        self.data.docs.as_ref()
    }
}

/// Borrowed data for one resolved enum variant constructor.
#[derive(Debug, Clone, Copy)]
pub struct MemberEnumVariant<'a> {
    variant: EnumVariantRef,
    data: EnumVariantData<'a>,
}

impl<'a> MemberEnumVariant<'a> {
    pub fn variant_ref(&self) -> EnumVariantRef {
        self.variant
    }

    pub fn label(&self) -> &'a str {
        self.data.variant.name.as_str()
    }

    pub fn docs_text(&self) -> Option<String> {
        self.data.variant.docs.as_ref().map(Documentation::text)
    }
}

/// One method candidate with enough origin information for UI ranking and labels.
#[derive(Debug, Clone, Copy)]
pub struct MemberMethodCandidate<'a> {
    function: MemberFunction<'a>,
    origin: MemberMethodOrigin,
}

impl<'a> MemberMethodCandidate<'a> {
    pub fn function(&self) -> MemberFunction<'a> {
        self.function
    }

    pub fn origin(&self) -> MemberMethodOrigin {
        self.origin
    }
}

/// Place where member lookup is requested.
#[derive(Debug, Clone, Copy)]
pub enum MemberUseSite {
    Target(TargetRef),
    Body(BodyRef),
}

impl MemberUseSite {
    pub fn target(target: TargetRef) -> Self {
        Self::Target(target)
    }

    pub fn body(body: BodyRef) -> Self {
        Self::Body(body)
    }
}

/// Projects member refs into field, function, and method view data.
pub struct MemberView<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> MemberView<'a, 'db> {
    pub fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self { db }
    }

    /// Return fields visible for a type at a target use site.
    pub fn field_candidates_for_ty<'view>(
        &'view self,
        use_site: TargetRef,
        ty: &Ty,
    ) -> anyhow::Result<Vec<MemberField<'view>>> {
        let mut fields = Vec::new();
        let member_query = MemberQuery::new(
            ItemPathQuery::new(self.db, self.db),
            TargetItemQuery::new(self.db, self.db, use_site),
        );
        for field_ref in member_query.fields_for_ty(ty)? {
            let Some(field) = self.field(field_ref)? else {
                continue;
            };
            fields.push(field);
        }
        Ok(fields)
    }

    /// Resolve a body type path and return its declared fields.
    pub fn field_candidates_for_body_type_path<'view>(
        &'view self,
        body: BodyRef,
        scope: ScopeId,
        path: &Path,
    ) -> anyhow::Result<Vec<MemberField<'view>>> {
        let Some(resolution) =
            BodyResolutionView::new(self.db).type_path_resolution(body, scope, path)?
        else {
            return Ok(Vec::new());
        };

        let mut fields = Vec::new();
        let member_query = MemberQuery::new(
            ItemPathQuery::new(self.db, self.db),
            TargetItemQuery::new(self.db, self.db, body.target),
        );
        if let TypePathResolution::SelfType(ty) | TypePathResolution::TypeDef(ty) = resolution {
            for field_ref in member_query.fields_for_type_def(ty)? {
                let Some(field) = self.field(field_ref)? else {
                    continue;
                };
                fields.push(field);
            }
        }

        Ok(fields)
    }

    /// Return borrowed data for one field.
    pub fn field(&self, field: FieldRef) -> anyhow::Result<Option<MemberField<'_>>> {
        Ok(ItemStoreQuery::new(self.db)
            .field_data(field)?
            .map(|data| MemberField { field, data }))
    }

    /// Return borrowed data for one function.
    pub fn function(&self, function: FunctionRef) -> anyhow::Result<Option<MemberFunction<'_>>> {
        Ok(ItemStoreQuery::new(self.db)
            .function_data(function)?
            .map(|data| MemberFunction { function, data }))
    }

    /// Return borrowed data for one enum variant.
    pub fn enum_variant(
        &self,
        variant: EnumVariantRef,
    ) -> anyhow::Result<Option<MemberEnumVariant<'_>>> {
        Ok(ItemStoreQuery::new(self.db)
            .enum_variant_data(variant)?
            .map(|data| MemberEnumVariant { variant, data }))
    }

    /// Resolve a body type path and return its enum variants.
    pub fn enum_variant_candidates_for_body_type_path(
        &self,
        body: BodyRef,
        scope: ScopeId,
        path: &Path,
    ) -> anyhow::Result<Vec<EnumVariantRef>> {
        let Some(resolution) =
            BodyResolutionView::new(self.db).type_path_resolution(body, scope, path)?
        else {
            return Ok(Vec::new());
        };
        let (TypePathResolution::SelfType(ty) | TypePathResolution::TypeDef(ty)) = resolution
        else {
            return Ok(Vec::new());
        };

        let item_query = ItemStoreQuery::new(self.db);
        let mut variants = Vec::new();
        let TypeDefId::Enum(enum_id) = ty.id else {
            return Ok(Vec::new());
        };
        let Some(data) = item_query.enum_data_for_type_def(ty)? else {
            return Ok(Vec::new());
        };
        variants.extend((0..data.variants.len()).map(|index| EnumVariantRef {
            origin: ty.origin,
            enum_id,
            index,
        }));
        Ok(variants)
    }

    /// Return methods visible for a type at a target or body use site.
    pub fn method_candidates_for_ty<'view>(
        &'view self,
        use_site: MemberUseSite,
        ty: &Ty,
    ) -> anyhow::Result<Vec<MemberMethodCandidate<'view>>> {
        let candidates = self.method_candidate_refs_for_ty(use_site, ty)?;
        self.method_candidates_from_refs(candidates)
    }

    /// Return method refs before loading borrowed function data.
    fn method_candidate_refs_for_ty(
        &self,
        use_site: MemberUseSite,
        ty: &Ty,
    ) -> anyhow::Result<Vec<MemberMethodCandidateRef>> {
        match use_site {
            MemberUseSite::Target(target) => self.target_method_candidate_refs_for_ty(target, ty),
            MemberUseSite::Body(body) => self.body_method_candidate_refs_for_ty(body, ty),
        }
    }

    /// Return target-level method refs.
    fn target_method_candidate_refs_for_ty(
        &self,
        use_site: TargetRef,
        ty: &Ty,
    ) -> anyhow::Result<Vec<MemberMethodCandidateRef>> {
        let member_query = MemberQuery::new(
            ItemPathQuery::new(self.db, self.db),
            TargetItemQuery::new(self.db, self.db, use_site),
        );
        Ok(member_query.method_candidates_for_ty(ty)?)
    }

    /// Return body-aware method refs, falling back to target-level refs if the body is absent.
    fn body_method_candidate_refs_for_ty(
        &self,
        body: BodyRef,
        ty: &Ty,
    ) -> anyhow::Result<Vec<MemberMethodCandidateRef>> {
        let Some(candidates) =
            BodyResolutionView::new(self.db).method_candidate_refs_for_ty(body, ty)?
        else {
            // Missing body facts should not hide target-level methods from editor queries.
            return self.target_method_candidate_refs_for_ty(body.target, ty);
        };

        Ok(candidates)
    }

    /// Load function data for method refs and keep candidates whose functions still exist.
    fn method_candidates_from_refs<'view>(
        &'view self,
        candidates: Vec<MemberMethodCandidateRef>,
    ) -> anyhow::Result<Vec<MemberMethodCandidate<'view>>> {
        let mut methods = Vec::new();
        for candidate in candidates {
            let Some(function) = self.function(candidate.function())? else {
                continue;
            };
            methods.push(Self::method_candidate(function, candidate));
        }

        Ok(methods)
    }

    /// Combine borrowed function data with lookup origin.
    fn method_candidate<'view>(
        function: MemberFunction<'view>,
        candidate: MemberMethodCandidateRef,
    ) -> MemberMethodCandidate<'view> {
        MemberMethodCandidate {
            function,
            origin: candidate.origin(),
        }
    }
}
