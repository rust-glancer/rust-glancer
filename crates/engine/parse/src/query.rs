use rg_ir_model::FileId;
use rg_source::SourceInput;
use rg_syntax::{Parse, SourceFile};
use rg_text::RustEdition;

use crate::parse_source_file;

/// Shares syntax for a captured source within one operation.
/// Saved and editor sources use this same provider. Its transient policy lets each operation
/// share a parse without retaining parse trees between requests.
#[ajika::query(transient)]
pub fn source_syntax(
    db: &rg_source::QueryContext<'_>,
    file: FileId,
    edition: RustEdition,
) -> ajika::Result<Parse<SourceFile>> {
    let entry = SourceInput::entry(db, file)?;
    let text = entry.text().map_err(ajika::Error::application)?;
    Ok(parse_source_file(&text, edition))
}
