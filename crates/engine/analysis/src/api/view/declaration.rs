//! Source-level declaration lookup shared by editor queries.

use rg_body_ir::{BodyItemRef, BodyValueItemRef, ResolvedFieldRef, ResolvedFunctionRef};
use rg_def_map::LocalDefRef;

use crate::{
    api::{Analysis, view::member::MemberLookup},
    model::{Declaration, SymbolKind},
};

/// Storage-independent identity for declarations that editor features can project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, derive_more::From)]
pub(crate) enum DeclarationRef {
    LocalDef(LocalDefRef),
    BodyItem(BodyItemRef),
    BodyValueItem(BodyValueItemRef),
    Field(ResolvedFieldRef),
    Function(ResolvedFunctionRef),
}

/// Reads declaration facts for IDs that already identify one source declaration.
pub(crate) struct DeclarationLookup<'a, 'db> {
    analysis: &'a Analysis<'db>,
}

impl<'a, 'db> DeclarationLookup<'a, 'db> {
    pub(crate) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self { analysis }
    }

    pub(crate) fn declaration(
        &self,
        declaration: DeclarationRef,
    ) -> anyhow::Result<Option<Declaration>> {
        match declaration {
            DeclarationRef::LocalDef(local_def) => self.local_def(local_def),
            DeclarationRef::BodyItem(item_ref) => self.body_item(item_ref),
            DeclarationRef::BodyValueItem(item_ref) => self.body_value_item(item_ref),
            DeclarationRef::Field(field) => self.field(field),
            DeclarationRef::Function(function) => self.function(function),
        }
    }

    fn local_def(&self, local_def: LocalDefRef) -> anyhow::Result<Option<Declaration>> {
        let Some(data) = self.analysis.def_map.local_def(local_def)? else {
            return Ok(None);
        };

        Ok(Some(Declaration {
            target: local_def.target,
            kind: SymbolKind::from_local_def_kind(data.kind),
            name: data.name.to_string(),
            file_id: data.file_id,
            span: data.span,
            selection_span: data.name_span.unwrap_or(data.span),
        }))
    }

    fn body_item(&self, item_ref: BodyItemRef) -> anyhow::Result<Option<Declaration>> {
        let Some(body_data) = self.analysis.body_ir.body_data(item_ref.body)? else {
            return Ok(None);
        };
        let Some(item) = body_data.local_item(item_ref.item) else {
            return Ok(None);
        };

        Ok(Some(Declaration {
            target: item_ref.body.target,
            kind: SymbolKind::from_body_item_kind(item.kind),
            name: item.name.to_string(),
            file_id: item.source.file_id,
            span: item.source.span,
            selection_span: item.name_source.span,
        }))
    }

    fn body_value_item(&self, item_ref: BodyValueItemRef) -> anyhow::Result<Option<Declaration>> {
        let Some(body_data) = self.analysis.body_ir.body_data(item_ref.body)? else {
            return Ok(None);
        };
        let Some(item) = body_data.local_value_item(item_ref.item) else {
            return Ok(None);
        };

        Ok(Some(Declaration {
            target: item_ref.body.target,
            kind: SymbolKind::from_body_value_item_kind(item.kind),
            name: item.name.to_string(),
            file_id: item.source.file_id,
            span: item.source.span,
            selection_span: item.name_source.span,
        }))
    }

    fn field(&self, field: ResolvedFieldRef) -> anyhow::Result<Option<Declaration>> {
        Ok(MemberLookup::new(self.analysis)
            .field_view(field)?
            .and_then(|field| field.declaration()))
    }

    fn function(&self, function: ResolvedFunctionRef) -> anyhow::Result<Option<Declaration>> {
        Ok(MemberLookup::new(self.analysis)
            .function_view(function)?
            .map(|function| function.declaration()))
    }
}
