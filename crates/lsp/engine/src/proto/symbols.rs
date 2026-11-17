use gen_lsp_types::{
    BaseSymbolInformation, DocumentSymbol as LspDocumentSymbol, SymbolKind as LspSymbolKind,
    WorkspaceSymbol as LspWorkspaceSymbol,
};
use rg_analysis::{DocumentSymbol, SymbolKind, WorkspaceSymbol};
use rg_lsp_proto::EditorSnapshot;
use rg_parse::LineIndex;
use rg_project::ProjectSnapshot;

use crate::proto::{navigation::SourceLocations, position};

/// Convert a syntax outline using the line index for the same editor text.
#[allow(deprecated)]
pub(crate) fn document_symbol(line_index: &LineIndex, symbol: DocumentSymbol) -> LspDocumentSymbol {
    let children = symbol
        .children
        .into_iter()
        .map(|child| document_symbol(line_index, child))
        .collect::<Vec<_>>();

    LspDocumentSymbol {
        name: symbol.name,
        detail: None,
        kind: symbol_kind(symbol.kind),
        tags: None,
        deprecated: None,
        range: position::range(line_index, symbol.span),
        selection_range: position::range(line_index, symbol.selection_span),
        children: (!children.is_empty()).then_some(children),
    }
}

pub(crate) fn workspace_symbol(
    snapshot: ProjectSnapshot<'_>,
    editor: &EditorSnapshot,
    symbol: WorkspaceSymbol,
) -> anyhow::Result<Option<LspWorkspaceSymbol>> {
    let Some(location) =
        SourceLocations::new(snapshot, editor).for_source(symbol.file_id, symbol.span)?
    else {
        return Ok(None);
    };

    Ok(Some(LspWorkspaceSymbol {
        base_symbol_information: BaseSymbolInformation {
            name: symbol.name,
            kind: symbol_kind(symbol.kind),
            tags: None,
            container_name: symbol.container_name,
        },
        location: location.into(),
        data: None,
    }))
}

pub(crate) fn symbol_kind(kind: SymbolKind) -> LspSymbolKind {
    match kind {
        SymbolKind::Const | SymbolKind::Static => LspSymbolKind::Constant,
        SymbolKind::Enum => LspSymbolKind::Enum,
        SymbolKind::EnumVariant => LspSymbolKind::EnumMember,
        SymbolKind::Field => LspSymbolKind::Field,
        SymbolKind::Function => LspSymbolKind::Function,
        SymbolKind::Impl => LspSymbolKind::Object,
        SymbolKind::Macro => LspSymbolKind::Function,
        SymbolKind::Method => LspSymbolKind::Method,
        SymbolKind::Module => LspSymbolKind::Module,
        SymbolKind::Struct | SymbolKind::Union => LspSymbolKind::Struct,
        SymbolKind::Trait => LspSymbolKind::Interface,
        SymbolKind::TypeAlias => LspSymbolKind::Class,
        SymbolKind::Variable => LspSymbolKind::Variable,
    }
}
