use anyhow::Context as _;
use ls_types::{
    DocumentSymbol as LspDocumentSymbol, Location, OneOf, SymbolKind as LspSymbolKind, Uri,
    WorkspaceSymbol as LspWorkspaceSymbol,
};
use rg_analysis::{DocumentSymbol, SymbolKind, WorkspaceSymbol};
use rg_def_map::PackageSlot;
use rg_project::ProjectSnapshot;

use crate::proto::{navigation, position};

#[allow(deprecated)]
pub(crate) fn document_symbol(
    snapshot: ProjectSnapshot<'_>,
    package_slot: PackageSlot,
    symbol: DocumentSymbol,
) -> anyhow::Result<LspDocumentSymbol> {
    let line_index = snapshot
        .file_line_index(package_slot, symbol.file_id)
        .context("while attempting to find file for document symbol conversion")?;
    let children = symbol
        .children
        .into_iter()
        .map(|child| document_symbol(snapshot, package_slot, child))
        .collect::<anyhow::Result<Vec<_>>>()?;

    Ok(LspDocumentSymbol {
        name: symbol.name,
        detail: None,
        kind: symbol_kind(symbol.kind),
        tags: None,
        deprecated: None,
        range: position::range(line_index, symbol.span),
        selection_range: position::range(line_index, symbol.selection_span),
        children: (!children.is_empty()).then_some(children),
    })
}

pub(crate) fn workspace_symbol(
    snapshot: ProjectSnapshot<'_>,
    symbol: WorkspaceSymbol,
) -> anyhow::Result<Option<LspWorkspaceSymbol>> {
    let Some(path) = snapshot.file_path(symbol.target.package, symbol.file_id) else {
        return Ok(None);
    };
    let Some(uri) = Uri::from_file_path(path) else {
        return Ok(None);
    };
    let range =
        navigation::range_for_file(snapshot, symbol.target.package, symbol.file_id, symbol.span)?;

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
