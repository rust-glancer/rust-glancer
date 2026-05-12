use std::{
    path::{Path, PathBuf},
    sync::{Arc, mpsc::Receiver},
    time::Instant,
};

use anyhow::Context as _;
use rg_analysis::TypeHint;
use rg_def_map::TargetRef;
use rg_lsp_proto::AnalysisConfig;
use rg_parse::TextSpan;
use rg_project::{CacheProbeProfile, FileContext, Project, ProjectSnapshot, SavedFileChange};
use rg_workspace::{CargoMetadataTarget, SysrootSources, WorkspaceMetadata};

use crate::{
    engine::command::{EngineCommand, EngineResponse},
    memory::{MemoryControl, MemoryReporter},
    project_stats::{ProjectStats, log_retained_memory},
    proto::{completion, hover, inlay_hint, navigation, position, symbols},
};

#[derive(Debug)]
pub(super) struct EngineWorker {
    project: Option<Project>,
    memory_control: Arc<dyn MemoryControl>,
}

impl EngineWorker {
    pub(super) fn new(memory_control: Arc<dyn MemoryControl>) -> Self {
        Self {
            project: None,
            memory_control,
        }
    }

    pub(super) fn run(mut self, receiver: Receiver<EngineCommand>) {
        tracing::debug!("LSP engine worker started");

        while let Ok(command) = receiver.recv() {
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
                    respond_to,
                } => {
                    tracing::trace!(
                        path = %path.display(),
                        line = position.line,
                        character = position.character,
                        "engine command started: goto_definition"
                    );
                    self.respond_to_query("goto_definition", respond_to, |worker| {
                        worker.goto_definition(path, position)
                    });
                }
                EngineCommand::GotoTypeDefinition {
                    path,
                    position,
                    respond_to,
                } => {
                    tracing::trace!(
                        path = %path.display(),
                        line = position.line,
                        character = position.character,
                        "engine command started: goto_type_definition"
                    );
                    self.respond_to_query("goto_type_definition", respond_to, |worker| {
                        worker.goto_type_definition(path, position)
                    });
                }
                EngineCommand::GotoImplementation {
                    path,
                    position,
                    respond_to,
                } => {
                    tracing::trace!(
                        path = %path.display(),
                        line = position.line,
                        character = position.character,
                        "engine command started: goto_implementation"
                    );
                    self.respond_to_query("goto_implementation", respond_to, |worker| {
                        worker.goto_implementation(path, position)
                    });
                }
                EngineCommand::Hover {
                    path,
                    position,
                    respond_to,
                } => {
                    tracing::trace!(
                        path = %path.display(),
                        line = position.line,
                        character = position.character,
                        "engine command started: hover"
                    );
                    self.respond_to_query("hover", respond_to, |worker| {
                        worker.hover(path, position)
                    });
                }
                EngineCommand::Completion {
                    path,
                    position,
                    respond_to,
                } => {
                    tracing::trace!(
                        path = %path.display(),
                        line = position.line,
                        character = position.character,
                        "engine command started: completion"
                    );
                    self.respond_to_query("completion", respond_to, |worker| {
                        worker.completion(path, position)
                    });
                }
                EngineCommand::DocumentSymbol { path, respond_to } => {
                    tracing::trace!(
                        path = %path.display(),
                        "engine command started: document_symbol"
                    );
                    self.respond_to_query("document_symbol", respond_to, |worker| {
                        worker.document_symbol(path)
                    });
                }
                EngineCommand::InlayHint {
                    path,
                    range,
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
                    self.respond_to_query("inlay_hint", respond_to, |worker| {
                        worker.inlay_hint(path, range)
                    });
                }
                EngineCommand::WorkspaceSymbol { query, respond_to } => {
                    tracing::trace!(query = %query, "engine command started: workspace_symbol");
                    self.respond_to_query("workspace_symbol", respond_to, |worker| {
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
            .build()
            .context("while attempting to build LSP analysis project")?;
        Self::log_startup_cache_probe(
            project_build
                .profile()
                .and_then(|profile| profile.cache_probe()),
        );
        let project = project_build.into_project();
        let snapshot = project.snapshot();
        Self::post_project_build(self.memory_control.as_ref(), snapshot, "initial index");

        self.project = Some(project);
        tracing::info!(
            workspace_root = %workspace_root.display(),
            elapsed_ms = started.elapsed().as_millis(),
            "workspace indexing finished"
        );

        Ok(())
    }

    fn reindex_workspace(&mut self) -> anyhow::Result<()> {
        let started = Instant::now();
        let memory_control = Arc::clone(&self.memory_control);
        let project = self
            .project
            .as_mut()
            .context("LSP engine is not initialized")?;

        tracing::info!("manual workspace reindex started");
        project
            .reindex_workspace()
            .context("while attempting to manually reindex workspace")?;
        Self::post_project_build(
            memory_control.as_ref(),
            project.snapshot(),
            "manual reindex",
        );
        tracing::info!(
            elapsed_ms = started.elapsed().as_millis(),
            "manual workspace reindex finished"
        );

        Ok(())
    }

    fn did_save(&mut self, path: PathBuf, text: Option<String>) -> anyhow::Result<()> {
        let started = Instant::now();
        let memory_control = Arc::clone(&self.memory_control);
        let project = self
            .project
            .as_mut()
            .context("LSP engine is not initialized")?;

        tracing::info!(
            path = %path.display(),
            notification_includes_text = text.is_some(),
            "processing saved file"
        );

        let summary = project
            .apply_change(SavedFileChange::new(&path))
            .context("while attempting to apply saved file change")?;
        tracing::info!(
            path = %path.display(),
            changed_files = summary.changed_files.len(),
            affected_packages = summary.affected_packages.len(),
            changed_targets = summary.changed_targets.len(),
            elapsed_ms = started.elapsed().as_millis(),
            "saved file reindex finished"
        );
        Self::post_project_build(memory_control.as_ref(), project.snapshot(), "after save");

        Ok(())
    }

    fn goto_definition(
        &self,
        path: PathBuf,
        position: ls_types::Position,
    ) -> anyhow::Result<Vec<ls_types::Location>> {
        self.navigation_query(path, position, NavigationQuery::Definition)
    }

    fn goto_type_definition(
        &self,
        path: PathBuf,
        position: ls_types::Position,
    ) -> anyhow::Result<Vec<ls_types::Location>> {
        self.navigation_query(path, position, NavigationQuery::TypeDefinition)
    }

    fn goto_implementation(
        &self,
        path: PathBuf,
        position: ls_types::Position,
    ) -> anyhow::Result<Vec<ls_types::Location>> {
        self.navigation_query(path, position, NavigationQuery::Implementation)
    }

    fn completion(
        &self,
        path: PathBuf,
        position: ls_types::Position,
    ) -> anyhow::Result<Vec<ls_types::CompletionItem>> {
        let started = Instant::now();
        let snapshot = self.snapshot()?;
        let target_offsets = self.target_offsets(snapshot, &path, position)?;
        let analysis_targets = target_offsets
            .iter()
            .map(|(_, target, _)| *target)
            .collect::<Vec<_>>();
        let analysis = snapshot.analysis_for_targets(&analysis_targets)?;
        let mut completions = Vec::new();

        for (context, target, offset) in target_offsets {
            for item in analysis.completions_at_dot(target, context.file, offset)? {
                let item = completion::completion_item(item);
                if !completions.contains(&item) {
                    completions.push(item);
                }
            }
        }

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
        &self,
        path: PathBuf,
        position: ls_types::Position,
    ) -> anyhow::Result<Option<ls_types::Hover>> {
        let started = Instant::now();
        let snapshot = self.snapshot()?;
        let target_offsets = self.target_offsets(snapshot, &path, position)?;
        let analysis_targets = target_offsets
            .iter()
            .map(|(_, target, _)| *target)
            .collect::<Vec<_>>();
        let analysis = snapshot.analysis_for_targets(&analysis_targets)?;

        for (context, target, offset) in target_offsets {
            let Some(info) = analysis.hover(target, context.file, offset)? else {
                continue;
            };
            let Some(line_index) = snapshot.file_line_index(context.package, context.file) else {
                continue;
            };
            let Some(hover) = hover::hover(info, line_index) else {
                continue;
            };
            tracing::debug!(
                path = %path.display(),
                line = position.line,
                character = position.character,
                has_hover = true,
                elapsed_ms = started.elapsed().as_millis(),
                "hover query finished"
            );
            return Ok(Some(hover));
        }

        tracing::debug!(
            path = %path.display(),
            line = position.line,
            character = position.character,
            has_hover = false,
            elapsed_ms = started.elapsed().as_millis(),
            "hover query finished"
        );
        Ok(None)
    }

    fn document_symbol(&self, path: PathBuf) -> anyhow::Result<Vec<ls_types::DocumentSymbol>> {
        let started = Instant::now();
        let snapshot = self.snapshot()?;
        let contexts = self.file_contexts(snapshot, &path)?;
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
                    let symbol = symbols::document_symbol(snapshot, context.package, symbol)?;
                    if !lsp_symbols.contains(&symbol) {
                        lsp_symbols.push(symbol);
                    }
                }
            }
        }

        tracing::debug!(
            path = %path.display(),
            result_count = lsp_symbols.len(),
            elapsed_ms = started.elapsed().as_millis(),
            "document symbol query finished"
        );

        Ok(lsp_symbols)
    }

    fn inlay_hint(
        &self,
        path: PathBuf,
        range: ls_types::Range,
    ) -> anyhow::Result<Vec<ls_types::InlayHint>> {
        let started = Instant::now();
        let snapshot = self.snapshot()?;
        let contexts = self.file_contexts(snapshot, &path)?;
        let analysis_targets = contexts
            .iter()
            .flat_map(|context| context.targets.iter().copied())
            .collect::<Vec<_>>();
        let analysis = snapshot.analysis_for_targets(&analysis_targets)?;
        let mut hints = Vec::<(rg_def_map::PackageSlot, TypeHint)>::new();

        for context in contexts {
            let Some(range) = self.text_span_for_context(snapshot, &context, range) else {
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
        let snapshot = self.snapshot()?;
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
        &self,
        path: PathBuf,
        position: ls_types::Position,
        query: NavigationQuery,
    ) -> anyhow::Result<Vec<ls_types::Location>> {
        let started = Instant::now();
        let snapshot = self.snapshot()?;
        let target_offsets = self.target_offsets(snapshot, &path, position)?;
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
                let Some(location) = navigation::location_for_target(snapshot, &target)? else {
                    continue;
                };
                if !locations.contains(&location) {
                    locations.push(location);
                }
            }
        }

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
        &self,
        snapshot: ProjectSnapshot<'_>,
        path: &Path,
        position: ls_types::Position,
    ) -> anyhow::Result<Vec<(FileContext, TargetRef, u32)>> {
        let mut targets = Vec::new();

        let contexts = self.file_contexts(snapshot, path)?;
        for context in contexts {
            let Some(offset) = self.offset_for_context(snapshot, &context, position) else {
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
        &self,
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
        &self,
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
        &self,
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

    fn snapshot(&self) -> anyhow::Result<ProjectSnapshot<'_>> {
        self.project
            .as_ref()
            .map(Project::snapshot)
            .context("LSP engine is not initialized")
    }

    /// Runs a read-only request, responds immediately, then heals disposable cache failures.
    fn respond_to_query<T>(
        &mut self,
        label: &'static str,
        respond_to: EngineResponse<T>,
        query: impl FnOnce(&Self) -> anyhow::Result<T>,
    ) where
        T: Send + 'static,
    {
        let result = MemoryReporter::report_op(self.memory_control.as_ref(), label, || query(self));
        let should_recover = result
            .as_ref()
            .err()
            .is_some_and(Project::is_recoverable_cache_load_failure);

        let _ = respond_to.send(result);

        if should_recover {
            self.recover_after_query_cache_failure(label);
        }
    }

    fn recover_after_query_cache_failure(&mut self, label: &'static str) {
        let Some(project) = self.project.as_mut() else {
            tracing::warn!(
                label,
                "analysis query hit invalid package cache before project initialization"
            );
            return;
        };

        let started = Instant::now();
        tracing::warn!(
            label,
            "analysis query hit invalid package cache; rebuilding cache before next command"
        );

        match project.recover_after_cache_load_failure() {
            Ok(()) => {
                let snapshot = project.snapshot();
                Self::post_project_build(
                    self.memory_control.as_ref(),
                    snapshot,
                    "after package cache recovery",
                );
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

    /// Hook for activities to be run after the project (re-)build.
    fn post_project_build(
        memory_control: &dyn MemoryControl,
        snapshot: ProjectSnapshot<'_>,
        label: &'static str,
    ) {
        // Indexing can temporarily materialize most of the project. Once the snapshot is ready for
        // editor queries, purge allocator caches and report memory separately from project shape.
        MemoryReporter::report_current(memory_control, label);
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
