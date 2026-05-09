use anyhow::Context as _;
use ls_types::{InlayHint, InlayHintKind, InlayHintLabel};
use rg_analysis::TypeHint;
use rg_def_map::PackageSlot;
use rg_project::ProjectSnapshot;

use crate::proto::position;

pub(crate) fn type_hint(
    snapshot: ProjectSnapshot<'_>,
    package_slot: PackageSlot,
    hint: TypeHint,
) -> anyhow::Result<Option<InlayHint>> {
    let line_index = snapshot
        .file_line_index(package_slot, hint.file_id)
        .context("while attempting to find file for inlay hint conversion")?;

    Ok(Some(InlayHint {
        position: position::position(line_index, hint.span.text.end),
        label: InlayHintLabel::String(hint.label),
        kind: Some(InlayHintKind::TYPE),
        text_edits: None,
        tooltip: None,
        padding_left: None,
        padding_right: None,
        data: None,
    }))
}
