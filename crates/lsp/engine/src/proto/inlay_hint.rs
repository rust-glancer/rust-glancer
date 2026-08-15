use ls_types::{InlayHint, InlayHintKind, InlayHintLabel};
use rg_analysis::{InlayHint as AnalysisInlayHint, InlayHintKind as AnalysisInlayHintKind};

use crate::proto::position;

/// Convert a hint using the line index built from the same editor text as its span.
pub(crate) fn inlay_hint_with_line_index(
    line_index: &rg_parse::LineIndex,
    hint: AnalysisInlayHint,
) -> InlayHint {
    let kind = match hint.kind {
        AnalysisInlayHintKind::Type => Some(InlayHintKind::TYPE),
        AnalysisInlayHintKind::Parameter => Some(InlayHintKind::PARAMETER),
        AnalysisInlayHintKind::Text => None,
    };

    InlayHint {
        position: position::position(line_index, hint.text_offset()),
        label: InlayHintLabel::String(hint.label),
        kind,
        text_edits: None,
        tooltip: None,
        padding_left: hint.padding_left,
        padding_right: hint.padding_right,
        data: None,
    }
}
