use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{
        Arc,
        mpsc::{Receiver, Sender},
    },
    time::{Duration, Instant},
};

use anyhow::Context as _;
use rg_analysis::{
    Analysis as QueryAnalysis, CompletionQuery, InlayHint as AnalysisInlayHint, ReferenceQuery,
    RenameEdit, RenameTarget,
};
use rg_ir_model::TargetRef;
use rg_lsp_proto::{
    AnalysisConfig, CargoMetadataTarget as ProtoCargoMetadataTarget, CompletionClientCapabilities,
    IndexingPerformancePreference as ProtoIndexingPerformancePreference,
    PackageResidencyPolicy as ProtoPackageResidencyPolicy, ServiceNotification,
    SysrootDiscovery as ProtoSysrootDiscovery,
};
use rg_parse::TextSpan;
use rg_project::{
    FileContext, IndexingPerformancePreference, PackageResidencyPolicy, Project,
    ProjectMemoryHooks, ProjectSnapshot, SavedFileChange, SplitIndexingMode,
};
use rg_workspace::{
    CargoMetadataConfig, RustEdition, SysrootSources, WorkspaceLoweringConfig, WorkspaceMetadata,
};

use crate::{
    dirty_state::{DirtyDocumentIdentity, DirtyState},
    documents::DirtyDocumentSnapshot,
    engine::{
        QueuedEngineCommand,
        command::{EngineCommand, EngineResponse},
        early_start::{DeferredIndexingFinish, EarlyStart, ReferenceSearchPlan},
        project_proxy::ProjectProxy,
    },
    memory::{MemoryControl, MemoryReporter, ProjectMemoryReporter},
    project_stats::{ProjectStats, log_retained_memory},
    proto::{
        completion, formatting as formatting_proto, hover, inlay_hint, navigation, position,
        references, rename, symbols,
    },
    service::ServiceNotificationsSink,
};

#[derive(Debug)]
pub(super) struct EngineWorker {
    sender: Sender<QueuedEngineCommand>,
    project: ProjectProxy,
    deferred_indexing_finish: DeferredIndexingFinish,
    workspace_root: Option<PathBuf>,
    dirty_state: DirtyState,
    notifications: ServiceNotificationsSink,
    memory_control: Arc<dyn MemoryControl>,
    memory_hooks: Arc<dyn ProjectMemoryHooks>,
}

#[derive(Debug)]
struct QueryContext {
    label: &'static str,
    queue_elapsed: Duration,
    dirty_identity: Option<DirtyDocumentIdentity>,
}

impl QueryContext {
    fn new(label: &'static str, queue_elapsed: Duration) -> Self {
        Self {
            label,
            queue_elapsed,
            dirty_identity: None,
        }
    }

    fn document(
        label: &'static str,
        queue_elapsed: Duration,
        dirty: Option<&DirtyDocumentSnapshot>,
    ) -> Self {
        Self {
            label,
            queue_elapsed,
            dirty_identity: dirty.map(DirtyDocumentIdentity::from_snapshot),
        }
    }

    fn stale_dirty_identity(&self, dirty_state: &DirtyState) -> Option<&DirtyDocumentIdentity> {
        self.dirty_identity
            .as_ref()
            .filter(|identity| !dirty_state.is_current_identity(identity))
    }
}

impl EngineWorker {
    pub(super) fn new(
        sender: Sender<QueuedEngineCommand>,
        memory_control: Arc<dyn MemoryControl>,
        dirty_state: DirtyState,
        notifications: ServiceNotificationsSink,
    ) -> Self {
        let memory_hooks = Arc::new(ProjectMemoryReporter::new(Arc::clone(&memory_control)));
        Self {
            sender,
            project: ProjectProxy::new(Arc::clone(&memory_control)),
            deferred_indexing_finish: DeferredIndexingFinish::new(),
            workspace_root: None,
            dirty_state,
            notifications,
            memory_control,
            memory_hooks,
        }
    }

    pub(super) fn run(mut self, receiver: Receiver<QueuedEngineCommand>) {
        tracing::debug!("LSP engine worker started");
        let mut deferred_commands = VecDeque::new();

        loop {
            let queued = match deferred_commands.pop_front() {
                Some(queued) => queued,
                None => match receiver.recv() {
                    Ok(queued) => queued,
                    Err(_) => break,
                },
            };
            let queue_elapsed = queued.enqueued_at.elapsed();
            let command = queued.command;
            match command {
                EngineCommand::Initialize {
                    root,
                    analysis,
                    respond_to,
                } => {
                    tracing::trace!(root = %root.display(), "engine command started: initialize");
                    let _ = respond_to.send(self.initialize(root, analysis));
                }
                EngineCommand::ProjectPathsChanged { paths, respond_to } => {
                    let (paths, responders) = Self::collect_project_path_changes(
                        paths,
                        respond_to,
                        &receiver,
                        &mut deferred_commands,
                    );
                    tracing::trace!(
                        path_count = paths.len(),
                        request_count = responders.len(),
                        "engine command started: project_paths_changed"
                    );
                    Self::respond_to_project_path_changes(
                        responders,
                        self.project_paths_changed(paths),
                    );
                }
                EngineCommand::GotoDefinition {
                    path,
                    position,
                    dirty,
                    respond_to,
                } => {
                    tracing::trace!(
                        path = %path.display(),
                        line = position.line,
                        character = position.character,
                        "engine command started: goto_definition"
                    );
                    let context =
                        QueryContext::document("goto_definition", queue_elapsed, dirty.as_ref());
                    self.respond_to_query(context, respond_to, |worker| {
                        worker.goto_definition(path, position, dirty)
                    });
                }
                EngineCommand::GotoTypeDefinition {
                    path,
                    position,
                    dirty,
                    respond_to,
                } => {
                    tracing::trace!(
                        path = %path.display(),
                        line = position.line,
                        character = position.character,
                        "engine command started: goto_type_definition"
                    );
                    let context = QueryContext::document(
                        "goto_type_definition",
                        queue_elapsed,
                        dirty.as_ref(),
                    );
                    self.respond_to_query(context, respond_to, |worker| {
                        worker.goto_type_definition(path, position, dirty)
                    });
                }
                EngineCommand::GotoImplementation {
                    path,
                    position,
                    dirty,
                    respond_to,
                } => {
                    tracing::trace!(
                        path = %path.display(),
                        line = position.line,
                        character = position.character,
                        "engine command started: goto_implementation"
                    );
                    let context = QueryContext::document(
                        "goto_implementation",
                        queue_elapsed,
                        dirty.as_ref(),
                    );
                    self.respond_to_query(context, respond_to, |worker| {
                        worker.goto_implementation(path, position, dirty)
                    });
                }
                EngineCommand::References {
                    path,
                    position,
                    include_declaration,
                    dirty,
                    respond_to,
                } => {
                    tracing::trace!(
                        path = %path.display(),
                        line = position.line,
                        character = position.character,
                        include_declaration,
                        "engine command started: references"
                    );
                    let context =
                        QueryContext::document("references", queue_elapsed, dirty.as_ref());
                    self.respond_to_query(context, respond_to, |worker| {
                        worker.references(path, position, include_declaration, dirty)
                    });
                }
                EngineCommand::PrepareRename {
                    path,
                    position,
                    dirty,
                    respond_to,
                } => {
                    tracing::trace!(
                        path = %path.display(),
                        line = position.line,
                        character = position.character,
                        "engine command started: prepare_rename"
                    );
                    let context =
                        QueryContext::document("prepare_rename", queue_elapsed, dirty.as_ref());
                    self.respond_to_query(context, respond_to, |worker| {
                        worker.prepare_rename(path, position, dirty)
                    });
                }
                EngineCommand::Rename {
                    path,
                    position,
                    new_name,
                    dirty,
                    respond_to,
                } => {
                    tracing::trace!(
                        path = %path.display(),
                        line = position.line,
                        character = position.character,
                        new_name = %new_name,
                        "engine command started: rename"
                    );
                    let context = QueryContext::document("rename", queue_elapsed, dirty.as_ref());
                    self.respond_to_query(context, respond_to, |worker| {
                        worker.rename(path, position, new_name, dirty)
                    });
                }
                EngineCommand::DocumentHighlight {
                    path,
                    position,
                    dirty,
                    respond_to,
                } => {
                    tracing::trace!(
                        path = %path.display(),
                        line = position.line,
                        character = position.character,
                        "engine command started: document_highlight"
                    );
                    let context =
                        QueryContext::document("document_highlight", queue_elapsed, dirty.as_ref());
                    self.respond_to_query(context, respond_to, |worker| {
                        worker.document_highlight(path, position, dirty)
                    });
                }
                EngineCommand::Hover {
                    path,
                    position,
                    dirty,
                    respond_to,
                } => {
                    tracing::trace!(
                        path = %path.display(),
                        line = position.line,
                        character = position.character,
                        "engine command started: hover"
                    );
                    let context = QueryContext::document("hover", queue_elapsed, dirty.as_ref());
                    self.respond_to_query(context, respond_to, |worker| {
                        worker.hover(path, position, dirty)
                    });
                }
                EngineCommand::Completion {
                    path,
                    position,
                    client_capabilities,
                    dirty,
                    respond_to,
                } => {
                    tracing::trace!(
                        path = %path.display(),
                        line = position.line,
                        character = position.character,
                        "engine command started: completion"
                    );
                    let context =
                        QueryContext::document("completion", queue_elapsed, dirty.as_ref());
                    self.respond_to_query(context, respond_to, |worker| {
                        worker.completion(path, position, client_capabilities, dirty)
                    });
                }
                EngineCommand::Formatting {
                    path,
                    text,
                    respond_to,
                } => {
                    tracing::trace!(
                        path = %path.display(),
                        "engine command started: formatting"
                    );
                    let context = QueryContext::new("formatting", queue_elapsed);
                    self.respond_to_query(context, respond_to, |worker| {
                        worker.formatting(path, text)
                    });
                }
                EngineCommand::DocumentSymbol {
                    path,
                    dirty,
                    respond_to,
                } => {
                    tracing::trace!(
                        path = %path.display(),
                        "engine command started: document_symbol"
                    );
                    let context =
                        QueryContext::document("document_symbol", queue_elapsed, dirty.as_ref());
                    self.respond_to_query(context, respond_to, |worker| {
                        worker.document_symbol(path, dirty)
                    });
                }
                EngineCommand::InlayHint {
                    path,
                    range,
                    dirty,
                    respond_to,
                } => {
                    tracing::trace!(
                        path = %path.display(),
                        start_line = range.start.line,
                        start_character = range.start.character,
                        end_line = range.end.line,
                        end_character = range.end.character,
                        "engine command started: inlay_hint"
                    );
                    let context =
                        QueryContext::document("inlay_hint", queue_elapsed, dirty.as_ref());
                    self.respond_to_query(context, respond_to, |worker| {
                        worker.inlay_hint(path, range, dirty)
                    });
                }
                EngineCommand::WorkspaceSymbol { query, respond_to } => {
                    tracing::trace!(query = %query, "engine command started: workspace_symbol");
                    let context = QueryContext::new("workspace_symbol", queue_elapsed);
                    self.respond_to_query(context, respond_to, |worker| {
                        worker.workspace_symbol(&query)
                    });
                }
                EngineCommand::ReindexWorkspace { respond_to } => {
                    tracing::trace!("engine command started: reindex_workspace");
                    let _ = respond_to.send(self.reindex_workspace());
                }
                EngineCommand::DeferredIndexingFinished { generation, result } => {
                    tracing::trace!(
                        generation,
                        "engine command started: deferred_indexing_finished"
                    );
                    let current_generation_finished = self
                        .deferred_indexing_finish
                        .finish_returned(&self.sender, &mut self.project, generation, result);
                    if current_generation_finished {
                        self.send_deferred_indexing_finished();
                        if let Ok(snapshot) = self.project.saved_snapshot() {
                            Self::log_project_snapshot(snapshot, "deferred indexing finish");
                        }
                    }
                }
                EngineCommand::Shutdown(respond_to) => {
                    tracing::info!("shutting down LSP engine worker");
                    let _ = respond_to.send(Ok(()));
                    break;
                }
            }
        }

        tracing::debug!("LSP engine worker stopped");
    }

    fn collect_project_path_changes(
        paths: Vec<PathBuf>,
        respond_to: EngineResponse<()>,
        receiver: &Receiver<QueuedEngineCommand>,
        deferred_commands: &mut VecDeque<QueuedEngineCommand>,
    ) -> (Vec<PathBuf>, Vec<EngineResponse<()>>) {
        let mut paths = paths;
        let mut responders = vec![respond_to];

        // Native watcher batches can arrive back-to-back while a large filesystem operation is
        // still settling. Merge immediately adjacent project-change commands before the worker
        // starts rebuilding, but stop as soon as an interactive command appears so command ordering
        // stays predictable.
        while let Ok(queued) = receiver.try_recv() {
            match queued.command {
                EngineCommand::ProjectPathsChanged {
                    paths: next_paths,
                    respond_to,
                } => {
                    paths.extend(next_paths);
                    responders.push(respond_to);
                }
                command => {
                    deferred_commands.push_back(QueuedEngineCommand {
                        command,
                        enqueued_at: queued.enqueued_at,
                    });
                    break;
                }
            }
        }

        paths.sort();
        paths.dedup();
        (paths, responders)
    }

    fn respond_to_project_path_changes(
        responders: Vec<EngineResponse<()>>,
        result: anyhow::Result<()>,
    ) {
        match result {
            Ok(()) => {
                for respond_to in responders {
                    let _ = respond_to.send(Ok(()));
                }
            }
            Err(error) => {
                let error_message = format!("{error:#}");
                let mut responders = responders.into_iter();
                if let Some(respond_to) = responders.next() {
                    let _ = respond_to.send(Err(error));
                }
                for respond_to in responders {
                    let _ = respond_to.send(Err(anyhow::anyhow!(error_message.clone())));
                }
            }
        }
    }

    fn initialize(&mut self, root: PathBuf, analysis: AnalysisConfig) -> anyhow::Result<()> {
        // Keep protocol DTOs out of the project layer. The engine boundary is where client-facing
        // configuration becomes concrete workspace/project configuration.
        let package_residency_policy = match analysis.package_residency_policy {
            ProtoPackageResidencyPolicy::AllResident => PackageResidencyPolicy::AllResident,
            ProtoPackageResidencyPolicy::WorkspaceResident => {
                PackageResidencyPolicy::WorkspaceResident
            }
            ProtoPackageResidencyPolicy::WorkspaceAndPathDepsResident => {
                PackageResidencyPolicy::WorkspaceAndPathDepsResident
            }
            ProtoPackageResidencyPolicy::WorkspacePathAndDirectDepsResident => {
                PackageResidencyPolicy::WorkspacePathAndDirectDepsResident
            }
            ProtoPackageResidencyPolicy::AllOffloadable => PackageResidencyPolicy::AllOffloadable,
        };
        let cargo_metadata_config = match analysis.cargo_metadata_config.target() {
            ProtoCargoMetadataTarget::Auto => CargoMetadataConfig::default(),
            ProtoCargoMetadataTarget::Triple(target) => {
                CargoMetadataConfig::default().target_triple(target.as_str())
            }
        }
        .all_features(analysis.cargo_metadata_config.all_features_enabled())
        .no_default_features(analysis.cargo_metadata_config.no_default_features_enabled())
        .custom_features(analysis.cargo_metadata_config.features().iter().cloned());
        let workspace_lowering_config = WorkspaceLoweringConfig::default()
            .cfg_test(analysis.cfg.test)
            .custom_cfg_atoms(analysis.cfg.atoms.iter().cloned());
        let indexing_preference = match analysis.indexing_preference {
            ProtoIndexingPerformancePreference::LowerPeakMemory => {
                IndexingPerformancePreference::LowerPeakMemory
            }
            ProtoIndexingPerformancePreference::FasterBuilds => {
                IndexingPerformancePreference::FasterBuilds
            }
        };
        let started = Instant::now();
        let configured_target = match analysis.cargo_metadata_config.target() {
            ProtoCargoMetadataTarget::Auto => "auto",
            ProtoCargoMetadataTarget::Triple(target) => target.as_str(),
        };
        tracing::info!(
            root = %root.display(),
            package_residency = analysis.package_residency_policy.config_name(),
            indexing_preference = analysis.indexing_preference.config_name(),
            cargo_target = configured_target,
            cargo_all_features = analysis.cargo_metadata_config.all_features_enabled(),
            cargo_no_default_features = analysis.cargo_metadata_config.no_default_features_enabled(),
            cargo_features = ?analysis.cargo_metadata_config.features(),
            cfg_test = analysis.cfg.test,
            cfg_atoms = ?analysis.cfg.atoms,
            "starting workspace indexing"
        );

        let manifest_path = root.join("Cargo.toml");
        if !manifest_path.exists() {
            anyhow::bail!(
                "workspace root {} does not contain Cargo.toml",
                root.display()
            );
        }

        let metadata_started = Instant::now();
        let metadata = cargo_metadata_config
            .load_metadata_with_target_cfg(&manifest_path)
            .context("while attempting to run cargo metadata for LSP initialization")?;
        tracing::info!(
            package_count = metadata.metadata.packages.len(),
            elapsed_ms = metadata_started.elapsed().as_millis(),
            "cargo metadata finished"
        );

        let workspace = WorkspaceMetadata::lower(
            metadata.metadata,
            metadata.target_cfg,
            workspace_lowering_config.clone(),
        )
        .context("while attempting to normalize Cargo metadata")?;
        let workspace_root = workspace.workspace_root().to_path_buf();
        let sysroot = match analysis.sysroot_discovery {
            ProtoSysrootDiscovery::Auto => SysrootSources::discover(workspace.workspace_root()),
            ProtoSysrootDiscovery::Disabled => None,
        };
        match &sysroot {
            Some(sysroot) => {
                tracing::info!(
                    library_root = %sysroot.library_root().display(),
                    "sysroot sources discovered"
                );
            }
            None => {
                tracing::info!("sysroot sources unavailable");
            }
        }

        let workspace = workspace.with_sysroot_sources(sysroot);
        let project = Project::builder(workspace)
            .workspace_lowering_config(workspace_lowering_config)
            .cargo_metadata_config(cargo_metadata_config)
            .indexing_preference(indexing_preference)
            .split_indexing_mode(SplitIndexingMode::EarlyStart)
            .package_residency_policy(package_residency_policy)
            .memory_hooks(Arc::clone(&self.memory_hooks))
            .build()
            .context("while attempting to build LSP analysis project")?;
        self.workspace_root = Some(workspace_root.clone());
        let detached = project.detach_split_indexing();
        let generation = self.project.replace_saved(project);
        Self::log_project_snapshot(self.project.saved_snapshot()?, "initial early-start index");
        tracing::info!(
            workspace_root = %workspace_root.display(),
            elapsed_ms = started.elapsed().as_millis(),
            "workspace early-start indexing finished"
        );
        self.deferred_indexing_finish
            .start_initial(&self.sender, generation, detached);

        Ok(())
    }

    fn reindex_workspace(&mut self) -> anyhow::Result<()> {
        let started = Instant::now();

        tracing::info!("manual workspace reindex started");
        self.mutate_saved_and_schedule_deferred_finish(|project| {
            project
                .reindex_workspace()
                .context("while attempting to manually reindex workspace")
        })?;
        Self::log_project_snapshot(self.project.saved_snapshot()?, "manual reindex");
        tracing::info!(
            elapsed_ms = started.elapsed().as_millis(),
            "manual workspace reindex finished"
        );

        Ok(())
    }

    fn project_paths_changed(&mut self, paths: Vec<PathBuf>) -> anyhow::Result<()> {
        let started = Instant::now();

        tracing::info!(path_count = paths.len(), "processing project path changes");
        if paths.is_empty() {
            tracing::info!(
                applied_changes = 0usize,
                changed_files = 0usize,
                affected_packages = 0usize,
                changed_targets = 0usize,
                elapsed_ms = started.elapsed().as_millis(),
                "project path reindex finished"
            );
            return Ok(());
        }

        let applied_changes = paths.len();
        let summary = self.mutate_saved_and_schedule_deferred_finish(|project| {
            project
                .apply_changes(paths.into_iter().map(SavedFileChange::new))
                .context("while attempting to apply project path changes")
        })?;
        let changed_files = summary.changed_files.len();
        let affected_packages = summary.affected_packages.len();
        let changed_targets = summary.changed_targets.len();

        tracing::info!(
            applied_changes,
            changed_files,
            affected_packages,
            changed_targets,
            elapsed_ms = started.elapsed().as_millis(),
            "project path reindex finished"
        );
        if applied_changes > 0 {
            Self::log_project_snapshot(
                self.project.saved_snapshot()?,
                "after project path changes",
            );
        }

        Ok(())
    }

    fn mutate_saved_and_schedule_deferred_finish<T>(
        &mut self,
        mutation: impl FnOnce(&mut Project) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let result = self.project.mutate_saved(mutation);
        if self.project.is_initialized() {
            self.deferred_indexing_finish
                .saved_project_changed(&self.sender, &self.project);
        }
        result
    }

    fn send_deferred_indexing_finished(&self) {
        let Some(root) = &self.workspace_root else {
            tracing::warn!("deferred indexing finished before workspace root was recorded");
            return;
        };

        self.notifications
            .send(ServiceNotification::DeferredIndexingFinished { root: root.clone() });
    }

    fn reference_search_plans_for_position(
        &mut self,
        path: &Path,
        position: ls_types::Position,
        dirty: Option<&DirtyDocumentSnapshot>,
    ) -> anyhow::Result<Vec<ReferenceSearchPlan>> {
        self.project.with_query_snapshot(dirty, |snapshot| {
            let target_offsets = Self::target_offsets(snapshot, path, position)?;
            let analysis = snapshot.full_analysis()?;
            let mut plans = Vec::new();

            for (context, target, offset) in target_offsets {
                plans.push(Self::reference_search_plan(
                    snapshot, &analysis, &context, target, offset,
                )?);
            }

            Ok(plans)
        })
    }

    fn goto_definition(
        &mut self,
        path: PathBuf,
        position: ls_types::Position,
        dirty: Option<DirtyDocumentSnapshot>,
    ) -> anyhow::Result<Vec<ls_types::Location>> {
        self.navigation_query(path, position, NavigationQuery::Definition, dirty)
    }

    fn goto_type_definition(
        &mut self,
        path: PathBuf,
        position: ls_types::Position,
        dirty: Option<DirtyDocumentSnapshot>,
    ) -> anyhow::Result<Vec<ls_types::Location>> {
        self.navigation_query(path, position, NavigationQuery::TypeDefinition, dirty)
    }

    fn goto_implementation(
        &mut self,
        path: PathBuf,
        position: ls_types::Position,
        dirty: Option<DirtyDocumentSnapshot>,
    ) -> anyhow::Result<Vec<ls_types::Location>> {
        self.navigation_query(path, position, NavigationQuery::Implementation, dirty)
    }

    fn references(
        &mut self,
        path: PathBuf,
        position: ls_types::Position,
        include_declaration: bool,
        dirty: Option<DirtyDocumentSnapshot>,
    ) -> anyhow::Result<Vec<ls_types::Location>> {
        let started = Instant::now();
        EarlyStart::ensure_path(&mut self.project, "references", &path)?;
        let search_plans =
            self.reference_search_plans_for_position(&path, position, dirty.as_ref())?;
        EarlyStart::ensure_reference_plans(&mut self.project, "references", &search_plans)?;
        // Dirty overlays lower Body IR only for dirty files. Cross-file body references are
        // intentionally best-effort until the buffer is saved and the normal project catches up.
        let locations = self
            .project
            .with_query_snapshot(dirty.as_ref(), |snapshot| {
                let target_offsets = Self::target_offsets(snapshot, &path, position)?;
                let analysis = snapshot.full_analysis()?;
                let mut locations = Vec::new();

                for (context, target, offset) in target_offsets {
                    let search_plan =
                        Self::reference_search_plan(snapshot, &analysis, &context, target, offset)?;
                    let reference_query = search_plan.query(include_declaration);

                    for reference in
                        analysis.references(target, context.file, offset, reference_query)?
                    {
                        let Some(location) =
                            references::location_for_reference(snapshot, &reference)?
                        else {
                            continue;
                        };
                        if !locations.contains(&location) {
                            locations.push(location);
                        }
                    }
                }

                Ok(locations)
            })?;

        tracing::trace!(
            path = %path.display(),
            line = position.line,
            character = position.character,
            include_declaration,
            result_count = locations.len(),
            elapsed_ms = started.elapsed().as_millis(),
            "references query finished"
        );

        Ok(locations)
    }

    fn prepare_rename(
        &mut self,
        path: PathBuf,
        position: ls_types::Position,
        dirty: Option<DirtyDocumentSnapshot>,
    ) -> anyhow::Result<Option<ls_types::PrepareRenameResponse>> {
        let started = Instant::now();
        EarlyStart::ensure_path(&mut self.project, "prepare_rename", &path)?;
        let response = self
            .project
            .with_query_snapshot(dirty.as_ref(), |snapshot| {
                let target_offsets = Self::target_offsets(snapshot, &path, position)?;
                let analysis_targets = target_offsets
                    .iter()
                    .map(|(_, target, _)| *target)
                    .collect::<Vec<_>>();
                let analysis = snapshot.analysis_for_targets(&analysis_targets)?;

                for (context, target, offset) in target_offsets {
                    if !snapshot.package_is_workspace_member(context.package) {
                        continue;
                    }
                    let Some(rename_target) =
                        analysis.prepare_rename(target, context.file, offset)?
                    else {
                        continue;
                    };
                    if !Self::rename_target_matches_source(
                        snapshot,
                        context.package,
                        &rename_target,
                    ) {
                        continue;
                    }

                    return rename::prepare_rename(snapshot, context.package, rename_target)
                        .map(Some);
                }

                Ok(None)
            })?;

        tracing::trace!(
            path = %path.display(),
            line = position.line,
            character = position.character,
            has_result = response.is_some(),
            elapsed_ms = started.elapsed().as_millis(),
            "prepare rename query finished"
        );

        Ok(response)
    }

    fn rename(
        &mut self,
        path: PathBuf,
        position: ls_types::Position,
        new_name: String,
        dirty: Option<DirtyDocumentSnapshot>,
    ) -> anyhow::Result<Option<ls_types::WorkspaceEdit>> {
        let started = Instant::now();
        EarlyStart::ensure_path(&mut self.project, "rename", &path)?;
        let search_plans =
            self.reference_search_plans_for_position(&path, position, dirty.as_ref())?;
        EarlyStart::ensure_reference_plans(&mut self.project, "rename", &search_plans)?;
        let edit = self
            .project
            .with_query_snapshot(dirty.as_ref(), |snapshot| {
                let target_offsets = Self::target_offsets(snapshot, &path, position)?;
                let analysis = snapshot.full_analysis()?;
                let mut edits = Vec::new();

                for (context, target, offset) in target_offsets {
                    if !snapshot.package_is_workspace_member(context.package) {
                        continue;
                    }
                    let search_plan =
                        Self::reference_search_plan(snapshot, &analysis, &context, target, offset)?;
                    let reference_query = search_plan.query(true);
                    let Some(rename_result) = analysis.rename(
                        target,
                        context.file,
                        offset,
                        &new_name,
                        reference_query,
                    )?
                    else {
                        continue;
                    };

                    if !Self::rename_target_matches_source(
                        snapshot,
                        context.package,
                        &rename_result.target,
                    ) {
                        continue;
                    }
                    for edit in rename_result.edits {
                        if !edits.contains(&edit) {
                            edits.push(edit);
                        }
                    }
                }

                let Some(edits) = Self::verified_rename_edits(snapshot, edits) else {
                    return Ok(None);
                };
                if edits.is_empty() {
                    return Ok(None);
                }
                rename::workspace_edit(snapshot, edits).map(Some)
            })?;

        tracing::trace!(
            path = %path.display(),
            line = position.line,
            character = position.character,
            new_name = %new_name,
            has_edit = edit.is_some(),
            elapsed_ms = started.elapsed().as_millis(),
            "rename query finished"
        );

        Ok(edit)
    }

    fn document_highlight(
        &mut self,
        path: PathBuf,
        position: ls_types::Position,
        dirty: Option<DirtyDocumentSnapshot>,
    ) -> anyhow::Result<Vec<ls_types::DocumentHighlight>> {
        let started = Instant::now();
        EarlyStart::ensure_path(&mut self.project, "document_highlight", &path)?;
        let highlights = self
            .project
            .with_query_snapshot(dirty.as_ref(), |snapshot| {
                let target_offsets = Self::target_offsets(snapshot, &path, position)?;

                let analysis_targets = target_offsets
                    .iter()
                    .map(|(_, target, _)| *target)
                    .collect::<Vec<_>>();
                let analysis = snapshot.analysis_for_targets(&analysis_targets)?;
                let mut highlights = Vec::new();

                for (context, target, offset) in target_offsets {
                    for reference in analysis.references(
                        target,
                        context.file,
                        offset,
                        ReferenceQuery::file_scoped(target, context.file),
                    )? {
                        if reference.target.package != context.package
                            || reference.file_id != context.file
                        {
                            continue;
                        }

                        let highlight = references::document_highlight_for_reference(
                            snapshot,
                            context.package,
                            context.file,
                            reference.span,
                        )?;
                        if !highlights.contains(&highlight) {
                            highlights.push(highlight);
                        }
                    }
                }

                Ok(highlights)
            })?;

        tracing::trace!(
            path = %path.display(),
            line = position.line,
            character = position.character,
            result_count = highlights.len(),
            elapsed_ms = started.elapsed().as_millis(),
            "document highlight query finished"
        );

        Ok(highlights)
    }

    fn completion(
        &mut self,
        path: PathBuf,
        position: ls_types::Position,
        client_capabilities: CompletionClientCapabilities,
        dirty: Option<DirtyDocumentSnapshot>,
    ) -> anyhow::Result<Vec<ls_types::CompletionItem>> {
        let started = Instant::now();
        EarlyStart::ensure_path(&mut self.project, "completion", &path)?;
        let source_text = dirty.as_ref().map(DirtyDocumentSnapshot::text);
        let completions = self
            .project
            .with_query_snapshot(dirty.as_ref(), |snapshot| {
                let target_offsets = Self::target_offsets(snapshot, &path, position)?;
                let analysis_targets = target_offsets
                    .iter()
                    .map(|(_, target, _)| *target)
                    .collect::<Vec<_>>();
                let analysis = snapshot.analysis_for_targets(&analysis_targets)?;
                let mut completions = Vec::new();

                for (context, target, offset) in target_offsets {
                    let Some(line_index) = snapshot.file_line_index(context.package, context.file)
                    else {
                        continue;
                    };
                    let mut query = CompletionQuery::new(target, context.file, offset)
                        .with_client_capabilities(rg_analysis::CompletionClientCapabilities {
                            snippet_support: client_capabilities.snippet_support,
                        });
                    if let Some(source_text) = source_text {
                        query = query.with_source_text(source_text);
                    }
                    for item in analysis.completions_at(query)? {
                        let item = completion::completion_item(item, line_index);
                        if !completions.contains(&item) {
                            completions.push(item);
                        }
                    }
                }

                Ok(completions)
            })?;

        tracing::trace!(
            path = %path.display(),
            line = position.line,
            character = position.character,
            result_count = completions.len(),
            elapsed_ms = started.elapsed().as_millis(),
            "completion query finished"
        );

        Ok(completions)
    }

    fn hover(
        &mut self,
        path: PathBuf,
        position: ls_types::Position,
        dirty: Option<DirtyDocumentSnapshot>,
    ) -> anyhow::Result<Option<ls_types::Hover>> {
        let started = Instant::now();
        EarlyStart::ensure_path(&mut self.project, "hover", &path)?;
        let hover = self
            .project
            .with_query_snapshot(dirty.as_ref(), |snapshot| {
                let target_offsets = Self::target_offsets(snapshot, &path, position)?;
                let analysis_targets = target_offsets
                    .iter()
                    .map(|(_, target, _)| *target)
                    .collect::<Vec<_>>();
                let analysis = snapshot.analysis_for_targets(&analysis_targets)?;

                for (context, target, offset) in target_offsets {
                    let Some(info) = analysis.hover(target, context.file, offset)? else {
                        continue;
                    };
                    let Some(line_index) = snapshot.file_line_index(context.package, context.file)
                    else {
                        continue;
                    };
                    let Some(hover) = hover::hover(info, line_index) else {
                        continue;
                    };
                    return Ok(Some(hover));
                }

                Ok(None)
            })?;

        tracing::trace!(
            path = %path.display(),
            line = position.line,
            character = position.character,
            has_hover = hover.is_some(),
            elapsed_ms = started.elapsed().as_millis(),
            "hover query finished"
        );
        Ok(hover)
    }

    fn document_symbol(
        &mut self,
        path: PathBuf,
        dirty: Option<DirtyDocumentSnapshot>,
    ) -> anyhow::Result<Vec<ls_types::DocumentSymbol>> {
        let started = Instant::now();
        let lsp_symbols = self
            .project
            .with_query_snapshot(dirty.as_ref(), |snapshot| {
                let contexts = Self::file_contexts(snapshot, &path)?;
                let analysis_targets = contexts
                    .iter()
                    .flat_map(|context| context.targets.iter().copied())
                    .collect::<Vec<_>>();
                let analysis = snapshot.analysis_for_targets(&analysis_targets)?;
                let mut lsp_symbols = Vec::new();

                for context in contexts {
                    for target in context.targets {
                        let symbols = analysis.document_symbols(target, context.file)?;
                        for symbol in symbols {
                            let symbol =
                                symbols::document_symbol(snapshot, context.package, symbol)?;
                            if !lsp_symbols.contains(&symbol) {
                                lsp_symbols.push(symbol);
                            }
                        }
                    }
                }

                Ok(lsp_symbols)
            })?;

        tracing::trace!(
            path = %path.display(),
            result_count = lsp_symbols.len(),
            elapsed_ms = started.elapsed().as_millis(),
            "document symbol query finished"
        );

        Ok(lsp_symbols)
    }

    fn formatting(
        &mut self,
        path: PathBuf,
        text: Arc<str>,
    ) -> anyhow::Result<Vec<ls_types::TextEdit>> {
        let started = Instant::now();
        let edition = {
            let snapshot = self.project.saved_snapshot()?;
            let contexts = Self::file_contexts(snapshot, &path)?;

            // Some routed documents may not map to package metadata. We use an explicit fallback
            // here so formatting can still run without reading Cargo.toml from disk.
            contexts
                .first()
                .and_then(|context| snapshot.package_edition(context.package))
                .unwrap_or(RustEdition::Edition2024)
        };
        let formatted_text = crate::formatting::rustfmt(text.as_ref(), edition)?;
        let edits = formatting_proto::document_edits(text.as_ref(), formatted_text)?;

        tracing::trace!(
            path = %path.display(),
            edition = %edition,
            edit_count = edits.len(),
            elapsed_ms = started.elapsed().as_millis(),
            "formatting query finished"
        );

        Ok(edits)
    }

    fn inlay_hint(
        &mut self,
        path: PathBuf,
        range: ls_types::Range,
        dirty: Option<DirtyDocumentSnapshot>,
    ) -> anyhow::Result<Vec<ls_types::InlayHint>> {
        let started = Instant::now();
        EarlyStart::ensure_path(&mut self.project, "inlay_hint", &path)?;
        let lsp_hints = self
            .project
            .with_query_snapshot(dirty.as_ref(), |snapshot| {
                let contexts = Self::file_contexts(snapshot, &path)?;
                let analysis_targets = contexts
                    .iter()
                    .flat_map(|context| context.targets.iter().copied())
                    .collect::<Vec<_>>();
                let analysis = snapshot.analysis_for_targets(&analysis_targets)?;
                let mut hints = Vec::<(rg_def_map::PackageSlot, AnalysisInlayHint)>::new();

                for context in contexts {
                    let Some(range) = Self::text_span_for_context(snapshot, &context, range) else {
                        continue;
                    };

                    for target in context.targets {
                        for hint in analysis.inlay_hints(target, context.file, Some(range))? {
                            if !hints
                                .iter()
                                .any(|(_, existing_hint)| existing_hint == &hint)
                            {
                                hints.push((context.package, hint));
                            }
                        }
                    }
                }

                let mut lsp_hints = Vec::new();
                for (package, hint) in hints {
                    let Some(hint) = inlay_hint::inlay_hint(snapshot, package, hint)? else {
                        continue;
                    };
                    lsp_hints.push(hint);
                }

                Ok(lsp_hints)
            })?;

        tracing::trace!(
            path = %path.display(),
            result_count = lsp_hints.len(),
            elapsed_ms = started.elapsed().as_millis(),
            "inlay hint query finished"
        );

        Ok(lsp_hints)
    }

    fn workspace_symbol(&self, query: &str) -> anyhow::Result<Vec<ls_types::WorkspaceSymbol>> {
        let started = Instant::now();
        let snapshot = self.project.saved_snapshot()?;
        let analysis = snapshot.full_analysis()?;
        let mut lsp_symbols = Vec::new();

        for symbol in analysis.workspace_symbols(query)? {
            let Some(symbol) = symbols::workspace_symbol(snapshot, symbol)? else {
                continue;
            };
            if !lsp_symbols.contains(&symbol) {
                lsp_symbols.push(symbol);
            }
        }

        tracing::trace!(
            query,
            result_count = lsp_symbols.len(),
            elapsed_ms = started.elapsed().as_millis(),
            "workspace symbol query finished"
        );

        Ok(lsp_symbols)
    }

    fn navigation_query(
        &mut self,
        path: PathBuf,
        position: ls_types::Position,
        query: NavigationQuery,
        dirty: Option<DirtyDocumentSnapshot>,
    ) -> anyhow::Result<Vec<ls_types::Location>> {
        let started = Instant::now();
        EarlyStart::ensure_path(&mut self.project, query.name(), &path)?;
        let locations = self
            .project
            .with_query_snapshot(dirty.as_ref(), |snapshot| {
                let target_offsets = Self::target_offsets(snapshot, &path, position)?;
                let analysis_targets = target_offsets
                    .iter()
                    .map(|(_, target, _)| *target)
                    .collect::<Vec<_>>();
                let analysis = snapshot.analysis_for_targets(&analysis_targets)?;
                let mut locations = Vec::new();

                for (context, target, offset) in target_offsets {
                    let targets = match query {
                        NavigationQuery::Definition => {
                            analysis.goto_definition(target, context.file, offset)?
                        }
                        NavigationQuery::TypeDefinition => {
                            analysis.goto_type_definition(target, context.file, offset)?
                        }
                        NavigationQuery::Implementation => {
                            analysis.goto_implementation(target, context.file, offset)?
                        }
                    };

                    for target in targets {
                        let Some(location) = navigation::location_for_target(snapshot, &target)?
                        else {
                            continue;
                        };
                        if !locations.contains(&location) {
                            locations.push(location);
                        }
                    }
                }

                Ok(locations)
            })?;

        tracing::trace!(
            query = query.name(),
            path = %path.display(),
            line = position.line,
            character = position.character,
            result_count = locations.len(),
            elapsed_ms = started.elapsed().as_millis(),
            "navigation query finished"
        );

        Ok(locations)
    }

    fn target_offsets(
        snapshot: ProjectSnapshot<'_>,
        path: &Path,
        position: ls_types::Position,
    ) -> anyhow::Result<Vec<(FileContext, TargetRef, u32)>> {
        let mut targets = Vec::new();

        let contexts = Self::file_contexts(snapshot, path)?;
        for context in contexts {
            let Some(offset) = Self::offset_for_context(snapshot, &context, position) else {
                tracing::trace!(
                    path = %path.display(),
                    line = position.line,
                    character = position.character,
                    package = ?context.package,
                    file = ?context.file,
                    "could not convert LSP position to file offset"
                );
                continue;
            };

            for target in &context.targets {
                targets.push((context.clone(), *target, offset));
            }
        }

        tracing::trace!(
            path = %path.display(),
            line = position.line,
            character = position.character,
            target_offset_count = targets.len(),
            "resolved request target offsets"
        );

        Ok(targets)
    }

    fn reference_search_plan(
        snapshot: ProjectSnapshot<'_>,
        analysis: &QueryAnalysis<'_>,
        context: &FileContext,
        target: TargetRef,
        offset: u32,
    ) -> anyhow::Result<ReferenceSearchPlan> {
        let declaration_targets = analysis
            .goto_definition(target, context.file, offset)?
            .into_iter()
            .map(|target| target.target)
            .collect::<Vec<_>>();
        let targets = snapshot.reference_search_targets(context.package, &declaration_targets);
        let labels = analysis.reference_search_labels(target, context.file, offset)?;
        let files = snapshot.reference_search_files_matching_labels(&targets, &labels)?;

        Ok(ReferenceSearchPlan::new(targets, files))
    }

    fn file_contexts(
        snapshot: ProjectSnapshot<'_>,
        path: &Path,
    ) -> anyhow::Result<Vec<FileContext>> {
        if !path.exists() {
            tracing::debug!(path = %path.display(), "query path does not exist");
            return Ok(Vec::new());
        }

        let contexts = snapshot.file_contexts_for_path(path)?;
        let target_count = contexts
            .iter()
            .map(|context| context.targets.len())
            .sum::<usize>();
        tracing::trace!(
            path = %path.display(),
            context_count = contexts.len(),
            target_count,
            "resolved file contexts"
        );

        Ok(contexts)
    }

    fn offset_for_context(
        snapshot: ProjectSnapshot<'_>,
        context: &FileContext,
        position: ls_types::Position,
    ) -> Option<u32> {
        let line_index = snapshot.file_line_index(context.package, context.file)?;
        let offset = line_index.offset_from_utf16_position(position::parse_position(position));
        tracing::trace!(
            package = ?context.package,
            file = ?context.file,
            line = position.line,
            character = position.character,
            offset = ?offset,
            "converted LSP position to file offset"
        );
        offset
    }

    fn text_span_for_context(
        snapshot: ProjectSnapshot<'_>,
        context: &FileContext,
        range: ls_types::Range,
    ) -> Option<TextSpan> {
        let line_index = snapshot.file_line_index(context.package, context.file)?;
        let start = line_index.offset_from_utf16_position(position::parse_position(range.start))?;
        let end = line_index.offset_from_utf16_position(position::parse_position(range.end))?;

        let span = TextSpan { start, end };
        tracing::trace!(
            package = ?context.package,
            file = ?context.file,
            start_line = range.start.line,
            start_character = range.start.character,
            end_line = range.end.line,
            end_character = range.end.character,
            span_start = span.start,
            span_end = span.end,
            "converted LSP range to text span"
        );
        Some(span)
    }

    fn rename_target_matches_source(
        snapshot: ProjectSnapshot<'_>,
        package: rg_def_map::PackageSlot,
        target: &RenameTarget,
    ) -> bool {
        snapshot
            .file_text_for_span(package, target.file_id, target.span)
            .is_some_and(|text| text == target.placeholder)
    }

    fn verified_rename_edits(
        snapshot: ProjectSnapshot<'_>,
        edits: Vec<RenameEdit>,
    ) -> Option<Vec<RenameEdit>> {
        let mut verified = Vec::new();

        for edit in edits {
            // Keep this query limited to workspace-owned files. References may legitimately see
            // dependency declarations, but rename should not edit source outside this workspace.
            if !snapshot.package_is_workspace_member(edit.target.package) {
                tracing::debug!(
                    package = ?edit.target.package,
                    "rename rejected because an edit targets a non-workspace package"
                );
                return None;
            }

            let Some(text) =
                snapshot.file_text_for_span(edit.target.package, edit.file_id, edit.span)
            else {
                tracing::debug!(
                    package = ?edit.target.package,
                    file = ?edit.file_id,
                    "rename rejected because an edit span has no source text"
                );
                return None;
            };
            if text != edit.old_text {
                tracing::debug!(
                    package = ?edit.target.package,
                    file = ?edit.file_id,
                    expected = %edit.old_text,
                    actual = %text,
                    "rename rejected because an edit span did not match the expected source text"
                );
                return None;
            }

            if !verified.contains(&edit) {
                verified.push(edit);
            }
        }

        Some(verified)
    }

    /// Runs a read-only request and hides disposable cache churn from the LSP client.
    fn respond_to_query<T>(
        &mut self,
        context: QueryContext,
        respond_to: EngineResponse<T>,
        query: impl FnOnce(&mut Self) -> anyhow::Result<T>,
    ) where
        T: Default + Send + 'static,
    {
        // If a newer document version is already available, this queued dirty query can only
        // produce obsolete results. This is an internal optimization, not a replacement for LSP
        // request cancellation.
        if let Some(dirty_identity) = context.stale_dirty_identity(&self.dirty_state) {
            tracing::debug!(
                label = context.label,
                path = %dirty_identity.path().display(),
                version = ?dirty_identity.version(),
                text_len = dirty_identity.text_len(),
                queued_ms = context.queue_elapsed.as_millis(),
                "stale dirty analysis query skipped"
            );
            let _ = respond_to.send(Ok(T::default()));
            return;
        }

        // From here on, clean and current-dirty requests share the same execution path. Keeping the
        // stale check in the context layer lets timing and cache recovery remain uniform for every
        // analysis query.
        let label = context.label;
        let queue_elapsed = context.queue_elapsed;
        tracing::trace!(
            label,
            queued_ms = queue_elapsed.as_millis(),
            "analysis query started"
        );
        let started = Instant::now();
        let memory_control = Arc::clone(&self.memory_control);
        let memory_before = MemoryReporter::snapshot(memory_control.as_ref());
        let result = query(self);
        let query_elapsed = started.elapsed();
        self.project.release_query_memory();
        MemoryReporter::purge_and_report_delta_debug(memory_control.as_ref(), label, memory_before);
        let should_recover = result
            .as_ref()
            .err()
            .is_some_and(Project::is_recoverable_cache_load_failure);
        match &result {
            Ok(_) => {
                tracing::info!(
                    query = label,
                    queued_ms = queue_elapsed.as_millis(),
                    elapsed_ms = query_elapsed.as_millis(),
                    status = "ok",
                    "analysis query completed"
                );
            }
            Err(error) => {
                let error = format!("{error:#}");
                tracing::warn!(
                    query = label,
                    queued_ms = queue_elapsed.as_millis(),
                    elapsed_ms = query_elapsed.as_millis(),
                    status = "error",
                    recoverable_cache_failure = should_recover,
                    error = %error,
                    "analysis query completed"
                );
            }
        }

        if should_recover {
            // Lazy package loads can fail when an offloaded artifact becomes stale between
            // indexing and a query. The next command sees a repaired project, while this request
            // degrades to an empty answer instead of a visible JSON-RPC popup in the editor.
            let _ = respond_to.send(Ok(T::default()));
            self.recover_after_query_cache_failure(label);
        } else {
            let _ = respond_to.send(result);
        }
    }

    fn recover_after_query_cache_failure(&mut self, label: &'static str) {
        if !self.project.is_initialized() {
            tracing::warn!(
                label,
                "analysis query hit invalid package cache before project initialization"
            );
            return;
        }

        let started = Instant::now();
        tracing::warn!(
            label,
            "analysis query hit invalid package cache; rebuilding cache before next command"
        );

        match self.mutate_saved_and_schedule_deferred_finish(|project| {
            project.recover_after_cache_load_failure()
        }) {
            Ok(()) => {
                let snapshot = self
                    .project
                    .saved_snapshot()
                    .expect("project should remain initialized after cache recovery");
                Self::log_project_snapshot(snapshot, "after package cache recovery");
                tracing::info!(
                    label,
                    elapsed_ms = started.elapsed().as_millis(),
                    "package cache recovery finished"
                );
            }
            Err(error) => {
                let error = format!("{error:#}");
                tracing::error!(
                    label,
                    error = %error,
                    "package cache recovery failed"
                );
            }
        }
    }

    /// Logs the retained project shape after the analysis project changes.
    fn log_project_snapshot(snapshot: ProjectSnapshot<'_>, label: &'static str) {
        ProjectStats::capture(snapshot).log_info(label);
        log_retained_memory(snapshot, label);
    }
}

#[derive(Debug, Clone, Copy)]
enum NavigationQuery {
    Definition,
    TypeDefinition,
    Implementation,
}

impl NavigationQuery {
    fn name(self) -> &'static str {
        match self {
            Self::Definition => "definition",
            Self::TypeDefinition => "type_definition",
            Self::Implementation => "implementation",
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        path::PathBuf,
        sync::{Arc, mpsc},
    };

    use tokio::sync::oneshot;

    use super::*;
    use crate::{
        documents::{DirtyDocumentSnapshotState, DocumentStore},
        service::ServiceNotificationPublisher,
    };

    #[derive(Debug)]
    struct NoopNotifications;

    impl ServiceNotificationPublisher for NoopNotifications {
        fn send(&self, _notification: ServiceNotification) {}
    }

    #[test]
    fn project_path_change_collection_merges_adjacent_project_commands_and_defers_next_command() {
        let (sender, receiver) = mpsc::channel();
        let (first_respond_to, first_response) = oneshot::channel::<anyhow::Result<()>>();
        let (second_respond_to, second_response) = oneshot::channel::<anyhow::Result<()>>();
        let (symbol_respond_to, _symbol_response) =
            oneshot::channel::<anyhow::Result<Vec<ls_types::WorkspaceSymbol>>>();
        let (third_respond_to, _third_response) = oneshot::channel::<anyhow::Result<()>>();

        sender
            .send(QueuedEngineCommand::new(
                EngineCommand::ProjectPathsChanged {
                    paths: vec![test_path("a"), test_path("b")],
                    respond_to: second_respond_to,
                },
            ))
            .expect("test command channel should accept adjacent project change");
        sender
            .send(QueuedEngineCommand::new(EngineCommand::WorkspaceSymbol {
                query: "needle".to_string(),
                respond_to: symbol_respond_to,
            }))
            .expect("test command channel should accept non-project command");
        sender
            .send(QueuedEngineCommand::new(
                EngineCommand::ProjectPathsChanged {
                    paths: vec![test_path("c")],
                    respond_to: third_respond_to,
                },
            ))
            .expect("test command channel should accept later project change");

        let mut deferred_commands = VecDeque::new();
        let (paths, responders) = EngineWorker::collect_project_path_changes(
            vec![test_path("b")],
            first_respond_to,
            &receiver,
            &mut deferred_commands,
        );

        assert_eq!(
            paths,
            vec![test_path("a"), test_path("b")],
            "adjacent project changes should be merged and deduplicated"
        );
        assert_eq!(
            responders.len(),
            2,
            "each merged project-change request needs its own response"
        );

        let deferred = deferred_commands
            .pop_front()
            .expect("first non-project command should be deferred");
        match deferred.command {
            EngineCommand::WorkspaceSymbol { query, .. } => {
                assert_eq!(query, "needle");
            }
            command => panic!("unexpected deferred command: {command:?}"),
        }
        assert!(
            deferred_commands.is_empty(),
            "only the first non-project command should be moved aside"
        );

        let queued_after_non_project = receiver
            .try_recv()
            .expect("commands after the first non-project command should stay queued");
        match queued_after_non_project.command {
            EngineCommand::ProjectPathsChanged { paths, .. } => {
                assert_eq!(paths, vec![test_path("c")]);
            }
            command => panic!("unexpected command left in queue: {command:?}"),
        }

        EngineWorker::respond_to_project_path_changes(responders, Ok(()));
        futures::executor::block_on(first_response)
            .expect("first merged project change should receive a response")
            .expect("first merged project change should succeed");
        futures::executor::block_on(second_response)
            .expect("second merged project change should receive a response")
            .expect("second merged project change should succeed");
    }

    #[test]
    fn project_path_change_response_fanout_reports_errors_to_all_callers() {
        let (first_respond_to, first_response) = oneshot::channel::<anyhow::Result<()>>();
        let (second_respond_to, second_response) = oneshot::channel::<anyhow::Result<()>>();

        EngineWorker::respond_to_project_path_changes(
            vec![first_respond_to, second_respond_to],
            Err(anyhow::anyhow!("batch rebuild failed")),
        );

        for response in [first_response, second_response] {
            let error = futures::executor::block_on(response)
                .expect("merged project change should receive an error response")
                .expect_err("merged project change should receive the batch error");
            assert!(
                format!("{error:#}").contains("batch rebuild failed"),
                "fanout error should preserve the original message"
            );
        }
    }

    #[test]
    fn stale_dirty_query_responds_without_running_analysis() {
        let path = PathBuf::from("/workspace/src/lib.rs");
        let mut documents = DocumentStore::default();
        documents.did_open(path.clone(), Some(1), "fn main() {}\n");
        documents.did_change(
            path.clone(),
            Some(2),
            Some("fn main() {\n    dirty();\n}\n"),
        );

        let DirtyDocumentSnapshotState::Dirty(snapshot) = documents.dirty_snapshot(&path) else {
            panic!("dirty full-sync document should expose a snapshot");
        };

        let (sender, _receiver) = std::sync::mpsc::channel();
        let notifications = ServiceNotificationsSink::from_publisher(NoopNotifications);
        let mut worker =
            EngineWorker::new(sender, Arc::new(()), DirtyState::default(), notifications);
        let (respond_to, response) = oneshot::channel();
        let context = QueryContext::document("hover", Duration::ZERO, Some(&snapshot));

        worker.respond_to_query(context, respond_to, |_| {
            panic!("stale query should not run analysis")
        });

        let result: Option<ls_types::Hover> = futures::executor::block_on(response)
            .expect("stale query should send a response")
            .expect("stale query should send a successful neutral result");
        assert!(result.is_none());
    }

    fn test_path(name: &str) -> PathBuf {
        PathBuf::from(format!("/workspace/src/{name}.rs"))
    }
}
