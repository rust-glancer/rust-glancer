use ls_types::{
    DocumentSymbol as LspDocumentSymbol, Location, OneOf, SymbolKind as LspSymbolKind, Uri,
    WorkspaceSymbol as LspWorkspaceSymbol,
};
use rg_analysis::{DocumentSymbol, SymbolKind, WorkspaceSymbol};
use rg_parse::LineIndex;
use rg_project::ProjectSnapshot;

use crate::proto::{navigation, position};

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
    symbol: WorkspaceSymbol,
) -> anyhow::Result<Option<LspWorkspaceSymbol>> {
    let Some(path) = snapshot.file_path(symbol.crate_ref.package, symbol.file_id) else {
        return Ok(None);
    };
    let Some(uri) = Uri::from_file_path(path) else {
        return Ok(None);
    };
    let range = navigation::range_for_file(
        snapshot,
        symbol.crate_ref.package,
        symbol.file_id,
        symbol.span,
    )?;

    Ok(Some(LspWorkspaceSymbol {
        name: symbol.name,
        kind: symbol_kind(symbol.kind),
        tags: None,
        container_name: symbol.container_name,
        location: OneOf::Left(Location { uri, range }),
        data: None,
    }))
}

pub(crate) fn symbol_kind(kind: SymbolKind) -> LspSymbolKind {
    match kind {
        SymbolKind::Const | SymbolKind::Static => LspSymbolKind::CONSTANT,
        SymbolKind::Enum => LspSymbolKind::ENUM,
        SymbolKind::EnumVariant => LspSymbolKind::ENUM_MEMBER,
        SymbolKind::Field => LspSymbolKind::FIELD,
        SymbolKind::Function => LspSymbolKind::FUNCTION,
        SymbolKind::Impl => LspSymbolKind::OBJECT,
        SymbolKind::Macro => LspSymbolKind::FUNCTION,
        SymbolKind::Method => LspSymbolKind::METHOD,
        SymbolKind::Module => LspSymbolKind::MODULE,
        SymbolKind::Struct | SymbolKind::Union => LspSymbolKind::STRUCT,
        SymbolKind::Trait => LspSymbolKind::INTERFACE,
        SymbolKind::TypeAlias => LspSymbolKind::CLASS,
        SymbolKind::Variable => LspSymbolKind::VARIABLE,
    }
}
