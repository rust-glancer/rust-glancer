//! Member data adapter for editor-facing queries.
//!
//! `rg_ty::MemberQuery` returns stable refs. Completion and hover still need borrowed item data,
//! docs, and display paths, so this module keeps that projection close to the analysis features.

use rg_body_ir::BodyScopeQuery;
use rg_ir_model::items::{Documentation, FieldKey, ParamItem};
use rg_ir_model::{
    BodyRef, FieldRef, FunctionRef, ItemOwner, ScopeId, TargetRef, TypePathResolution,
    hir::items::{FieldData, FunctionData},
};
use rg_ir_storage::{ItemStoreQuery, Path, TargetItemQuery};
use rg_ir_view::{IndexedViewDb, item::path::PathView};
pub(crate) use rg_ty::MemberMethodOrigin;
use rg_ty::{ItemPathQuery, MemberMethodCandidateRef, MemberQuery, Ty};

use crate::SymbolKind;

/// Borrowed data for one resolved field, independent from the storage layer it came from.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MemberField<'a> {
    field: FieldRef,
    data: FieldData<'a>,
}

impl<'a> MemberField<'a> {
    pub(crate) fn field_ref(&self) -> FieldRef {
        self.field
    }

    pub(crate) fn key(&self) -> Option<&'a FieldKey> {
        self.data.field.key.as_ref()
    }

    pub(crate) fn data(&self) -> FieldData<'a> {
        self.data
    }

    pub(crate) fn display_path(&self, paths: &PathView<'_, '_>) -> anyhow::Result<Option<String>> {
        paths.type_def_path(self.field.owner)
    }

    pub(crate) fn docs_text(&self) -> Option<String> {
        self.docs().map(Documentation::text)
    }

    fn docs(&self) -> Option<&'a Documentation> {
        self.data.field.docs.as_ref()
    }
}

/// Borrowed data for one resolved function, independent from the storage layer it came from.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MemberFunction<'a> {
    function: FunctionRef,
    data: &'a FunctionData,
}

impl<'a> MemberFunction<'a> {
    pub(crate) fn function_ref(&self) -> FunctionRef {
        self.function
    }

    pub(crate) fn name(&self) -> &'a str {
        self.data.name.as_str()
    }

    pub(crate) fn params(&self) -> &'a [ParamItem] {
        self.data.signature.params()
    }

    pub(crate) fn data(&self) -> &'a FunctionData {
        self.data
    }

    pub(crate) fn display_path(&self, paths: &PathView<'_, '_>) -> anyhow::Result<Option<String>> {
        paths.function_path(self.function)
    }

    pub(crate) fn symbol_kind(&self) -> SymbolKind {
        match self.data.owner {
            ItemOwner::Module(_) => SymbolKind::Function,
            ItemOwner::Trait(_) | ItemOwner::Impl(_) => SymbolKind::Method,
        }
    }

    pub(crate) fn docs_text(&self) -> Option<String> {
        self.docs().map(Documentation::text)
    }

    pub(crate) fn has_self_receiver(&self) -> bool {
        self.data.has_self_receiver()
    }

    fn docs(&self) -> Option<&'a Documentation> {
        self.data.docs.as_ref()
    }
}

/// One method candidate with enough origin information for UI ranking and labels.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MemberMethodCandidate<'a> {
    function: MemberFunction<'a>,
    origin: MemberMethodOrigin,
}

impl<'a> MemberMethodCandidate<'a> {
    pub(crate) fn function(&self) -> MemberFunction<'a> {
        self.function
    }

    pub(crate) fn origin(&self) -> MemberMethodOrigin {
        self.origin
    }
}

pub(crate) struct MemberView<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> MemberView<'a, 'db> {
    pub(crate) fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self { db }
    }

    pub(crate) fn field_candidates_for_ty<'view>(
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

    pub(crate) fn field_candidates_for_body_type_path<'view>(
        &'view self,
        body: BodyRef,
        scope: ScopeId,
        path: &Path,
    ) -> anyhow::Result<Vec<MemberField<'view>>> {
        let Some(body_data) = self.db.body_data(body)? else {
            return Ok(Vec::new());
        };
        let resolution = BodyScopeQuery::new(self.db, self.db, body, body_data)
            .resolve_type_path_in_scope(scope, path)?;

        let mut fields = Vec::new();
        let member_query = MemberQuery::new(
            ItemPathQuery::new(self.db, self.db),
            TargetItemQuery::new(self.db, self.db, body.target),
        );
        if let TypePathResolution::SelfType(types) | TypePathResolution::TypeDefs(types) =
            resolution
        {
            for ty in types {
                for field_ref in member_query.fields_for_type_def(ty)? {
                    let Some(field) = self.field(field_ref)? else {
                        continue;
                    };
                    fields.push(field);
                }
            }
        }

        Ok(fields)
    }

    pub(crate) fn field(&self, field: FieldRef) -> anyhow::Result<Option<MemberField<'_>>> {
        Ok(ItemStoreQuery::new(self.db)
            .field_data(field)?
            .map(|data| MemberField { field, data }))
    }

    pub(crate) fn function(
        &self,
        function: FunctionRef,
    ) -> anyhow::Result<Option<MemberFunction<'_>>> {
        Ok(ItemStoreQuery::new(self.db)
            .function_data(function)?
            .map(|data| MemberFunction { function, data }))
    }

    pub(crate) fn method_candidates_for_ty<'view>(
        &'view self,
        use_site: TargetRef,
        ty: &Ty,
    ) -> anyhow::Result<Vec<MemberMethodCandidate<'view>>> {
        let mut methods = Vec::new();
        let member_query = MemberQuery::new(
            ItemPathQuery::new(self.db, self.db),
            TargetItemQuery::new(self.db, self.db, use_site),
        );
        for candidate in member_query.method_candidates_for_ty(ty)? {
            let Some(function) = self.function(candidate.function())? else {
                continue;
            };
            methods.push(Self::method_candidate(function, candidate));
        }

        Ok(methods)
    }

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
