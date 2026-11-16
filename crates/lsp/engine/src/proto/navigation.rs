use std::path::Path;

use gen_lsp_types::{Location, Range};
use rg_analysis::NavigationTarget;
use rg_ir_model::{FileId, Span};
use rg_lsp_proto::path_to_file_uri;
use rg_project::ProjectSnapshot;

use crate::proto::position;

pub(crate) fn location_for_target(
    snapshot: ProjectSnapshot<'_>,
    target: &NavigationTarget,
) -> anyhow::Result<Option<Location>> {
    let path = snapshot.sources().path(target.file_id)?;
    let Ok(uri) = path_to_file_uri(path) else {
        return Ok(None);
    };

    let range = range_for_file(snapshot, target.file_id, target.span)?;

    Ok(Some(Location { uri, range }))
}

/// Convert a target in the current document using that document's own line index.
pub(crate) fn location_for_current_document(
    path: &Path,
    line_index: &rg_parse::LineIndex,
    target: &NavigationTarget,
) -> Option<Location> {
    let uri = path_to_file_uri(path).ok()?;
    let range = target
        .span
        .map(|span| position::range(line_index, span))
        .unwrap_or_else(position::zero_range);
    Some(Location { uri, range })
}

pub(crate) fn range_for_file(
    snapshot: ProjectSnapshot<'_>,
    file_id: FileId,
    span: Option<Span>,
) -> anyhow::Result<Range> {
    let Some(span) = span else {
        return Ok(position::zero_range());
    };
    let line_index = snapshot.sources().line_index(file_id)?;

    Ok(position::range(line_index, span))
}
