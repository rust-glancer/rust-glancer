use std::path::Path;

use anyhow::Context as _;
use ls_types::{Location, Range};
use rg_analysis::NavigationTarget;
use rg_def_map::PackageSlot;
use rg_lsp_proto::path_to_file_uri;
use rg_parse::{FileId, Span};
use rg_project::ProjectSnapshot;

use crate::proto::position;

pub(crate) fn location_for_target(
    snapshot: ProjectSnapshot<'_>,
    target: &NavigationTarget,
) -> anyhow::Result<Option<Location>> {
    let Some(path) = snapshot.file_path(target.crate_ref.package, target.file_id) else {
        return Ok(None);
    };
    let Ok(uri) = path_to_file_uri(path) else {
        return Ok(None);
    };

    let range = range_for_file(
        snapshot,
        target.crate_ref.package,
        target.file_id,
        target.span,
    )?;

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
    package_slot: PackageSlot,
    file_id: FileId,
    span: Option<Span>,
) -> anyhow::Result<Range> {
    let Some(span) = span else {
        return Ok(position::zero_range());
    };
    let line_index = snapshot
        .file_line_index(package_slot, file_id)?
        .context("while attempting to find file for LSP range conversion")?;

    Ok(position::range(line_index, span))
}
