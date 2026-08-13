use std::{collections::BTreeSet, path::PathBuf};

use ls_types::Diagnostic;
use rg_lsp_proto::ServiceNotification;

use crate::service::ServiceNotificationsSink;

use super::cargo::CargoDiagnostics;

/// Complete saved-source diagnostics output for one Cargo run.
///
/// Editor applicability is intentionally absent. Every entry carries the exact saved text
/// observed for its path, and the server-side editor owner either publishes it at the matching
/// client version or leaves the client's prior diagnostics visible.
pub(super) struct WorkspaceDiagnostics {
    file_diagnostics: Vec<FileDiagnostics>,
    known_paths: BTreeSet<PathBuf>,
}

impl WorkspaceDiagnostics {
    pub(super) fn new(diagnostics: CargoDiagnostics, previous_paths: &BTreeSet<PathBuf>) -> Self {
        let mut diagnostics = diagnostics.into_inner();
        let mut known_paths = previous_paths.clone();
        known_paths.extend(diagnostics.keys().cloned());

        // Re-offer clears for all formerly reported paths on every run. Publication can be skipped
        // while an editor buffer differs from saved source, and this small path set lets a later
        // save-triggered run apply the clear without feedback or editor state in this process.
        let file_diagnostics = known_paths
            .iter()
            .cloned()
            .map(|path| {
                let saved_text = match rg_source::read_source_text(&path) {
                    Ok(text) => Some(text.to_string()),
                    Err(error) => {
                        tracing::trace!(
                            path = %path.display(),
                            error = %error,
                            "diagnostics source identity is unavailable"
                        );
                        None
                    }
                };
                FileDiagnostics {
                    diagnostics: diagnostics.remove(&path).unwrap_or_default(),
                    path,
                    saved_text,
                }
            })
            .collect();

        Self {
            file_diagnostics,
            known_paths,
        }
    }

    pub(super) fn take_known_paths(&mut self) -> BTreeSet<PathBuf> {
        std::mem::take(&mut self.known_paths)
    }

    pub(super) fn publish(self, notifications: &ServiceNotificationsSink) {
        for file_diagnostics in self.file_diagnostics {
            file_diagnostics.publish(notifications);
        }
    }
}

#[derive(Debug)]
struct FileDiagnostics {
    path: PathBuf,
    diagnostics: Vec<Diagnostic>,
    saved_text: Option<String>,
}

impl FileDiagnostics {
    fn publish(self, notifications: &ServiceNotificationsSink) {
        notifications.send(ServiceNotification::PublishDiagnostics {
            path: self.path,
            diagnostics: self.diagnostics,
            saved_text: self.saved_text,
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

    use super::{super::cargo::CargoDiagnostics, WorkspaceDiagnostics};

    #[test]
    fn current_results_and_known_stale_paths_are_both_emitted() {
        let stale = PathBuf::from("/workspace/src/lib.rs");
        let current = PathBuf::from("/workspace/src/main.rs");
        let diagnostics = CargoDiagnostics::from_map(BTreeMap::from([(
            current.clone(),
            vec![diagnostic("still broken")],
        )]));

        let workspace_diagnostics =
            WorkspaceDiagnostics::new(diagnostics, &BTreeSet::from([stale.clone()]));

        assert_eq!(workspace_diagnostics.file_diagnostics.len(), 2);
        assert_eq!(workspace_diagnostics.file_diagnostics[0].path, stale);
        assert!(
            workspace_diagnostics.file_diagnostics[0]
                .diagnostics
                .is_empty()
        );
        assert_eq!(workspace_diagnostics.file_diagnostics[1].path, current);
        assert_eq!(
            workspace_diagnostics.file_diagnostics[1].diagnostics.len(),
            1
        );
        assert_eq!(workspace_diagnostics.known_paths.len(), 2);
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
