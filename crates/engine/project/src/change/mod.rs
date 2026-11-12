//! Capture saved changes, validate the corresponding queries, then publish source routing.

mod input;
mod workspace_graph;

use std::collections::BTreeMap;

use anyhow::Context as _;
use rg_def_map::CrateDependencies;
use rg_ir_model::PackageSlot;
use rg_source::{SourceInput, SourcePresence};
use rg_std::{NormalizedPathBuf, UniqueVec};
use rg_workspace::WorkspaceMetadata;

pub use self::input::{AnalysisChangeSummary, ChangedFile, SavedFileChange};
use self::workspace_graph::WorkspaceGraphChanges;
use crate::{Project, ProjectMemoryPurgePoint, indexing::build_state, state::ProjectState};

impl Project {
    pub fn reindex_workspace(&mut self) -> anyhow::Result<()> {
        self.publish_changes(&[], true)?;
        Ok(())
    }

    pub fn apply_change(
        &mut self,
        change: SavedFileChange,
    ) -> anyhow::Result<AnalysisChangeSummary> {
        self.apply_changes([change])
    }

    pub fn apply_changes(
        &mut self,
        changes: impl IntoIterator<Item = SavedFileChange>,
    ) -> anyhow::Result<AnalysisChangeSummary> {
        let mut saved = BTreeMap::new();
        let mut spellings = Vec::new();
        for change in changes {
            let change = match change {
                SavedFileChange::FsPath(path) => {
                    let path = std::path::absolute(path).context("resolve saved source path")?;
                    let canonical = NormalizedPathBuf::from_absolute(&path)
                        .with_context(|| format!("normalize {}", path.display()))?
                        .into_path_buf();
                    // A symlink can change targets while keeping its original module spelling.
                    if path != canonical {
                        spellings.push(path);
                    }
                    SavedFileChange::FsPath(canonical)
                }
                captured => captured,
            };
            let key = change.path().to_path_buf();
            if change.captured_source().is_some() || !saved.contains_key(&key) {
                saved.insert(key, change);
            }
        }
        if saved.is_empty() {
            return Ok(AnalysisChangeSummary::default());
        }

        let graph_changed = saved.values().any(|change| {
            WorkspaceGraphChanges::check(
                self.workspace(),
                self.state.parse_db(),
                &self.cargo_metadata_config,
                change,
            ) == WorkspaceGraphChanges::Changed
        });
        let mut changes = saved.into_values().collect::<Vec<_>>();
        changes.extend(spellings.into_iter().map(SavedFileChange::FsPath));
        self.publish_changes(&changes, graph_changed)
    }

    fn publish_changes(
        &mut self,
        changes: &[SavedFileChange],
        graph_changed: bool,
    ) -> anyhow::Result<AnalysisChangeSummary> {
        let workspace = if graph_changed {
            let manifest = self.workspace().workspace_root().join("Cargo.toml");
            let loaded = self
                .cargo_metadata_config
                .load_metadata_with_target_cfg(&manifest)
                .with_context(|| format!("load {}", manifest.display()))?;
            WorkspaceMetadata::lower(
                loaded.metadata,
                loaded.target_cfg,
                self.workspace_lowering_config.clone(),
            )
            .context("normalize refreshed Cargo metadata")?
            .with_sysroot_sources(self.workspace().sysroot_sources())
        } else {
            self.workspace().clone()
        };
        let database = self.database.get_mut().expect("project query runtime");
        // Lazy captures made during refresh may also become visible in the accepted inputs. Ajika
        // requires queries to work regardless of whether an initializer has already run.
        // Input changes advance the revision, so later queries will observe the updated state anyway.
        let accepted = database.inputs().clone();
        let interner = database.interner();
        let (targets, presence_changed) = database.update_inputs(|update| {
            let (targets, presence_changed) = if graph_changed {
                update.forget_all(SourceInput::CAPTURE);
                update.forget_all(SourcePresence::RESOLVED_FILE);
                workspace.write_inputs(update)?;
                (CrateDependencies::write_inputs(&workspace, update)?, false)
            } else {
                let paths = changes
                    .iter()
                    .map(SavedFileChange::path)
                    .collect::<Vec<_>>();
                let presence_changed =
                    SourceInput::refresh_saved_paths(&accepted, update, &interner, &paths)?;
                (self.state.targets.clone(), presence_changed)
            };
            for source in changes.iter().filter_map(SavedFileChange::captured_source) {
                SourceInput::set_saved(update, &interner, source)?;
            }
            Ok((targets, presence_changed))
        })?;
        let result = build_state(database, workspace, targets);
        match result {
            Ok(candidate) => {
                let summary = if graph_changed {
                    let changed_files = changes
                        .iter()
                        .flat_map(|change| candidate.parse.file_refs_for_path(change.path()))
                        .map(|file| ChangedFile {
                            package: file.package,
                            file: file.file,
                        })
                        .collect::<UniqueVec<_>>()
                        .into_vec();
                    let changed_crates = candidate
                        .targets
                        .iter()
                        .copied()
                        .filter(|krate| {
                            !candidate.workspace.packages()[krate.package.0]
                                .origin
                                .is_sysroot()
                        })
                        .collect::<Vec<_>>();
                    AnalysisChangeSummary {
                        changed_files,
                        changed_crates,
                    }
                } else {
                    Self::summarize_changes(&self.state, &candidate)
                };
                // A path can change its referent while both files stay in the target manifest.
                // A successful request must observe the new lookup even in that case.
                if graph_changed || presence_changed || !summary.is_empty() {
                    self.state = candidate;
                    self.memory_hooks
                        .purge(ProjectMemoryPurgePoint::AfterProjectChange);
                    return Ok(summary);
                }
            }
            Err(error) => {
                // Ajika can finish a checkpoint before the host learns that it cannot publish.
                // The previous inputs must validate any such records before serving a request.
                database.replace_inputs(accepted);
                return Err(error).context("validate saved project candidate");
            }
        }
        database.replace_inputs(accepted);
        Ok(AnalysisChangeSummary::default())
    }

    fn summarize_changes(before: &ProjectState, after: &ProjectState) -> AnalysisChangeSummary {
        let mut changed_files = UniqueVec::new();
        let mut changed_crates = UniqueVec::new();
        for (slot, (old, new)) in before
            .parse
            .packages()
            .iter()
            .zip(after.parse.packages())
            .enumerate()
        {
            let package = PackageSlot(slot);
            for file in old.parsed_files().chain(new.parsed_files()) {
                let id = file.file_id();
                if old.parsed_file(id).map(|file| file.source_revision())
                    != new.parsed_file(id).map(|file| file.source_revision())
                {
                    changed_files.push(ChangedFile { package, file: id });
                    changed_crates.extend(before.crates_for_file(package, id));
                    changed_crates.extend(after.crates_for_file(package, id));
                }
            }
        }
        // A shared file can leave one target while remaining in the same package's source union.
        changed_crates.extend(
            after
                .targets
                .iter()
                .copied()
                .filter(|krate| before.module_files.get(krate) != after.module_files.get(krate)),
        );
        AnalysisChangeSummary {
            changed_files: changed_files.into_vec(),
            changed_crates: changed_crates.into_vec(),
        }
    }
}
