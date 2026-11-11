use rg_ir_model::FileId;
use rg_source::SourceInput;
use rg_syntax::{Parse, SourceFile};
use rg_text::RustEdition;

use crate::parse_source_file;

/// Shares syntax for a captured source within one operation.
#[ajika::query(transient)]
pub fn saved_syntax(
    db: &rg_source::QueryContext<'_>,
    file: FileId,
    edition: RustEdition,
) -> ajika::Result<Parse<SourceFile>> {
    let entry = SourceInput::entry(db, file)?;
    let text = entry.text().map_err(ajika::Error::application)?;
    Ok(parse_source_file(&text, edition))
}
