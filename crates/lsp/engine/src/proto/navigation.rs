use gen_lsp_types::Location;
use rg_analysis::NavigationTarget;
use rg_ir_model::{FileId, Span};
use rg_lsp_proto::{EditorSnapshot, path_to_file_uri};
use rg_project::ProjectSnapshot;

use crate::proto::position;

/// Locations use the operation's source reader. Only URI spelling needs an editor mapping:
/// an editor may have opened a symlink while project identities use its canonical path.
pub(crate) struct SourceLocations<'editor, 'project> {
    snapshot: ProjectSnapshot<'project>,
    editor: &'editor EditorSnapshot,
}

impl<'editor, 'project> SourceLocations<'editor, 'project> {
    pub(crate) fn new(
        snapshot: ProjectSnapshot<'project>,
        editor: &'editor EditorSnapshot,
    ) -> Self {
        Self { snapshot, editor }
    }

    pub(crate) fn for_target(&self, target: &NavigationTarget) -> anyhow::Result<Option<Location>> {
        self.for_source(target.file_id, target.span)
    }

    pub(crate) fn for_source(
        &self,
        file: FileId,
        span: Option<Span>,
    ) -> anyhow::Result<Option<Location>> {
        let source_path = self.snapshot.sources().path(file)?;
        let path = self
            .editor
            .document_for_source(source_path)
            .map_or(source_path, |document| document.path());
        let Ok(uri) = path_to_file_uri(path) else {
            return Ok(None);
        };
        let range = match span {
            Some(span) => position::range(self.snapshot.sources().line_index(file)?, span),
            None => position::zero_range(),
        };
        Ok(Some(Location { uri, range }))
    }
}
