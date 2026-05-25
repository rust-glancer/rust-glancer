//! Builds hover payloads from resolved analysis declarations.

use rg_body_ir::BodyTy;
use rg_def_map::TargetRef;
use rg_parse::{FileId, Span};

use crate::{
    api::{
        Analysis,
        query::type_at::TypeResolver,
        render::signature::SignatureRenderer,
        resolve::declaration::SymbolDeclarationResolver,
        view::details::{DeclarationDetails, DeclarationDetailsContext, DeclarationDetailsView},
    },
    model::{HoverBlock, HoverInfo, SymbolAt, SymbolKind},
};

pub(crate) struct HoverResolver<'a, 'db>(&'a Analysis<'db>);

impl<'a, 'db> HoverResolver<'a, 'db> {
    pub(crate) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self(analysis)
    }

    pub(crate) fn hover(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<HoverInfo>> {
        let Some(symbol) = self.0.symbol_at_for_query(target, file_id, offset)? else {
            return Ok(None);
        };
        let range = self.symbol_range(&symbol)?;
        let declarations =
            SymbolDeclarationResolver::new(self.0).declarations_for_symbol(symbol.clone())?;
        let context = DeclarationDetailsContext {
            module_display_name: Self::module_display_name_for_symbol(&symbol),
        };
        let details = DeclarationDetailsView::new(self.0);
        let mut blocks = Vec::new();

        for declaration in declarations {
            let Some(details) = details.details_for_declaration(declaration, &context)? else {
                continue;
            };
            let block = Self::hover_block(details);
            if !blocks.contains(&block) {
                blocks.push(block);
            }
        }

        if blocks.is_empty()
            && let Some(ty) = TypeResolver::new(self.0).type_at(target, file_id, offset)?
            && let Some(block) = self.hover_for_ty(&ty)?
        {
            blocks.push(block);
        }

        Ok((!blocks.is_empty()).then_some(HoverInfo { range, blocks }))
    }

    fn module_display_name_for_symbol(symbol: &SymbolAt) -> Option<String> {
        match symbol {
            SymbolAt::BodyPath { path, .. }
            | SymbolAt::BodyValuePath { path, .. }
            | SymbolAt::TypePath { path, .. }
            | SymbolAt::UsePath { path, .. } => path.last_segment_label(),
            SymbolAt::Body { .. }
            | SymbolAt::Binding { .. }
            | SymbolAt::Def { .. }
            | SymbolAt::Expr { .. }
            | SymbolAt::Field { .. }
            | SymbolAt::Function { .. }
            | SymbolAt::EnumVariant { .. }
            | SymbolAt::LocalEnumVariant { .. }
            | SymbolAt::LocalItem { .. }
            | SymbolAt::LocalValueItem { .. }
            | SymbolAt::LocalField { .. }
            | SymbolAt::LocalFunction { .. } => None,
        }
    }

    fn hover_for_ty(&self, ty: &BodyTy) -> anyhow::Result<Option<HoverBlock>> {
        let Some(signature) = SignatureRenderer::new(self.0).ty_signature(ty)? else {
            return Ok(None);
        };
        Ok(Some(HoverBlock {
            kind: SymbolKind::TypeAlias,
            path: None,
            signature: None,
            ty: Some(signature),
            docs: None,
        }))
    }

    fn hover_block(details: DeclarationDetails) -> HoverBlock {
        HoverBlock {
            kind: details.kind,
            path: details.path,
            signature: details.signature,
            ty: None,
            docs: details.docs,
        }
    }

    fn symbol_range(&self, symbol: &SymbolAt) -> anyhow::Result<Option<Span>> {
        match symbol {
            SymbolAt::Body { body } => Ok(self
                .0
                .body_ir
                .body_data(*body)?
                .map(|body_data| body_data.source().span)),
            SymbolAt::Binding { body, binding } => Ok(self
                .0
                .body_ir
                .body_data(*body)?
                .and_then(|body_data| body_data.binding(*binding))
                .map(|binding| binding.source.span)),
            SymbolAt::BodyPath { span, .. }
            | SymbolAt::BodyValuePath { span, .. }
            | SymbolAt::Def { span, .. }
            | SymbolAt::Field { span, .. }
            | SymbolAt::Function { span, .. }
            | SymbolAt::EnumVariant { span, .. }
            | SymbolAt::LocalItem { span, .. }
            | SymbolAt::LocalValueItem { span, .. }
            | SymbolAt::LocalField { span, .. }
            | SymbolAt::LocalEnumVariant { span, .. }
            | SymbolAt::LocalFunction { span, .. }
            | SymbolAt::TypePath { span, .. }
            | SymbolAt::UsePath { span, .. } => Ok(Some(*span)),
            SymbolAt::Expr { body, expr } => Ok(self
                .0
                .body_ir
                .body_data(*body)?
                .and_then(|body_data| body_data.expr(*expr))
                .map(|expr| expr.source.span)),
        }
    }
}
