use gen_lsp_types::{DocumentHighlight, DocumentHighlightKind};
use rg_ir_model::Span;

use crate::proto::position;

pub(crate) fn document_highlight(
    line_index: &rg_parse::LineIndex,
    span: Span,
) -> DocumentHighlight {
    DocumentHighlight {
        range: position::range(line_index, span),
        // Read/write classification is independent from source freshness and remains deferred.
        kind: Some(DocumentHighlightKind::Read),
    }
}
