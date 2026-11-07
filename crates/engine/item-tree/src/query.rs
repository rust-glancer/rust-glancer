use rg_ir_model::{FileId, PackageSlot};
use rg_std::Shrink as _;
use rg_workspace::PackageInput;

use crate::{FileTree, lower::FileLowering};

/// Lowers one file in its package context. Macro spans retain both the package slot and the
/// canonical file ID. Module membership is discovered separately because it also depends on
/// the logical directory from which the file was reached.
#[ajika::query]
pub fn file_declarations(
    db: &ajika::Db<'_>,
    package: PackageSlot,
    file: FileId,
) -> ajika::Result<FileTree> {
    let edition = PackageInput::at(package).edition(db)?;
    let syntax = rg_parse::saved_syntax(db, file, edition)?;
    let mut tree = FileLowering::lower(package, file, edition, &syntax.tree());
    tree.shrink_to_fit();
    Ok(tree)
}
