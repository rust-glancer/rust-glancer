use rg_ir_model::{FileId, PackageSlot, Span};
use rg_source::QueryContext;
use rg_std::Shrink as _;
use rg_workspace::PackageInput;

use crate::{DeclarationSourceStore, FileTree, lower::FileLowering};

/// Two outputs from lowering the same saved file: declaration meaning and occurrence positions.
///
/// Adding a comment before `fn load() -> User` changes its positions, but can leave its lowered
/// declaration unchanged. Separate outputs let semantic queries depend on `items` without also
/// depending on `locations`. Queries that need positions read the matching location output.
#[ajika::output]
pub struct FileDeclarations {
    pub items: FileTree,
    pub locations: DeclarationSourceStore<Span>,
}

/// Lowers one file in its package context. Macro spans retain both the package slot and the
/// canonical file ID. Module membership is discovered separately because it also depends on
/// the logical directory from which the file was reached.
#[ajika::query]
pub fn file_declarations(
    db: &QueryContext<'_>,
    package: PackageSlot,
    file: FileId,
) -> ajika::Result<FileDeclarations> {
    let edition = PackageInput::at(package).edition(db)?;
    let syntax = rg_parse::saved_syntax(db, file, edition)?;
    let (mut tree, mut locations) = FileLowering::lower(package, file, edition, &syntax);
    locations.shrink_to_fit();
    tree.shrink_to_fit();
    FileDeclarations::new(db, tree, locations)
}

/// Acquiring a location map is an explicit dependency, separate from reading declaration meaning.
/// Read it from the operation's query context so reused declaration IDs resolve to that snapshot's
/// positions, including after an edit that only moved the declaration.
pub fn file_declaration_sources<'db>(
    db: &'db QueryContext<'db>,
    package: PackageSlot,
    file: FileId,
) -> ajika::Result<&'db DeclarationSourceStore<Span>> {
    file_declarations(db, package, file)?
        .locations
        .value_ref(db)
}
