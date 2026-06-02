//! Composite member view over nominal types.

use rg_body_ir::BodyScopeQuery;
use rg_ir_model::{
    BodyRef, FieldRef, FunctionRef, ItemOwner, ScopeId, TraitApplicability, TypeDefRef,
    TypePathResolution,
    hir::items::{FieldData, FunctionData},
};
use rg_ir_storage::{ItemStoreQuery, Path};
use rg_item_tree::{Documentation, FieldKey, ParamItem};
use rg_ty::{Autoderef, AutoderefMode, ImplMatcher, ItemPathQuery, NominalTy, Ty};

use crate::{IndexedViewDb, SymbolKind, item::declaration::Declaration, item::path::PathView};

/// A nominal receiver type with the generic arguments visible at the use site.
#[derive(Debug, Clone, Copy)]
pub enum MemberReceiverTy<'a> {
    Nominal(&'a NominalTy),
}

impl<'a> MemberReceiverTy<'a> {
    pub fn in_indexed_ty(ty: &'a Ty) -> impl Iterator<Item = Self> + 'a {
        ty.as_nominals().iter().map(Self::Nominal)
    }

    fn owner(self) -> MemberOwnerRef {
        match self {
            Self::Nominal(ty) => MemberOwnerRef::Nominal(ty.def),
        }
    }
}

/// Reference to a declaration owner whose fields can be enumerated without receiver generic args.
#[derive(Debug, Clone, Copy)]
pub enum MemberOwnerRef {
    Nominal(TypeDefRef),
}

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

#[derive(Debug, Clone, Copy)]
pub enum MemberMethodOrigin {
    Inherent,
    Trait { applicability: TraitApplicability },
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
        let autoderef = Autoderef::new(ItemPathQuery::new(self.db, self.db));
        let mut fields = Vec::new();

        for candidate in autoderef.candidates(AutoderefMode::FieldLookup, ty) {
            let candidate = candidate?;
            for receiver_ty in MemberReceiverTy::in_indexed_ty(candidate.ty()) {
                fields.extend(self.field_candidates(receiver_ty)?);
            }
        }

        Ok(fields)
    }

    pub fn field_candidates_for_body_type_path<'view>(
        &'view self,
        body: BodyRef,
        scope: ScopeId,
        path: &Path,
    ) -> anyhow::Result<Vec<MemberField<'view>>> {
        let mut fields = Vec::new();
        for owner in self.owners_for_body_type_path(body, scope, path)? {
            fields.extend(self.field_candidates_for_owner(owner)?);
        }
        Ok(fields)
    }

    fn field_candidates<'view>(
        &'view self,
        receiver_ty: MemberReceiverTy<'_>,
    ) -> anyhow::Result<Vec<MemberField<'view>>> {
        self.field_candidates_for_owner(receiver_ty.owner())
    }

    fn field_candidates_for_owner<'view>(
        &'view self,
        owner: MemberOwnerRef,
    ) -> anyhow::Result<Vec<MemberField<'view>>> {
        let field_refs: Vec<_> = match owner {
            MemberOwnerRef::Nominal(ty) => ItemStoreQuery::new(self.db).fields_for_type(ty)?,
        };

        let mut fields = Vec::new();
        for field_ref in field_refs {
            let Some(field) = self.field(field_ref)? else {
                continue;
            };
            fields.push(field);
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

    fn method_candidates<'view>(
        &'view self,
        receiver_ty: MemberReceiverTy<'_>,
    ) -> anyhow::Result<Vec<MemberMethodCandidate<'view>>> {
        let mut candidates = Vec::new();

        match receiver_ty {
            MemberReceiverTy::Nominal(ty) => {
                let item_paths = ItemPathQuery::new(self.db, self.db);
                let matcher = ImplMatcher::new(item_paths);

                for function in ItemStoreQuery::new(self.db).inherent_functions_for_type(ty.def)? {
                    if !matcher.function_applies_to_receiver(function, ty)? {
                        continue;
                    }

                    let Some(function) = self.function(function)? else {
                        continue;
                    };
                    candidates.push(MemberMethodCandidate {
                        function,
                        origin: MemberMethodOrigin::Inherent,
                    });
                }

                // Trait candidates carry applicability because this project intentionally avoids
                // full solving, but still wants useful editor suggestions for likely matches.
                for (function, applicability) in
                    matcher.trait_function_candidates_for_receiver(None, ty, None)?
                {
                    let Some(function) = self.function(function)? else {
                        continue;
                    };
                    candidates.push(MemberMethodCandidate {
                        function,
                        origin: MemberMethodOrigin::Trait { applicability },
                    });
                }
            }
        }

        Ok(candidates)
    }

    pub fn method_candidates_for_ty<'view>(
        &'view self,
        ty: &Ty,
    ) -> anyhow::Result<Vec<MemberMethodCandidate<'view>>> {
        let autoderef = Autoderef::new(ItemPathQuery::new(self.db, self.db));
        let mut methods = Vec::new();

        for candidate in autoderef.candidates(AutoderefMode::MethodReceiver, ty) {
            let candidate = candidate?;
            for receiver_ty in MemberReceiverTy::in_indexed_ty(candidate.ty()) {
                methods.extend(self.method_candidates(receiver_ty)?);
            }
        }

        Ok(methods)
    }

    fn owners_for_body_type_path(
        &self,
        body: BodyRef,
        scope: ScopeId,
        path: &Path,
    ) -> anyhow::Result<Vec<MemberOwnerRef>> {
        let Some(body_data) = self.db.body_ir.body_data(body)? else {
            return Ok(Vec::new());
        };
        let resolution = BodyScopeQuery::new(self.db, self.db, body, body_data)
            .resolve_type_path_in_scope(scope, path)?;
        let owners = match resolution {
            TypePathResolution::SelfType(types) | TypePathResolution::TypeDefs(types) => {
                types.into_iter().map(MemberOwnerRef::Nominal).collect()
            }
            TypePathResolution::TypeAliases(_)
            | TypePathResolution::Traits(_)
            | TypePathResolution::Unknown => Vec::new(),
        };
        Ok(owners)
    }
}
