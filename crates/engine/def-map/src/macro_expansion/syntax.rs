//! Shared syntax helpers for declarative macro expansion call sites.

use rg_parse::{FileId, Span};
use rg_tt::{Edition, Span as TtSpan, syntax_bridge::SpanFactory};
use rg_workspace::RustEdition;

pub(crate) fn macro_edition(edition: RustEdition) -> Edition {
    match edition {
        RustEdition::Edition2015 => Edition::Edition2015,
        RustEdition::Edition2018 => Edition::Edition2018,
        RustEdition::Edition2021 => Edition::Edition2021,
        RustEdition::Edition2024 => Edition::Edition2024,
    }
}

pub(crate) fn tt_span_for_parse_span(file_id: FileId, span: Span, edition: Edition) -> TtSpan {
    let text_range = rg_syntax::TextRange::new(span.text.start.into(), span.text.end.into());
    SpanFactory::new(
        u32::try_from(file_id.0).expect("file id should fit macro span storage"),
        edition,
    )
    .span_for(text_range)
}
