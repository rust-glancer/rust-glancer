//! Member data projections for editor-facing queries.
//!
//! `rg_ty::MemberQuery` returns stable refs. Completion, hover, and declaration details also need
//! borrowed item data, docs, display paths, and body-local method lookup. This view keeps that
//! cross-layer projection behind the view facade instead of exposing body-resolution internals to
//! analysis queries.

use rg_ir_model::Path;
use rg_ir_model::{
    BodyRef, CrateRef, EnumVariantRef, FieldKey, FieldRef, FunctionRef, ItemOwner, ScopeId,
    TraitApplicability, TypeDefId,
};
use rg_item_tree::{Documentation, ParamItem, ParamKind};
use rg_semantic_ir::{
    EnumVariantData, FieldData, FunctionData, ItemLookupIndex, ItemStoreQuery, TypePathResolution,
};
use rg_ty::{
    MemberMethodCandidateRef, MemberMethodOrigin as TyMemberMethodOrigin, MemberQuery, Ty,
    TyContext,
};

use crate::{
    IndexedViewDb, SymbolKind, body::BodyResolutionView, item::path::PathView, ty::IndexedType,
};

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

    pub(crate) fn data(&self) -> FieldData<'a> {
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

    /// Iterate parameters without exposing the item-tree storage shape.
    pub fn parameters(&self) -> impl ExactSizeIterator<Item = FunctionParameterView<'a>> + 'a {
        self.data
            .signature
            .params()
            .iter()
            .map(FunctionParameterView::new)
    }

    pub fn parameter(&self, index: usize) -> Option<FunctionParameterView<'a>> {
        self.data
            .signature
            .params()
            .get(index)
            .map(FunctionParameterView::new)
    }

    pub(crate) fn data(&self) -> &'a FunctionData {
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

/// Borrowed parameter facts needed by editor features.
///
/// Item lowering retains complete patterns and type syntax. Completion and inlay hints only need
/// the written pattern plus whether the parameter is a receiver, so the full item-tree node stays
/// behind this projection.
#[derive(Debug, Clone, Copy)]
pub struct FunctionParameterView<'a> {
    param: &'a ParamItem,
}

impl<'a> FunctionParameterView<'a> {
    fn new(param: &'a ParamItem) -> Self {
        Self { param }
    }

    pub fn pattern(self) -> &'a str {
        self.param.pat.as_str()
    }

    pub fn is_receiver(self) -> bool {
        matches!(self.param.kind, ParamKind::SelfParam(_))
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

/// Declaration source for a method candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberMethodOrigin {
    Inherent,
    Trait { applicability: TraitApplicability },
}

impl From<TyMemberMethodOrigin> for MemberMethodOrigin {
    fn from(origin: TyMemberMethodOrigin) -> Self {
        match origin {
            TyMemberMethodOrigin::Inherent => Self::Inherent,
            TyMemberMethodOrigin::Trait { applicability } => Self::Trait { applicability },
        }
    }
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
    Crate(CrateRef),
    Body(BodyRef),
}

impl MemberUseSite {
    pub fn krate(crate_ref: CrateRef) -> Self {
        Self::Crate(crate_ref)
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

    /// Return fields visible for a type at a crate use site.
    pub fn field_candidates_for_ty<'view>(
        &'view self,
        use_site: CrateRef,
        ty: &IndexedType,
    ) -> anyhow::Result<Vec<MemberField<'view>>> {
        let mut fields = Vec::new();
        let Some(semantic_index) = self.semantic_index(use_site)? else {
            return Ok(fields);
        };
        let member_query = MemberQuery::new(TyContext::new(
            self.db,
            self.db,
            semantic_index,
            self.db.trait_selection(use_site),
        ));
        for field_ref in member_query.fields_for_ty(ty.raw())? {
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
        let Some(semantic_index) = self.semantic_index(body.crate_ref)? else {
            return Ok(fields);
        };
        let member_query = MemberQuery::new(TyContext::new(
            self.db,
            self.db,
            semantic_index,
            self.db.trait_selection(body.crate_ref),
        ));
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

    /// Return methods visible for a type at a crate or body use site.
    pub fn method_candidates_for_ty<'view>(
        &'view self,
        use_site: MemberUseSite,
        ty: &IndexedType,
    ) -> anyhow::Result<Vec<MemberMethodCandidate<'view>>> {
        let candidates = self.method_candidate_refs_for_ty(use_site, ty.raw())?;
        self.method_candidates_from_refs(candidates)
    }

    /// Return method refs before loading borrowed function data.
    fn method_candidate_refs_for_ty(
        &self,
        use_site: MemberUseSite,
        ty: &Ty,
    ) -> anyhow::Result<Vec<MemberMethodCandidateRef>> {
        match use_site {
            MemberUseSite::Crate(crate_ref) => {
                self.crate_method_candidate_refs_for_ty(crate_ref, ty)
            }
            MemberUseSite::Body(body) => self.body_method_candidate_refs_for_ty(body, ty),
        }
    }

    /// Return crate-level method refs.
    fn crate_method_candidate_refs_for_ty(
        &self,
        use_site: CrateRef,
        ty: &Ty,
    ) -> anyhow::Result<Vec<MemberMethodCandidateRef>> {
        let Some(semantic_index) = self.semantic_index(use_site)? else {
            return Ok(Vec::new());
        };
        let member_query = MemberQuery::new(TyContext::new(
            self.db,
            self.db,
            semantic_index,
            self.db.trait_selection(use_site),
        ));
        Ok(member_query.method_candidates_for_ty(ty)?)
    }

    /// Return body-aware method refs, falling back to crate-level refs if the body is absent.
    fn body_method_candidate_refs_for_ty(
        &self,
        body: BodyRef,
        ty: &Ty,
    ) -> anyhow::Result<Vec<MemberMethodCandidateRef>> {
        let Some(candidates) =
            BodyResolutionView::new(self.db).method_candidate_refs_for_ty(body, ty)?
        else {
            // Missing body facts should not hide crate-level methods from editor queries.
            return self.crate_method_candidate_refs_for_ty(body.crate_ref, ty);
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
            origin: candidate.origin().into(),
        }
    }

    /// Return the crate-scoped semantic index that backs fast type/member queries.
    fn semantic_index(&self, use_site: CrateRef) -> anyhow::Result<Option<&ItemLookupIndex>> {
        Ok(self.db.body_ir.semantic_index(use_site)?)
    }
}
