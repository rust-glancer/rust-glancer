//! Capture saved changes, validate the corresponding queries, then publish source routing.

mod input;
mod workspace_graph;

use std::collections::BTreeMap;

use anyhow::Context as _;
use rg_def_map::CrateDependencies;
use rg_ir_model::FileId;
use rg_source::SourceInput;
use rg_std::{NormalizedPathBuf, UniqueVec};
use rg_workspace::WorkspaceMetadata;

pub use self::input::{AnalysisChangeSummary, SavedFileChange};
use self::workspace_graph::WorkspaceGraphChanges;
use crate::{Project, ProjectMemoryPurgePoint, indexing::build_state};

impl Project {
    pub fn reindex_workspace(&mut self) -> anyhow::Result<()> {
        self.publish_changes(&[], true)?;
        Ok(())
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

        let interner = self
            .queries
            .get_mut()
            .expect("project query runtime")
            .interner();
        let mut graph_changed = false;
        for change in saved.values() {
            let file = FileId::for_path(&interner, change.path())?;
            if WorkspaceGraphChanges::check(
                self.workspace(),
                self.state.sources.contains_collected_file(file),
                &self.cargo_metadata_config,
                change,
            ) == WorkspaceGraphChanges::Changed
            {
                graph_changed = true;
                break;
            }
        }
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
        let previous = &self.state;
        let queries = self.queries.get_mut().expect("project query runtime");
        let published = queries.update_saved(|database| {
            // Keep the accepted inputs for source-change comparison. The clone shares lazy
            // observations; it does not freeze an inventory of files discovered so far.
            let accepted = database.inputs().clone();
            let interner = database.interner();
            let changed_paths = changes
                .iter()
                .map(|change| FileId::for_path(&interner, change.path()))
                .collect::<ajika::Result<Vec<_>>>()?;
            let (targets, presence_changed) = database.update_inputs(|update| {
                let (targets, presence_changed) = if graph_changed {
                    SourceInput::forget_observations(update);
                    workspace.write_inputs(update)?;
                    (CrateDependencies::write_inputs(&workspace, update)?, false)
                } else {
                    let paths = changes
                        .iter()
                        .map(SavedFileChange::path)
                        .collect::<Vec<_>>();
                    let presence_changed =
                        SourceInput::refresh_saved_paths(&accepted, update, &interner, &paths)?;
                    (previous.targets.clone(), presence_changed)
                };
                for source in changes.iter().filter_map(SavedFileChange::captured_source) {
                    SourceInput::set_saved(update, &interner, source)?;
                }
                Ok((targets, presence_changed))
            })?;
            // Source publication is independent of module discovery. A lazy include can change
            // without appearing in the published source index at all.
            let source_changes = SourceInput::changed_files(&accepted, database.inputs())?;
            let candidate = build_state(database, workspace, targets)
                .context("validate saved project candidate")?;
            let summary = if graph_changed {
                let mut changed_files = source_changes.into_iter().collect::<UniqueVec<_>>();
                changed_files.extend(
                    changed_paths
                        .into_iter()
                        .filter(|&file| candidate.sources.contains_collected_file(file)),
                );
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
                    changed_files: changed_files.into_vec(),
                    changed_crates,
                }
            } else {
                candidate
                    .sources
                    .changes_from(&previous.sources, &source_changes)
            };
            // A path can change its referent while both files stay in the target.
            // A successful request must observe the new lookup even in that case.
            if graph_changed || presence_changed || !summary.is_empty() {
                return Ok(Some((candidate, summary)));
            }
            // Accept the refreshed saved inputs, but keep the published metadata and generation
            // when no source content, routing, or workspace configuration changed.
            Ok(None)
        })?;
        let Some((candidate, summary)) = published else {
            return Ok(AnalysisChangeSummary::default());
        };
        self.state = candidate;
        self.memory_hooks
            .purge(ProjectMemoryPurgePoint::AfterProjectChange);
        Ok(summary)
    }
}
