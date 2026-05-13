use anyhow::Context as _;
use ls_types::{DocumentHighlight, DocumentHighlightKind, Location, Uri};
use rg_analysis::ReferenceLocation;
use rg_def_map::PackageSlot;
use rg_parse::{FileId, Span};
use rg_project::ProjectSnapshot;

use crate::proto::position;

pub(crate) fn location_for_reference(
    snapshot: ProjectSnapshot<'_>,
    reference: &ReferenceLocation,
) -> anyhow::Result<Option<Location>> {
    let Some(path) = snapshot.file_path(reference.target.package, reference.file_id) else {
        return Ok(None);
    };
    let Some(uri) = Uri::from_file_path(path) else {
        return Ok(None);
    };

    let range = range_for_file(
        snapshot,
        reference.target.package,
        reference.file_id,
        reference.span,
    )?;

    Ok(Some(Location { uri, range }))
}

pub(crate) fn document_highlight_for_reference(
    snapshot: ProjectSnapshot<'_>,
    package: PackageSlot,
    file_id: FileId,
    span: Span,
) -> anyhow::Result<DocumentHighlight> {
    Ok(DocumentHighlight {
        range: range_for_file(snapshot, package, file_id, span)?,
        // We use `read` highlight kind because it's simpler and barely noticeable. We'll fix it if
        // somebody will actually notice the difference, which I doubt; until then it's not worth it.
        kind: Some(DocumentHighlightKind::READ),
    })
}

fn range_for_file(
    snapshot: ProjectSnapshot<'_>,
    package_slot: PackageSlot,
    file_id: FileId,
    span: Span,
) -> anyhow::Result<ls_types::Range> {
    let line_index = snapshot
        .file_line_index(package_slot, file_id)
        .context("while attempting to find file for LSP range conversion")?;

    Ok(position::range(line_index, span))
}
