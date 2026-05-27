//! Composite member view over semantic and body-local nominal types.

use rg_body_ir::{
    BodyAutoderef, BodyAutoderefMode, BodyFieldData, BodyFunctionData, BodyTypePathResolution,
};
use rg_def_map::Path;
use rg_ir_model::{
    BodyFieldRef, BodyFunctionRef, BodyItemRef, BodyRef, FieldRef as SemanticFieldRef,
    FunctionRef as SemanticFunctionRef, ItemOwner, ScopeId, TraitApplicability, TypeDefRef,
    identity::{FieldRef, FieldRefRepr, FunctionRef, FunctionRefRepr},
};
use rg_semantic_ir::{Documentation, FieldData, FieldKey, FunctionData, ParamItem};
use rg_ty::{IndexedLocalNominalTy, IndexedNominalTy, IndexedTy, IndexedTyExt};

use crate::{IndexedViewDb, SymbolKind, item::declaration::Declaration, item::path::PathView};

/// A nominal receiver type whose declarations may live in either Semantic IR or Body IR.
#[derive(Debug, Clone, Copy)]
pub enum MemberReceiverTy<'a> {
    Semantic(&'a IndexedNominalTy),
    BodyLocal(&'a IndexedLocalNominalTy),
}

impl<'a> MemberReceiverTy<'a> {
    pub fn in_indexed_ty(ty: &'a IndexedTy) -> impl Iterator<Item = Self> + 'a {
        ty.as_local_nominals()
            .iter()
            .map(Self::BodyLocal)
            .chain(ty.as_nominals().iter().map(Self::Semantic))
    }

    fn owner(self) -> MemberOwnerRef {
        match self {
            Self::Semantic(ty) => MemberOwnerRef::Semantic(ty.def),
            Self::BodyLocal(ty) => MemberOwnerRef::BodyLocal(ty.item),
        }
    }
}

/// Reference to a declaration owner whose fields can be enumerated without receiver generic args.
#[derive(Debug, Clone, Copy)]
pub enum MemberOwnerRef {
    Semantic(TypeDefRef),
    BodyLocal(BodyItemRef),
}

/// Borrowed data for one resolved field, independent from the storage layer it came from.
#[derive(Debug, Clone, Copy)]
pub enum MemberField<'a> {
    Semantic {
        field: SemanticFieldRef,
        data: FieldData<'a>,
    },
    BodyLocal {
        field: BodyFieldRef,
        data: BodyFieldData<'a>,
    },
}

impl<'a> MemberField<'a> {
    pub fn field_ref(&self) -> FieldRef {
        match self {
            Self::Semantic { field, .. } => FieldRef::semantic(*field),
            Self::BodyLocal { field, .. } => FieldRef::body_local(*field),
        }
    }

    pub fn key(&self) -> Option<&'a FieldKey> {
        match self {
            Self::Semantic { data, .. } => data.field.key.as_ref(),
            Self::BodyLocal { data, .. } => data.field.key.as_ref(),
        }
    }

    pub fn display_path(&self, paths: &PathView<'_, '_>) -> anyhow::Result<Option<String>> {
        match self {
            Self::Semantic { field, .. } => paths.type_def_path(field.owner),
            Self::BodyLocal { .. } => Ok(None),
        }
    }

    pub fn declaration(&self) -> Option<Declaration> {
        let key = self.key()?;
        Some(match self {
            Self::Semantic { field, data } => Declaration::new(
                field.owner.target,
                SymbolKind::Field,
                key.declaration_label(),
                data.file_id,
                data.field.span,
                data.field.span,
            ),
            Self::BodyLocal { field, data } => Declaration::new(
                field.item.body.target,
                SymbolKind::Field,
                key.declaration_label(),
                data.item.source.file_id,
                data.field.span,
                data.field.span,
            ),
        })
    }

    pub fn docs_text(&self) -> Option<String> {
        self.docs().map(Documentation::text)
    }

    fn docs(&self) -> Option<&'a Documentation> {
        match self {
            Self::Semantic { data, .. } => data.field.docs.as_ref(),
            Self::BodyLocal { data, .. } => data.field.docs.as_ref(),
        }
    }
}

/// Borrowed data for one resolved function, independent from the storage layer it came from.
#[derive(Debug, Clone, Copy)]
pub enum MemberFunction<'a> {
    Semantic {
        function: SemanticFunctionRef,
        data: &'a FunctionData,
    },
    BodyLocal {
        function: BodyFunctionRef,
        data: &'a BodyFunctionData,
    },
}

impl<'a> MemberFunction<'a> {
    pub fn function_ref(&self) -> FunctionRef {
        match self {
            Self::Semantic { function, .. } => FunctionRef::semantic(*function),
            Self::BodyLocal { function, .. } => FunctionRef::body_local(*function),
        }
    }

    pub fn name(&self) -> &'a str {
        match self {
            Self::Semantic { data, .. } => data.name.as_str(),
            Self::BodyLocal { data, .. } => data.name.as_str(),
        }
    }

    pub fn params(&self) -> &'a [ParamItem] {
        match self {
            Self::Semantic { data, .. } => data.signature.params(),
            Self::BodyLocal { data, .. } => &data.declaration.params,
        }
    }

    pub fn display_path(&self, paths: &PathView<'_, '_>) -> anyhow::Result<Option<String>> {
        match self {
            Self::Semantic { function, .. } => paths.function_path(*function),
            Self::BodyLocal { .. } => Ok(None),
        }
    }

    pub fn symbol_kind(&self) -> SymbolKind {
        match self {
            Self::Semantic { data, .. } => match data.owner {
                ItemOwner::Module(_) => SymbolKind::Function,
                ItemOwner::Trait(_) | ItemOwner::Impl(_) => SymbolKind::Method,
            },
            Self::BodyLocal { data, .. } => SymbolKind::from_body_function_owner(data.owner),
        }
    }

    pub fn declaration(&self) -> Declaration {
        match self {
            Self::Semantic { function, data } => Declaration::new(
                function.target,
                self.symbol_kind(),
                data.name.to_string(),
                data.source.file_id,
                data.span,
                data.name_span.unwrap_or(data.span),
            ),
            Self::BodyLocal { function, data } => Declaration::new(
                function.body.target,
                self.symbol_kind(),
                data.name.to_string(),
                data.source.file_id,
                data.source.span,
                data.name_source.span,
            ),
        }
    }

    pub fn docs_text(&self) -> Option<String> {
        self.docs().map(Documentation::text)
    }

    pub fn has_self_receiver(&self) -> bool {
        match self {
            Self::Semantic { data, .. } => data.has_self_receiver(),
            Self::BodyLocal { data, .. } => data.has_self_receiver(),
        }
    }

    fn docs(&self) -> Option<&'a Documentation> {
        match self {
            Self::Semantic { data, .. } => data.docs.as_ref(),
            Self::BodyLocal { data, .. } => data.docs.as_ref(),
        }
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
    analysis: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> MemberView<'a, 'db> {
    pub fn new(analysis: &'a IndexedViewDb<'db>) -> Self {
        Self { analysis }
    }

    pub fn field_candidates_for_ty<'view>(
        &'view self,
        ty: &IndexedTy,
    ) -> anyhow::Result<Vec<MemberField<'view>>> {
        let autoderef = BodyAutoderef::new(&self.analysis.def_map, &self.analysis.semantic_ir);
        let mut fields = Vec::new();

        for candidate in autoderef.candidates(BodyAutoderefMode::FieldLookup, ty) {
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

    pub fn field_candidates<'view>(
        &'view self,
        receiver_ty: MemberReceiverTy<'_>,
    ) -> anyhow::Result<Vec<MemberField<'view>>> {
        self.field_candidates_for_owner(receiver_ty.owner())
    }

    pub fn field_candidates_for_owner<'view>(
        &'view self,
        owner: MemberOwnerRef,
    ) -> anyhow::Result<Vec<MemberField<'view>>> {
        let field_refs: Vec<_> = match owner {
            MemberOwnerRef::Semantic(ty) => self
                .analysis
                .semantic_ir
                .fields_for_type(ty)?
                .into_iter()
                .map(FieldRef::semantic)
                .collect(),
            MemberOwnerRef::BodyLocal(item) => self
                .analysis
                .body_ir
                .fields_for_local_type(item)?
                .into_iter()
                .map(FieldRef::body_local)
                .collect(),
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
        match field.repr() {
            FieldRefRepr::Semantic(field) => Ok(self
                .analysis
                .semantic_ir
                .field_data(field)?
                .map(|data| MemberField::Semantic { field, data })),
            FieldRefRepr::BodyLocal(field) => Ok(self
                .analysis
                .body_ir
                .local_field_data(field)?
                .map(|data| MemberField::BodyLocal { field, data })),
        }
    }

    pub fn function(&self, function: FunctionRef) -> anyhow::Result<Option<MemberFunction<'_>>> {
        match function.repr() {
            FunctionRefRepr::Semantic(function) => Ok(self
                .analysis
                .semantic_ir
                .function_data(function)?
                .map(|data| MemberFunction::Semantic { function, data })),
            FunctionRefRepr::BodyLocal(function) => Ok(self
                .analysis
                .body_ir
                .local_function_data(function)?
                .map(|data| MemberFunction::BodyLocal { function, data })),
        }
    }

    pub fn method_candidates<'view>(
        &'view self,
        receiver_ty: MemberReceiverTy<'_>,
    ) -> anyhow::Result<Vec<MemberMethodCandidate<'view>>> {
        let mut candidates = Vec::new();

        match receiver_ty {
            MemberReceiverTy::Semantic(ty) => {
                for function in self
                    .analysis
                    .semantic_ir
                    .inherent_functions_for_type(ty.def)?
                {
                    if !self
                        .analysis
                        .body_ir
                        .semantic_function_applies_to_receiver(
                            &self.analysis.def_map,
                            &self.analysis.semantic_ir,
                            function,
                            ty,
                        )?
                    {
                        continue;
                    }

                    let Some(function) = self.function(FunctionRef::semantic(function))? else {
                        continue;
                    };
                    candidates.push(MemberMethodCandidate {
                        function,
                        origin: MemberMethodOrigin::Inherent,
                    });
                }

                // Trait candidates carry applicability because this project intentionally avoids
                // full solving, but still wants useful editor suggestions for likely matches.
                for (function, applicability) in self
                    .analysis
                    .body_ir
                    .semantic_trait_function_candidates_for_receiver(
                        &self.analysis.def_map,
                        &self.analysis.semantic_ir,
                        ty,
                    )?
                {
                    let Some(function) = self.function(FunctionRef::semantic(function))? else {
                        continue;
                    };
                    candidates.push(MemberMethodCandidate {
                        function,
                        origin: MemberMethodOrigin::Trait { applicability },
                    });
                }
            }
            MemberReceiverTy::BodyLocal(ty) => {
                for function in self
                    .analysis
                    .body_ir
                    .inherent_functions_for_local_type(ty.item)?
                {
                    if !self.analysis.body_ir.local_function_applies_to_receiver(
                        &self.analysis.def_map,
                        &self.analysis.semantic_ir,
                        function,
                        ty,
                    )? {
                        continue;
                    }

                    let Some(function) = self.function(FunctionRef::body_local(function))? else {
                        continue;
                    };
                    candidates.push(MemberMethodCandidate {
                        function,
                        origin: MemberMethodOrigin::Inherent,
                    });
                }
            }
        }

        Ok(candidates)
    }

    pub fn method_candidates_for_ty<'view>(
        &'view self,
        ty: &IndexedTy,
    ) -> anyhow::Result<Vec<MemberMethodCandidate<'view>>> {
        let autoderef = BodyAutoderef::new(&self.analysis.def_map, &self.analysis.semantic_ir);
        let mut methods = Vec::new();

        for candidate in autoderef.candidates(BodyAutoderefMode::MethodReceiver, ty) {
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
        let resolution = self.analysis.body_ir.resolve_type_path_in_scope(
            &self.analysis.def_map,
            &self.analysis.semantic_ir,
            body,
            scope,
            path,
        )?;
        let owners = match resolution {
            BodyTypePathResolution::BodyLocal(item) => vec![MemberOwnerRef::BodyLocal(item)],
            BodyTypePathResolution::SelfType(types) | BodyTypePathResolution::TypeDefs(types) => {
                types.into_iter().map(MemberOwnerRef::Semantic).collect()
            }
            BodyTypePathResolution::Primitive(_)
            | BodyTypePathResolution::Traits(_)
            | BodyTypePathResolution::Unknown => Vec::new(),
        };
        Ok(owners)
    }
}
