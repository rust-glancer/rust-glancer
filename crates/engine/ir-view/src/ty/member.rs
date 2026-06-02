//! Composite member view over nominal types.

use rg_body_ir::BodyScopeQuery;
use rg_ir_model::{
    BodyRef, FieldRef, FunctionRef, ItemOwner, ScopeId, TypePathResolution,
    hir::items::{FieldData, FunctionData},
};
use rg_ir_storage::{ItemStoreQuery, Path};
use rg_item_tree::{Documentation, FieldKey, ParamItem};
pub use rg_ty::MemberMethodOrigin;
use rg_ty::{ItemPathQuery, MemberMethodCandidateRef, MemberQuery, Ty};

use crate::{IndexedViewDb, SymbolKind, item::declaration::Declaration, item::path::PathView};

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

    pub fn declaration(&self) -> Option<Declaration> {
        let key = self.key()?;
        Some(Declaration::new(
            self.field.owner.origin.origin_target(),
            SymbolKind::Field,
            key.declaration_label(),
            self.data.file_id,
            self.data.field.span,
            self.data.field.span,
        ))
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

    pub fn declaration(&self) -> Declaration {
        Declaration::new(
            self.function.origin.origin_target(),
            self.symbol_kind(),
            self.data.name.to_string(),
            self.data.source.file_id,
            self.data.span,
            self.data.name_span.unwrap_or(self.data.span),
        )
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

pub struct MemberView<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> MemberView<'a, 'db> {
    pub fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self { db }
    }

    pub fn field_candidates_for_ty<'view>(
        &'view self,
        ty: &Ty,
    ) -> anyhow::Result<Vec<MemberField<'view>>> {
        let mut fields = Vec::new();
        let member_query = MemberQuery::new(ItemPathQuery::new(self.db, self.db));
        for field_ref in member_query.fields_for_ty(ty)? {
            let Some(field) = self.field(field_ref)? else {
                continue;
            };
            fields.push(field);
        }
        Ok(fields)
    }

    pub fn field_candidates_for_body_type_path<'view>(
        &'view self,
        body: BodyRef,
        scope: ScopeId,
        path: &Path,
    ) -> anyhow::Result<Vec<MemberField<'view>>> {
        let Some(body_data) = self.db.body_ir.body_data(body)? else {
            return Ok(Vec::new());
        };
        let resolution = BodyScopeQuery::new(self.db, self.db, body, body_data)
            .resolve_type_path_in_scope(scope, path)?;

        let mut fields = Vec::new();
        let member_query = MemberQuery::new(ItemPathQuery::new(self.db, self.db));
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

    pub fn field(&self, field: FieldRef) -> anyhow::Result<Option<MemberField<'_>>> {
        Ok(ItemStoreQuery::new(self.db)
            .field_data(field)?
            .map(|data| MemberField { field, data }))
    }

    pub fn function(&self, function: FunctionRef) -> anyhow::Result<Option<MemberFunction<'_>>> {
        Ok(ItemStoreQuery::new(self.db)
            .function_data(function)?
            .map(|data| MemberFunction { function, data }))
    }

    pub fn method_candidates_for_ty<'view>(
        &'view self,
        ty: &Ty,
    ) -> anyhow::Result<Vec<MemberMethodCandidate<'view>>> {
        let mut methods = Vec::new();
        let member_query = MemberQuery::new(ItemPathQuery::new(self.db, self.db));
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
