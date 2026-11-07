use rg_ir_model::FileId;
use rg_source::SourceInput;
use rg_syntax::{Parse, SourceFile};
use rg_text::RustEdition;

use crate::parse_source_file;

/// Shares syntax for a captured source within one operation.
#[ajika::query(transient)]
pub fn saved_syntax(
    db: &ajika::Db<'_>,
    file: FileId,
    edition: RustEdition,
) -> ajika::Result<Parse<SourceFile>> {
    let path = file.path(&db.interner())?;
    let entry = SourceInput::at(path).entry(db)?;
    let text = entry.text().map_err(ajika::Error::application)?;
    Ok(parse_source_file(&text, edition))
}
