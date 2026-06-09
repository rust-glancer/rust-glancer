use anyhow::Context as _;
use ls_types::{InlayHint, InlayHintKind, InlayHintLabel};
use rg_analysis::{InlayHint as AnalysisInlayHint, InlayHintKind as AnalysisInlayHintKind};
use rg_def_map::PackageSlot;
use rg_project::ProjectSnapshot;

use crate::proto::position;

pub(crate) fn inlay_hint(
    snapshot: ProjectSnapshot<'_>,
    package_slot: PackageSlot,
    hint: AnalysisInlayHint,
) -> anyhow::Result<Option<InlayHint>> {
    let line_index = snapshot
        .file_line_index(package_slot, hint.file_id)
        .context("while attempting to find file for inlay hint conversion")?;
    let kind = match hint.kind {
        AnalysisInlayHintKind::Type => Some(InlayHintKind::TYPE),
        AnalysisInlayHintKind::Parameter => Some(InlayHintKind::PARAMETER),
        AnalysisInlayHintKind::Text => None,
    };

    Ok(Some(InlayHint {
        position: position::position(line_index, hint.text_offset()),
        label: InlayHintLabel::String(hint.label),
        kind,
        text_edits: None,
        tooltip: None,
        padding_left: hint.padding_left,
        padding_right: hint.padding_right,
        data: None,
    }))
}
