//! Builds hover payloads from resolved analysis declarations.

mod formatting;

use anyhow::Context as _;
use rg_ir_model::{CrateRef, FileId, SemanticItemRef, identity::DeclarationRef};
use rg_ir_view::{
    display::ty_label::TypeRenderer,
    item::{
        details::{DeclarationDetails, DeclarationDetailsContext, DeclarationDetailsView},
        initializer::InitializerView,
    },
    ty::IndexedType,
};
use rg_std::UniqueVec;

use crate::{
    Analysis, SymbolKind,
    documentation::{DocumentationLinkResolver, SourceDocumentationQuery},
    model::{HoverBlock, HoverInfo, SymbolAt},
    source_symbol::{SourceSymbol, SourceSymbolResolver},
};

pub(crate) struct HoverResolver<'a, 'db>(&'a Analysis<'db>);

impl<'a, 'db> HoverResolver<'a, 'db> {
    pub(crate) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self(analysis)
    }

    pub(crate) fn hover(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<HoverInfo>> {
        if let Some(link) = SourceDocumentationQuery::new(self.0)
            .link_at(crate_ref, file_id, offset)
            .context("find hovered documentation link")?
        {
            return self.hover_for_source_symbol(
                crate_ref,
                SourceSymbol::plain_declaration(link.declaration, crate_ref, file_id, link.span),
            );
        }
        let symbols = self
            .0
            .source_symbols_at_for_query(crate_ref, file_id, offset)?;
        // A repeated argument may have different meanings at its expanded uses. Show each result,
        // while folding identical hover blocks back into one entry for the written token.
        let mut range = None;
        let mut blocks = UniqueVec::new();
        for symbol in symbols {
            let Some(hover) = self.hover_for_source_symbol(crate_ref, symbol)? else {
                continue;
            };
            range.get_or_insert(hover.range);
            blocks.extend(hover.blocks);
        }
        Ok(range.map(|range| HoverInfo {
            range,
            blocks: blocks.into_vec(),
        }))
    }

    fn hover_for_source_symbol(
        &self,
        crate_ref: CrateRef,
        source_symbol: SourceSymbol,
    ) -> anyhow::Result<Option<HoverInfo>> {
        let range = Some(source_symbol.span());
        let symbol = source_symbol.symbol().clone();
        let source_symbols = SourceSymbolResolver::new(self.0.view_db());
        let declarations = source_symbols.declarations_for_symbol(symbol.clone())?;
        let context = DeclarationDetailsContext::new(Self::module_display_name_for_symbol(&symbol));
        let edition = self.0.view_db().crate_edition(crate_ref)?;
        let details = DeclarationDetailsView::new(self.0.view_db(), edition);
        let type_renderer = TypeRenderer::new(self.0.view_db(), edition);
        let mut blocks = UniqueVec::new();

        for declaration in declarations {
            let Some(details) = details.details_for_declaration(declaration, &context)? else {
                continue;
            };
            let block = self
                .hover_block(declaration, details)
                .context("resolve hover documentation")?;
            blocks.push(block);
        }

        if blocks.is_empty()
            && let Some(ty) = source_symbols.ty_for_symbol(symbol)?
            && let Some(block) = self.hover_for_ty(&type_renderer, &ty)?
        {
            blocks.push(block);
        }

        Ok((!blocks.is_empty()).then_some(HoverInfo {
            range,
            blocks: blocks.into_vec(),
        }))
    }

    fn module_display_name_for_symbol(symbol: &SymbolAt) -> Option<String> {
        match symbol {
            SymbolAt::TypePath { type_path, .. } => type_path.path().last_segment_label(),
            SymbolAt::ValuePath { path, .. } | SymbolAt::UsePath { path, .. } => {
                path.last_segment_label()
            }
            SymbolAt::FunctionBody { .. }
            | SymbolAt::Declaration { .. }
            | SymbolAt::Expr { .. }
            | SymbolAt::RecordField { .. } => None,
        }
    }

    fn hover_for_ty(
        &self,
        renderer: &TypeRenderer<'_, '_>,
        ty: &IndexedType,
    ) -> anyhow::Result<Option<HoverBlock>> {
        let Some(signature) = renderer.render(ty)? else {
            return Ok(None);
        };
        Ok(Some(HoverBlock {
            kind: SymbolKind::TypeAlias,
            path: None,
            signature: None,
            ty: Some(signature),
            docs: None,
            doc_links: Vec::new(),
        }))
    }

    fn hover_block(
        &self,
        declaration: DeclarationRef,
        details: DeclarationDetails,
    ) -> anyhow::Result<HoverBlock> {
        let (kind, path, signature, docs) = details.into_parts();
        // Item previews can contain elided fields (`...`), so retain their existing layout.
        // Function headers have no preview limit and benefit from Rust-aware line wrapping.
        let mut signature = signature.map(|signature| match kind {
            SymbolKind::Function | SymbolKind::Method => {
                formatting::function_signature(&signature).unwrap_or(signature)
            }
            _ => signature,
        });
        // Initializer details belong to hover. Shared declaration signatures used for completion
        // and generated code omit both evaluated values and source previews.
        let initializer = match declaration {
            DeclarationRef::Item(SemanticItemRef::Const(item)) => Some(item.into()),
            DeclarationRef::Item(SemanticItemRef::Static(item)) => Some(item.into()),
            _ => None,
        };
        if let Some(initializer) = initializer
            && let Some(signature) = &mut signature
            && let Some(value) = InitializerView::new(self.0.view_db()).render(initializer)?
        {
            signature.push_str(" = ");
            signature.push_str(&value);
        }
        let doc_links = match &docs {
            Some(docs) => DocumentationLinkResolver::new(self.0.view_db())
                .resolve(declaration, docs)
                .context("resolve hover item links")?,
            None => Vec::new(),
        };
        Ok(HoverBlock {
            kind,
            path,
            signature,
            ty: None,
            docs,
            doc_links,
        })
    }
}
