//! Shared syntax helpers for declarative macro expansion call sites.

use rg_ir_model::{FileId, Span};
use rg_tt::{Edition, Span as TtSpan, syntax_bridge::SpanFactory};

pub(crate) fn tt_span_for_parse_span(file_id: FileId, span: Span, edition: Edition) -> TtSpan {
    let text_range = rg_syntax::TextRange::new(span.text.start.into(), span.text.end.into());
    SpanFactory::new(
        u32::try_from(file_id.0).expect("file id should fit macro span storage"),
        edition,
    )
    .span_for(text_range)
}
