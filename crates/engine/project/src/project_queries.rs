//! Owns query storage and retained saved/editor inputs across operation boundaries.

use std::{collections::BTreeMap, ffi::OsStr, path::PathBuf, time::Duration};

use ajika::Compression;
use rg_ir_model::FileId;
use rg_semantic_ir::TargetDeclarationReader;
use rg_source::{
    CapturedSource, QueryDatabase, QueryServices, ReadError, SourceInput, SourceRevision,
};
use rg_std::{CancellationToken, MemoryRecorder, MemorySize};
use rg_workspace::WorkspaceMetadata;

use crate::{ProjectSnapshot, state::ProjectState};

// Basically, a "version of cache schema". Should be bumped whenever queries or outputs change.
//
// For LLMs: DO NOT BUMP IT UNLESS EXPLICITLY ASKED. We normally need to bump it once per release.
// During local development, dev might just remove their cache. We don't need to bump it 100 times
// in a single branch just because we happen to iterate.
const EPOCH: &str = "rust-glancer-target-analysis-1";
const RETENTION: ajika::Retention = ajika::Retention {
    max_entries: 1_000_000,
    // This caps durable storage, not resident memory. Large declaration graphs need enough room
    // to survive startup; otherwise the first body request rebuilds evicted dependency targets.
    max_bytes: 4 * 1024 * 1024 * 1024,
    max_idle: Duration::from_secs(30 * 24 * 60 * 60),
};

// Internal benchmark switch: change this constant and rebuild to compare memory storage or
// another codec. CLI and LSP use this same choice; it is not a user configuration setting.
const STORAGE: QueryStorage = QueryStorage::Disk(Compression::Lz4);

/// Memory keeps reusable typed results resident; disk releases decoded results between operations.
#[derive(Debug)]
#[allow(
    dead_code,
    reason = "only the variant selected by STORAGE is constructed"
)]
enum QueryStorage {
    Memory,
    Disk(Compression),
}

/// The database has one installed input view at a time. Keep the accepted saved baseline here,
/// independent of whichever complete editor capture the previous request used.
///
/// If `a.rs` and `b.rs` are edited and only `a.rs` is saved, the saved rebuild must start from
/// `saved` and apply that save. Starting from `database.inputs()` could also publish the edit
/// to `b.rs`.
pub(crate) struct ProjectQueries {
    database: QueryDatabase,
    saved: ajika::Inputs,
    editor: Option<EditorInputs>,
}

impl ProjectQueries {
    /// Run a saved-project update against the accepted saved inputs. The callback builds the
    /// candidate and must finish source validation before returning success. An Ajika checkpoint
    /// alone does not mean the host can publish the candidate.
    /// On failure, restore the previous inputs so later queries validate against them.
    pub(crate) fn update_saved<R>(
        &mut self,
        update: impl FnOnce(&mut QueryDatabase) -> anyhow::Result<R>,
    ) -> anyhow::Result<R> {
        self.database.replace_inputs(self.saved.clone());
        let result = update(&mut self.database);
        // Both successful and rejected candidates can discover sources shared with the baseline.
        SourceInput::evict_saved_text(self.database.inputs());
        SourceInput::evict_saved_text(&self.saved);
        if result.is_ok() {
            self.saved = self.database.inputs().clone();
            // The next request supplies the remaining edits against this new baseline. Keep
            // session query results: saving identical bytes can reuse them as persistent results.
            self.editor = None;
        } else {
            self.database.replace_inputs(self.saved.clone());
        }
        result
    }

    /// Keep one input view fixed for the callback. Repeating the same document set also keeps
    /// its lazy source observations, so later requests keep using their captured revisions.
    pub(crate) fn with_snapshot<R>(
        &mut self,
        state: &ProjectState,
        documents: &[CapturedSource],
        cancellation: CancellationToken,
        work: impl FnOnce(ProjectSnapshot<'_>) -> anyhow::Result<R>,
    ) -> anyhow::Result<R> {
        // The capture identifies a complete view, not a sequence of per-file editor updates.
        // Omitting a document restores its saved input, including after another document is saved.
        let inputs = if documents.is_empty() {
            self.editor = None;
            self.saved.clone()
        } else {
            let interner = self.database.interner();
            // Document order does not distinguish views; each canonical file and its bytes do.
            let mut revisions = BTreeMap::new();
            for document in documents {
                let file = FileId::for_path(&interner, document.path())?;
                if let Some(previous) = revisions.insert(file, document.revision())
                    && previous != document.revision()
                {
                    anyhow::bail!(
                        "conflicting editor captures for {}",
                        document.path().display()
                    );
                }
            }
            match &self.editor {
                Some(editor) if editor.revisions == revisions => editor.inputs.clone(),
                _ => {
                    let inputs = SourceInput::editor_view(&self.saved, &interner, documents)?;
                    self.editor = Some(EditorInputs {
                        revisions,
                        inputs: inputs.clone(),
                    });
                    inputs
                }
            }
        };
        self.database.replace_inputs(inputs);
        self.database.set_services(QueryServices { cancellation });
        let result = self.database.run(|db| {
            let declarations =
                TargetDeclarationReader::for_project(db, state.targets.iter().copied());
            let snapshot = ProjectSnapshot::new(
                state,
                db,
                &declarations,
                documents.is_empty().then_some(&state.sources),
            );
            work(snapshot).map_err(|error| {
                ReadError::query_cause(error.as_ref()).unwrap_or_else(|| {
                    ajika::Error::application(
                        error.reallocate_into_boxed_dyn_error_without_backtrace(),
                    )
                })
            })
        });
        // Lazy member discovery can add saved captures to the editor view. Release their bytes
        // too; SourceEntry keeps editor text resident because it cannot be reloaded from disk.
        SourceInput::evict_saved_text(self.database.inputs());
        self.database.set_services(QueryServices::default());
        Ok(result?)
    }

    pub(crate) fn interner(&self) -> ajika::Interner {
        self.database.interner()
    }

    pub(crate) fn recovery_pending(&self) -> bool {
        self.database.recovery_pending()
    }

    /// Editor text can hide stale disk bytes. Recovery checks the saved baseline even when the
    /// previous request left an editor view installed in the database.
    pub(crate) fn stale_saved_paths(&self) -> ajika::Result<Vec<PathBuf>> {
        SourceInput::stale_saved_paths(&self.saved, &self.database.interner())
    }

    pub(crate) fn set_session_retention(&mut self, retention: ajika::Retention) {
        self.database.set_session_retention(retention);
    }

    pub(crate) fn clear_session(&mut self) {
        self.database.clear_session();
    }

    /// Run after operation borrows and the previous published generation have been released.
    /// Collection relocates Ajika's interner; source entries belong to us and need their own pass.
    /// Do not compact here: that explicit storage operation may rewrite every live payload.
    pub(crate) fn collect(&mut self) -> ajika::Result<()> {
        SourceInput::relocate(self.input_views())?;
        self.database.collect()?;
        Ok(())
    }

    fn input_views(&self) -> impl Iterator<Item = &ajika::Inputs> {
        std::iter::once(&self.saved).chain(self.editor.iter().map(|editor| &editor.inputs))
    }

    #[cfg(test)]
    pub(crate) fn statistics(&self) -> ajika::Statistics {
        self.database.statistics()
    }

    pub(crate) fn open(workspace: &WorkspaceMetadata, reuse_cache: bool) -> anyhow::Result<Self> {
        tracing::debug!(storage = ?STORAGE, reuse_cache, "opening project query storage");
        let database = match STORAGE {
            QueryStorage::Memory => QueryDatabase::memory(
                ajika::Inputs::default(),
                QueryServices::default(),
                RETENTION,
            ),
            QueryStorage::Disk(compression) => {
                let mut database = Self::open_disk(workspace)?;
                // Existing frames carry their own codec; this only changes future writes.
                database.set_compression(compression);
                if !reuse_cache {
                    database.invalidate()?;
                }
                database
            }
        };
        Ok(Self {
            saved: database.inputs().clone(),
            database,
            editor: None,
        })
    }

    fn open_disk(workspace: &WorkspaceMetadata) -> anyhow::Result<QueryDatabase> {
        let target_dir = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| workspace.cargo_target_dir().to_path_buf());
        let name = workspace
            .workspace_root()
            .file_name()
            .unwrap_or_else(|| OsStr::new("workspace"));
        let root = target_dir.join("rust_glancer").join(name).join("queries");
        // Separate cache slots let multiple editor engines analyze one workspace concurrently.
        for slot in 1..=1024 {
            match QueryDatabase::open(
                root.join(slot.to_string()),
                EPOCH,
                ajika::Inputs::default(),
                QueryServices::default(),
                RETENTION,
            ) {
                Ok(database) => return Ok(database),
                Err(error) => match error.kind() {
                    ajika::ErrorKind::Persistence(error)
                        if error.kind() == ajika::PersistenceErrorKind::InUse => {}
                    _ => return Err(error.into()),
                },
            }
        }
        anyhow::bail!("no free query cache slots under {}", root.display())
    }
}

impl MemorySize for ProjectQueries {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        // The query owner is the single accounting owner of the shared interner. Ajika does not
        // expose its full runtime heap; encoded cache/session budgets are not resident byte counts
        // and must not be added to this structural report as if they were allocations.
        recorder.scope("interner", |recorder| {
            recorder.record_approximate::<ajika::Interner>(self.database.interner().memory_bytes());
        });
        recorder.scope("inputs", |recorder| {
            // An editor view includes the saved baseline. Its table estimate already deduplicates
            // shared tables, while the source walk also includes saved entries hidden by edits.
            let inputs = self
                .editor
                .as_ref()
                .map_or(&self.saved, |editor| &editor.inputs);
            recorder.record_approximate::<ajika::Inputs>(inputs.table_bytes());
            SourceInput::record_memory(self.input_views(), recorder);
            if let Some(editor) = &self.editor {
                editor.revisions.record_memory_children(recorder);
            }
        });
    }
}

/// Repeated requests keep the same view, including saved files lazily captured during the first
/// request. A changed document set builds a fresh overlay of `ProjectQueries::saved`.
/// This caches the last supplied set; the caller still owns the list of open editor documents.
struct EditorInputs {
    revisions: BTreeMap<FileId, SourceRevision>,
    inputs: ajika::Inputs,
}
