use std::{
    path::{Path, PathBuf},
    sync::{Arc, mpsc::Receiver},
    time::{Duration, Instant},
};

use anyhow::Context as _;
use rg_analysis::{CompletionQuery, ReferenceQuery, TypeHint};
use rg_def_map::TargetRef;
use rg_lsp_proto::{AnalysisConfig, CompletionClientCapabilities};
use rg_parse::TextSpan;
use rg_project::{
    CacheProbeProfile, FileContext, Project, ProjectMemoryHooks, ProjectSnapshot, SavedFileChange,
};
use rg_workspace::{CargoMetadataTarget, SysrootSources, WorkspaceMetadata};

use crate::{
    dirty_state::{DirtyDocumentIdentity, DirtyState},
    documents::DirtyDocumentSnapshot,
    engine::{
        QueuedEngineCommand,
        command::{EngineCommand, EngineResponse},
        project_proxy::ProjectProxy,
    },
    memory::{MemoryControl, MemoryReporter, ProjectMemoryReporter},
    project_stats::{ProjectStats, log_retained_memory},
    proto::{completion, hover, inlay_hint, navigation, position, references, symbols},
};

#[derive(Debug)]
pub(super) struct EngineWorker {
    project: ProjectProxy,
    dirty_state: DirtyState,
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
    pub(super) fn new(memory_control: Arc<dyn MemoryControl>, dirty_state: DirtyState) -> Self {
        let memory_hooks = Arc::new(ProjectMemoryReporter::new(Arc::clone(&memory_control)));
        Self {
            project: ProjectProxy::new(Arc::clone(&memory_control)),
            dirty_state,
            memory_control,
            memory_hooks,
        }
    }

    pub(super) fn run(mut self, receiver: Receiver<QueuedEngineCommand>) {
        tracing::debug!("LSP engine worker started");

        while let Ok(queued) = receiver.recv() {
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
                EngineCommand::DidSave {
                    path,
                    text,
                    respond_to,
                } => {
                    tracing::trace!(
                        path = %path.display(),
                        has_text = text.is_some(),
                        text_len = ?text.as_ref().map(String::len),
                        "engine command started: did_save"
                    );
                    let _ = respond_to.send(self.did_save(path, text));
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
                EngineCommand::Shutdown(respond_to) => {
                    tracing::info!("shutting down LSP engine worker");
                    let _ = respond_to.send(Ok(()));
                    break;
                }
            }
        }

        tracing::debug!("LSP engine worker stopped");
    }

    fn initialize(&mut self, root: PathBuf, analysis: AnalysisConfig) -> anyhow::Result<()> {
        let package_residency_policy = analysis.package_residency_policy;
        let cargo_metadata_config = analysis.cargo_metadata_config;
        let started = Instant::now();
        let configured_target = match cargo_metadata_config.target() {
            CargoMetadataTarget::Auto => "auto",
            CargoMetadataTarget::Triple(target) => target.as_str(),
        };
        tracing::info!(
            root = %root.display(),
            package_residency = package_residency_policy.config_name(),
            cargo_target = configured_target,
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
            .load_metadata(&manifest_path)
            .context("while attempting to run cargo metadata for LSP initialization")?;
        tracing::info!(
            package_count = metadata.packages.len(),
            elapsed_ms = metadata_started.elapsed().as_millis(),
            "cargo metadata finished"
        );

        let workspace = WorkspaceMetadata::from_cargo(metadata)
            .context("while attempting to normalize Cargo metadata")?;
        let workspace_root = workspace.workspace_root().to_path_buf();
        let sysroot = SysrootSources::discover(workspace.workspace_root());
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

        let log_startup_cache_probe = tracing::enabled!(tracing::Level::DEBUG);
        let workspace = workspace.with_sysroot_sources(sysroot);
        let project_build = Project::builder(workspace)
            .cargo_metadata_config(cargo_metadata_config)
            .package_residency_policy(package_residency_policy)
            .profile_build_timing(log_startup_cache_probe)
            .memory_hooks(Arc::clone(&self.memory_hooks))
            .build()
            .context("while attempting to build LSP analysis project")?;
        Self::log_startup_cache_probe(
            project_build
                .profile()
                .and_then(|profile| profile.cache_probe()),
        );
        self.project.replace_saved(project_build.into_project());
        Self::log_project_snapshot(self.project.saved_snapshot()?, "initial index");
        tracing::info!(
            workspace_root = %workspace_root.display(),
            elapsed_ms = started.elapsed().as_millis(),
            "workspace indexing finished"
        );

        Ok(())
    }

    fn reindex_workspace(&mut self) -> anyhow::Result<()> {
        let started = Instant::now();

        tracing::info!("manual workspace reindex started");
        self.project.mutate_saved(|project| {
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

    fn did_save(&mut self, path: PathBuf, text: Option<String>) -> anyhow::Result<()> {
        let started = Instant::now();
        tracing::info!(
            path = %path.display(),
            notification_includes_text = text.is_some(),
            "processing saved file"
        );

        let summary = self.project.mutate_saved(|project| {
            project
                .apply_change(SavedFileChange::new(&path))
                .context("while attempting to apply saved file change")
        })?;
        tracing::info!(
            path = %path.display(),
            changed_files = summary.changed_files.len(),
            affected_packages = summary.affected_packages.len(),
            changed_targets = summary.changed_targets.len(),
            elapsed_ms = started.elapsed().as_millis(),
            "saved file reindex finished"
        );
        Self::log_project_snapshot(self.project.saved_snapshot()?, "after save");

        Ok(())
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
        // Dirty overlays lower Body IR only for dirty files. Cross-file body references are
        // intentionally best-effort until the buffer is saved and the normal project catches up.
        let locations = self
            .project
            .with_query_snapshot(dirty.as_ref(), |snapshot| {
                let target_offsets = Self::target_offsets(snapshot, &path, position)?;
                let analysis = snapshot.full_analysis()?;
                let mut locations = Vec::new();

                for (context, target, offset) in target_offsets {
                    let declaration_targets = analysis
                        .goto_definition(target, context.file, offset)?
                        .into_iter()
                        .map(|target| target.target)
                        .collect::<Vec<_>>();
                    let search_targets =
                        snapshot.reference_search_targets(context.package, &declaration_targets);

                    for reference in analysis.references(
                        target,
                        context.file,
                        offset,
                        ReferenceQuery::find_references(&search_targets, include_declaration),
                    )? {
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

        tracing::debug!(
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

    fn document_highlight(
        &mut self,
        path: PathBuf,
        position: ls_types::Position,
        dirty: Option<DirtyDocumentSnapshot>,
    ) -> anyhow::Result<Vec<ls_types::DocumentHighlight>> {
        let started = Instant::now();
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

        tracing::debug!(
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

        tracing::debug!(
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

        tracing::debug!(
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

        tracing::debug!(
            path = %path.display(),
            result_count = lsp_symbols.len(),
            elapsed_ms = started.elapsed().as_millis(),
            "document symbol query finished"
        );

        Ok(lsp_symbols)
    }

    fn inlay_hint(
        &mut self,
        path: PathBuf,
        range: ls_types::Range,
        dirty: Option<DirtyDocumentSnapshot>,
    ) -> anyhow::Result<Vec<ls_types::InlayHint>> {
        let started = Instant::now();
        let lsp_hints = self
            .project
            .with_query_snapshot(dirty.as_ref(), |snapshot| {
                let contexts = Self::file_contexts(snapshot, &path)?;
                let analysis_targets = contexts
                    .iter()
                    .flat_map(|context| context.targets.iter().copied())
                    .collect::<Vec<_>>();
                let analysis = snapshot.analysis_for_targets(&analysis_targets)?;
                let mut hints = Vec::<(rg_def_map::PackageSlot, TypeHint)>::new();

                for context in contexts {
                    let Some(range) = Self::text_span_for_context(snapshot, &context, range) else {
                        continue;
                    };

                    for target in context.targets {
                        for hint in analysis.type_hints(target, context.file, Some(range))? {
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
                    let Some(hint) = inlay_hint::type_hint(snapshot, package, hint)? else {
                        continue;
                    };
                    lsp_hints.push(hint);
                }

                Ok(lsp_hints)
            })?;

        tracing::debug!(
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

        tracing::debug!(
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

        tracing::debug!(
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
        tracing::debug!(
            path = %path.display(),
            context_count = contexts.len(),
            target_count,
            "resolved file contexts"
        );
        tracing::trace!(
            path = %path.display(),
            context_count = contexts.len(),
            target_count,
            "resolved file contexts for query"
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

    /// Runs a read-only request, responds immediately, then heals disposable cache failures.
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
        tracing::debug!(
            label,
            queued_ms = queue_elapsed.as_millis(),
            "analysis query dequeued"
        );
        let started = Instant::now();
        let memory_control = Arc::clone(&self.memory_control);
        let memory_before =
            MemoryReporter::log_checkpoint(memory_control.as_ref(), label, "query_before");
        let result = query(self);
        let query_elapsed = started.elapsed();
        MemoryReporter::log_checkpoint_delta(
            memory_control.as_ref(),
            label,
            "query_after",
            memory_before,
        );
        let should_recover = result
            .as_ref()
            .err()
            .is_some_and(Project::is_recoverable_cache_load_failure);
        tracing::debug!(
            label,
            queued_ms = queue_elapsed.as_millis(),
            query_elapsed_ms = query_elapsed.as_millis(),
            result_ok = result.is_ok(),
            recoverable_cache_failure = should_recover,
            "analysis query finished"
        );

        let _ = respond_to.send(result);

        if should_recover {
            self.recover_after_query_cache_failure(label);
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

        match self
            .project
            .mutate_saved(|project| project.recover_after_cache_load_failure())
        {
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
                tracing::error!(
                    label,
                    error = %error,
                    "package cache recovery failed"
                );
            }
        }
    }

    fn log_startup_cache_probe(profile: Option<&CacheProbeProfile>) {
        let Some(profile) = profile else {
            return;
        };

        tracing::debug!(
            packages = profile.package_count,
            resident = profile.resident_count,
            offloadable = profile.offloadable_count,
            hits = profile.hit_count,
            misses = profile.miss_count(),
            missing_artifacts = profile.missing_artifact_count,
            artifact_read_errors = profile.artifact_read_error_count,
            source_mismatches = profile.source_mismatch_count,
            source_errors = profile.source_error_count,
            body_ir_policy_mismatches = profile.body_ir_policy_mismatch_count,
            parse_restore_errors = profile.restore_error_count,
            unplanned_packages = profile.unplanned_package_count,
            artifact_read_ms = profile.artifact_read_elapsed.as_millis(),
            source_fingerprint_ms = profile.source_fingerprint_elapsed.as_millis(),
            parse_restore_ms = profile.parse_restore_elapsed.as_millis(),
            "startup cache probe finished"
        );
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
    use std::{path::PathBuf, sync::Arc};

    use tokio::sync::oneshot;

    use super::*;
    use crate::documents::{DirtyDocumentSnapshotState, DocumentStore};

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

        let mut worker = EngineWorker::new(Arc::new(()), DirtyState::default());
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
}
