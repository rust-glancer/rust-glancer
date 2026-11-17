use gen_lsp_types::{DocumentHighlight, DocumentHighlightKind, Location};
use rg_analysis::ReferenceLocation;
use rg_ir_model::Span;
use rg_lsp_proto::path_to_file_uri;
use rg_project::ProjectSnapshot;

use crate::proto::position;

pub(crate) fn location_for_reference(
    snapshot: ProjectSnapshot<'_>,
    reference: &ReferenceLocation,
) -> anyhow::Result<Option<Location>> {
    let path = snapshot.sources().path(reference.file_id)?;
    let Ok(uri) = path_to_file_uri(path) else {
        return Ok(None);
    };

    let range = position::range(
        snapshot.sources().line_index(reference.file_id)?,
        reference.span,
    );

    Ok(Some(Location { uri, range }))
}

/// Convert a current-body reference using the line index for the same editor text.
pub(crate) fn document_highlight_for_current_document(
    line_index: &rg_parse::LineIndex,
    span: Span,
) -> DocumentHighlight {
    DocumentHighlight {
        range: position::range(line_index, span),
        // Read/write classification is independent from source freshness and remains deferred.
        kind: Some(DocumentHighlightKind::Read),
    }
}
