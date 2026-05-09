use std::{collections::BTreeSet, path::PathBuf};

use ls_types::Diagnostic;
use rg_lsp_proto::EngineEvent;

use crate::{documents::DocumentStore, events::EngineEventSink};

use super::diagnostics::CheckDiagnostics;

/// Complete diagnostics publication for one cargo diagnostics run.
///
/// `file_diagnostics` is what should be sent to the client, while `published_paths` records
/// which files should still have live cargo diagnostics after this plan. Dirty files may be kept
/// live without sending a new publication, because their visible buffer no longer matches cargo's
/// saved-file snapshot.
pub(super) struct WorkspaceDiagnostics {
    file_diagnostics: Vec<FileDiagnostics>,
    published_paths: BTreeSet<PathBuf>,
}

impl WorkspaceDiagnostics {
    pub(super) fn new(
        diagnostics: CheckDiagnostics,
        documents: &DocumentStore,
        previous_paths: &BTreeSet<PathBuf>,
    ) -> Self {
        let mut file_diagnostics = Vec::new();
        let mut published_paths = BTreeSet::new();

        for (path, diagnostics) in diagnostics.into_inner() {
            let freshness = documents.freshness(&path);

            if freshness.dirty() {
                // Cargo diagnostics belong to the saved snapshot. When the editor has a newer
                // dirty buffer, leave the client state untouched and let it keep remapping the
                // last diagnostics it already knows about.
                if previous_paths.contains(&path) {
                    published_paths.insert(path);
                }
                continue;
            }

            let version = freshness.tracked().then(|| freshness.version()).flatten();
            published_paths.insert(path.clone());
            file_diagnostics.push(FileDiagnostics {
                path,
                diagnostics,
                version,
            });
        }

        for stale_path in previous_paths {
            if published_paths.contains(stale_path) {
                continue;
            }

            let freshness = documents.freshness(stale_path);
            if freshness.dirty() {
                // The latest cargo run no longer mentions this file, but clearing while the buffer
                // is dirty would make diagnostics disappear for a state cargo never checked.
                published_paths.insert(stale_path.clone());
                continue;
            }

            let version = freshness.tracked().then(|| freshness.version()).flatten();
            file_diagnostics.push(FileDiagnostics {
                path: stale_path.clone(),
                diagnostics: Vec::new(),
                version,
            });
        }

        Self {
            file_diagnostics,
            published_paths,
        }
    }

    pub(super) fn take_published_paths(&mut self) -> BTreeSet<PathBuf> {
        std::mem::take(&mut self.published_paths)
    }

    pub(super) fn publish(self, events: &EngineEventSink) {
        for file_diagnostics in self.file_diagnostics {
            file_diagnostics.publish(events);
        }
    }
}

#[derive(Debug)]
struct FileDiagnostics {
    path: PathBuf,
    diagnostics: Vec<Diagnostic>,
    version: Option<i32>,
}

impl FileDiagnostics {
    fn publish(self, events: &EngineEventSink) {
        events.send(EngineEvent::PublishDiagnostics {
            path: self.path,
            diagnostics: self.diagnostics,
            version: self.version,
        });
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, BTreeSet},
        path::PathBuf,
    };

    use ls_types::{Diagnostic, Position, Range};

    use super::WorkspaceDiagnostics;
    use crate::{check::diagnostics::CheckDiagnostics, documents::DocumentStore};

    #[test]
    fn new_clears_stale_diagnostic_files() {
        let previous_paths = BTreeSet::from([
            PathBuf::from("/workspace/src/lib.rs"),
            PathBuf::from("/workspace/src/main.rs"),
        ]);
        let diagnostics = CheckDiagnostics::from_map(BTreeMap::from([(
            PathBuf::from("/workspace/src/main.rs"),
            vec![diagnostic("still broken")],
        )]));

        let documents = DocumentStore::default();
        let workspace_diagnostics =
            WorkspaceDiagnostics::new(diagnostics, &documents, &previous_paths);

        assert_eq!(workspace_diagnostics.file_diagnostics.len(), 2);
        assert_eq!(
            workspace_diagnostics.file_diagnostics[0].path,
            PathBuf::from("/workspace/src/main.rs")
        );
        assert_eq!(
            workspace_diagnostics.file_diagnostics[0].diagnostics.len(),
            1
        );
        assert_eq!(
            workspace_diagnostics.file_diagnostics[1].path,
            PathBuf::from("/workspace/src/lib.rs")
        );
        assert!(
            workspace_diagnostics.file_diagnostics[1]
                .diagnostics
                .is_empty()
        );
        assert_eq!(
            workspace_diagnostics.published_paths,
            [PathBuf::from("/workspace/src/main.rs")].into()
        );
    }

    #[test]
    fn new_leaves_dirty_documents_unpublished() {
        let path = PathBuf::from("/workspace/src/lib.rs");
        let diagnostics =
            CheckDiagnostics::from_map(BTreeMap::from([(path.clone(), vec![diagnostic("new")])]));
        let mut documents = DocumentStore::default();
        documents.did_open(path.clone(), Some(1), "fn main() {}\n");
        documents.did_change(path.clone(), Some(2), Some("fn main() {\n}\n"));

        let workspace_diagnostics =
            WorkspaceDiagnostics::new(diagnostics, &documents, &BTreeSet::new());

        assert!(workspace_diagnostics.file_diagnostics.is_empty());
        assert!(workspace_diagnostics.published_paths.is_empty());
    }

    #[test]
    fn new_keeps_previous_dirty_diagnostics_visible() {
        let path = PathBuf::from("/workspace/src/lib.rs");
        let diagnostics = CheckDiagnostics::from_map(BTreeMap::from([(
            path.clone(),
            vec![diagnostic("saved snapshot changed")],
        )]));
        let previous_paths = BTreeSet::from([path.clone()]);
        let mut documents = DocumentStore::default();
        documents.did_open(path.clone(), Some(1), "fn main() {}\n");
        documents.did_change(path.clone(), Some(2), Some("fn main() {\n}\n"));

        let workspace_diagnostics =
            WorkspaceDiagnostics::new(diagnostics, &documents, &previous_paths);

        assert!(workspace_diagnostics.file_diagnostics.is_empty());
        assert_eq!(workspace_diagnostics.published_paths, previous_paths);
    }

    #[test]
    fn new_keeps_stale_dirty_diagnostics_until_clean_check() {
        let path = PathBuf::from("/workspace/src/lib.rs");
        let diagnostics = CheckDiagnostics::default();
        let previous_paths = BTreeSet::from([path.clone()]);
        let mut documents = DocumentStore::default();
        documents.did_open(path.clone(), Some(1), "fn main() {}\n");
        documents.did_change(path, Some(2), Some("fn main() {\n}\n"));

        let workspace_diagnostics =
            WorkspaceDiagnostics::new(diagnostics, &documents, &previous_paths);

        assert!(workspace_diagnostics.file_diagnostics.is_empty());
        assert_eq!(workspace_diagnostics.published_paths, previous_paths);
    }

    fn diagnostic(message: &str) -> Diagnostic {
        Diagnostic {
            range: Range::new(Position::new(0, 0), Position::new(0, 1)),
            severity: None,
            code: None,
            code_description: None,
            source: None,
            message: message.to_string(),
            related_information: None,
            tags: None,
            data: None,
        }
    }
}
