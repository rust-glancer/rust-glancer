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
    let Some(path) = snapshot.file_path(reference.crate_ref.package, reference.file_id) else {
        return Ok(None);
    };
    let Some(uri) = Uri::from_file_path(path) else {
        return Ok(None);
    };

    let range = range_for_file(
        snapshot,
        reference.crate_ref.package,
        reference.file_id,
        reference.span,
    )?;

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
        kind: Some(DocumentHighlightKind::READ),
    }
}

fn range_for_file(
    snapshot: ProjectSnapshot<'_>,
    package_slot: PackageSlot,
    file_id: FileId,
    span: Span,
) -> anyhow::Result<ls_types::Range> {
    let line_index = snapshot
        .file_line_index(package_slot, file_id)?
        .context("while attempting to find file for LSP range conversion")?;

    Ok(position::range(line_index, span))
}
