//! Builds hover payloads from resolved analysis declarations.

use rg_ir_model::TargetRef;
use rg_ir_view::display::ty_label::TypeRenderer;
use rg_parse::FileId;
use rg_ty::Ty;

use crate::{
    Analysis, SymbolKind,
    model::{HoverBlock, HoverInfo, SymbolAt},
    query::declaration_details::{
        DeclarationDetails, DeclarationDetailsContext, DeclarationDetailsResolver,
    },
    source_symbol::SourceSymbolResolver,
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
        let Some(source_symbol) = self.0.source_symbol_at_for_query(target, file_id, offset)?
        else {
            return Ok(None);
        };
        let range = Some(source_symbol.span());
        let symbol = source_symbol.symbol().clone();
        let source_symbols = SourceSymbolResolver::new(self.0.view_db());
        let declarations = source_symbols.declarations_for_symbol(symbol.clone())?;
        let context = DeclarationDetailsContext {
            module_display_name: Self::module_display_name_for_symbol(&symbol),
        };
        let details = DeclarationDetailsResolver::new(self.0.view_db());
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
            && let Some(ty) = source_symbols.ty_for_symbol(symbol)?
            && let Some(block) = self.hover_for_ty(&ty)?
        {
            blocks.push(block);
        }

        Ok((!blocks.is_empty()).then_some(HoverInfo { range, blocks }))
    }

    fn module_display_name_for_symbol(symbol: &SymbolAt) -> Option<String> {
        match symbol {
            SymbolAt::TypePath { path, .. }
            | SymbolAt::ValuePath { path, .. }
            | SymbolAt::UsePath { path, .. } => path.last_segment_label(),
            SymbolAt::FunctionBody { .. }
            | SymbolAt::Declaration { .. }
            | SymbolAt::Expr { .. } => None,
        }
    }

    fn hover_for_ty(&self, ty: &Ty) -> anyhow::Result<Option<HoverBlock>> {
        let Some(signature) = TypeRenderer::new(self.0.view_db()).render(ty)? else {
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
            kind: details.kind.into(),
            path: details.path,
            signature: details.signature,
            ty: None,
            docs: details.docs,
        }
    }
}
