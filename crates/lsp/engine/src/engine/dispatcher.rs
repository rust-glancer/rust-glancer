//! FIFO command routing for the single semantic execution lane.
//!
//! RPC tasks may enqueue work concurrently, but saved-project changes and semantic queries run one
//! at a time here. This is intentional: query-time materialization mutates package residency, so a
//! pool of otherwise read-only queries would still need to coordinate ownership of the project.
//! Background deferred indexing follows the same rule by returning its result as another command.

use std::sync::{
    Arc,
    mpsc::{Receiver, Sender},
};

use rg_lsp_proto::AnalysisScope;

use crate::{
    engine::{
        QueuedEngineCommand,
        command::EngineCommand,
        project::ProjectCoordinator,
        query::{QueryContext, QueryRunner},
    },
    memory::MemoryControl,
    service::ServiceNotificationsSink,
};

/// Owns the engine lane and routes each command to its narrower subsystem.
///
/// `ProjectCoordinator` owns long-lived saved state. A `QueryRunner` is borrowed only for one
/// request, making it impossible for query helpers to outlive the command or retain a project
/// snapshot across the next mutation.
#[derive(Debug)]
pub(super) struct EngineDispatcher {
    project: ProjectCoordinator,
    memory_control: Arc<dyn MemoryControl>,
}

impl EngineDispatcher {
    pub(super) fn new(
        sender: Sender<QueuedEngineCommand>,
        memory_control: Arc<dyn MemoryControl>,
        notifications: ServiceNotificationsSink,
    ) -> Self {
        Self {
            project: ProjectCoordinator::new(sender, Arc::clone(&memory_control), notifications),
            memory_control,
        }
    }

    fn query_runner(&mut self) -> QueryRunner<'_> {
        QueryRunner::new(&mut self.project, Arc::clone(&self.memory_control))
    }

    /// Drain commands until shutdown or until every sender has been dropped.
    ///
    /// Lifecycle commands mutate the coordinator directly. Analysis commands are wrapped in a
    /// `QueryContext`, then delegated to the shared query lifecycle before their response is sent.
    pub(super) fn run(mut self, receiver: Receiver<QueuedEngineCommand>) {
        tracing::debug!("LSP engine dispatcher started");

        while let Ok(queued) = receiver.recv() {
            let queue_elapsed = queued.enqueued_at.elapsed();
            let command = queued.command;
            // Keep the protocol-to-subsystem mapping explicit. Most arms are deliberately small:
            // they record request identity here and leave query/recovery policy to `QueryRunner`.
            match command {
                EngineCommand::Initialize {
                    root,
                    configuration,
                    respond_to,
                } => {
                    tracing::trace!(root = %root.display(), "engine command started: initialize");
                    let _ = respond_to.send(self.project.initialize(root, configuration));
                }
                EngineCommand::RecoverStaleSource { path } => {
                    tracing::trace!(
                        path = %path.display(),
                        "engine command started: recover_stale_source"
                    );
                    if let Err(error) = self.project.recover_stale_source(path) {
                        tracing::warn!(
                            error = %format!("{error:#}"),
                            "background stale-source recovery failed"
                        );
                    }
                }
                EngineCommand::SavedProjectChanges {
                    changes,
                    respond_to,
                } => {
                    tracing::trace!(
                        change_count = changes.len(),
                        "engine command started: saved_project_changes"
                    );
                    let _ = respond_to.send(self.project.saved_project_changes(changes));
                }
                EngineCommand::GotoDefinition { input, respond_to } => {
                    tracing::trace!(
                        path = %input.analysis.target().path().display(),
                        line = input.position.line,
                        character = input.position.character,
                        "engine command started: goto_definition"
                    );
                    let context = QueryContext::document(
                        "goto_definition",
                        queue_elapsed,
                        &input.analysis,
                        AnalysisScope::ChangedPackages,
                    );
                    self.query_runner()
                        .respond_to_query(context, respond_to, |runner, _| {
                            runner.goto_definition(input)
                        });
                }
                EngineCommand::GotoTypeDefinition { input, respond_to } => {
                    tracing::trace!(
                        path = %input.analysis.target().path().display(),
                        line = input.position.line,
                        character = input.position.character,
                        "engine command started: goto_type_definition"
                    );
                    let context = QueryContext::document(
                        "goto_type_definition",
                        queue_elapsed,
                        &input.analysis,
                        AnalysisScope::ChangedPackages,
                    );
                    self.query_runner()
                        .respond_to_query(context, respond_to, |runner, _| {
                            runner.goto_type_definition(input)
                        });
                }
                EngineCommand::GotoImplementation { input, respond_to } => {
                    tracing::trace!(
                        path = %input.analysis.target().path().display(),
                        line = input.position.line,
                        character = input.position.character,
                        "engine command started: goto_implementation"
                    );
                    let context = QueryContext::document(
                        "goto_implementation",
                        queue_elapsed,
                        &input.analysis,
                        AnalysisScope::ReverseDependencyClosure,
                    );
                    self.query_runner()
                        .respond_to_query(context, respond_to, |runner, _| {
                            runner.goto_implementation(input)
                        });
                }
                EngineCommand::References {
                    input,
                    include_declaration,
                    respond_to,
                } => {
                    tracing::trace!(
                        path = %input.analysis.target().path().display(),
                        line = input.position.line,
                        character = input.position.character,
                        include_declaration,
                        "engine command started: references"
                    );
                    let context = QueryContext::document(
                        "references",
                        queue_elapsed,
                        &input.analysis,
                        AnalysisScope::ReverseDependencyClosure,
                    );
                    self.query_runner()
                        .respond_to_query(context, respond_to, |runner, _| {
                            runner.references(input, include_declaration)
                        });
                }
                EngineCommand::PrepareRename { input, respond_to } => {
                    tracing::trace!(
                        path = %input.analysis.target().path().display(),
                        line = input.position.line,
                        character = input.position.character,
                        "engine command started: prepare_rename"
                    );
                    let context = QueryContext::document(
                        "prepare_rename",
                        queue_elapsed,
                        &input.analysis,
                        AnalysisScope::ChangedPackages,
                    );
                    self.query_runner()
                        .respond_to_query(context, respond_to, |runner, _| {
                            runner.prepare_rename(input)
                        });
                }
                EngineCommand::Rename {
                    input,
                    new_name,
                    respond_to,
                } => {
                    tracing::trace!(
                        path = %input.analysis.target().path().display(),
                        line = input.position.line,
                        character = input.position.character,
                        new_name = %new_name,
                        "engine command started: rename"
                    );
                    let context = QueryContext::document(
                        "rename",
                        queue_elapsed,
                        &input.analysis,
                        AnalysisScope::ReverseDependencyClosure,
                    );
                    self.query_runner()
                        .respond_to_query(context, respond_to, |runner, _| {
                            runner.rename(input, new_name)
                        });
                }
                EngineCommand::DocumentHighlight { input, respond_to } => {
                    tracing::trace!(
                        path = %input.analysis.target().path().display(),
                        line = input.position.line,
                        character = input.position.character,
                        "engine command started: document_highlight"
                    );
                    let context = QueryContext::document(
                        "document_highlight",
                        queue_elapsed,
                        &input.analysis,
                        AnalysisScope::ChangedPackages,
                    );
                    self.query_runner()
                        .respond_to_query(context, respond_to, |runner, _| {
                            runner.document_highlight(input)
                        });
                }
                EngineCommand::Hover { input, respond_to } => {
                    tracing::trace!(
                        path = %input.analysis.target().path().display(),
                        line = input.position.line,
                        character = input.position.character,
                        "engine command started: hover"
                    );
                    let context = QueryContext::document(
                        "hover",
                        queue_elapsed,
                        &input.analysis,
                        AnalysisScope::ChangedPackages,
                    );
                    self.query_runner()
                        .respond_to_query(context, respond_to, |runner, _| runner.hover(input));
                }
                EngineCommand::Completion {
                    input,
                    client_capabilities,
                    respond_to,
                } => {
                    tracing::trace!(
                        path = %input.analysis.target().path().display(),
                        line = input.position.line,
                        character = input.position.character,
                        "engine command started: completion"
                    );
                    let context = QueryContext::document(
                        "completion",
                        queue_elapsed,
                        &input.analysis,
                        AnalysisScope::ChangedPackages,
                    );
                    self.query_runner().respond_to_query(
                        context,
                        respond_to,
                        |runner, cancellation| {
                            runner.completion(input, client_capabilities, cancellation)
                        },
                    );
                }
                EngineCommand::Formatting {
                    snapshot,
                    respond_to,
                } => {
                    tracing::trace!(
                        path = %snapshot.target().path().display(),
                        "engine command started: formatting"
                    );
                    let context = QueryContext::document(
                        "formatting",
                        queue_elapsed,
                        &snapshot,
                        AnalysisScope::TargetDocument,
                    );
                    self.query_runner()
                        .respond_to_query(context, respond_to, |runner, _| {
                            runner.formatting(snapshot).map(Some)
                        });
                }
                EngineCommand::DocumentSymbol {
                    snapshot,
                    respond_to,
                } => {
                    tracing::trace!(
                        path = %snapshot.target().path().display(),
                        "engine command started: document_symbol"
                    );
                    let context = QueryContext::document(
                        "document_symbol",
                        queue_elapsed,
                        &snapshot,
                        AnalysisScope::ChangedPackages,
                    );
                    self.query_runner()
                        .respond_to_query(context, respond_to, |runner, _| {
                            runner.document_symbol(snapshot)
                        });
                }
                EngineCommand::InlayHint { input, respond_to } => {
                    tracing::trace!(
                        path = %input.analysis.target().path().display(),
                        start_line = input.range.start.line,
                        start_character = input.range.start.character,
                        end_line = input.range.end.line,
                        end_character = input.range.end.character,
                        "engine command started: inlay_hint"
                    );
                    let context = QueryContext::document(
                        "inlay_hint",
                        queue_elapsed,
                        &input.analysis,
                        AnalysisScope::ChangedPackages,
                    );
                    self.query_runner()
                        .respond_to_query(context, respond_to, |runner, _| {
                            runner.inlay_hint(input)
                        });
                }
                EngineCommand::WorkspaceSymbol { query, respond_to } => {
                    tracing::trace!(query = %query, "engine command started: workspace_symbol");
                    let context = QueryContext::new("workspace_symbol", queue_elapsed);
                    self.query_runner()
                        .respond_to_query(context, respond_to, |runner, _| {
                            runner.workspace_symbol(&query)
                        });
                }
                EngineCommand::ReindexWorkspace { respond_to } => {
                    tracing::trace!("engine command started: reindex_workspace");
                    let _ = respond_to.send(self.project.reindex_workspace());
                }
                EngineCommand::DeferredIndexingFinished { generation, result } => {
                    tracing::trace!(
                        generation,
                        "engine command started: deferred_indexing_finished"
                    );
                    self.project.deferred_indexing_finished(generation, result);
                }
                EngineCommand::Shutdown(respond_to) => {
                    tracing::info!("shutting down LSP engine dispatcher");
                    let _ = respond_to.send(Ok(()));
                    break;
                }
            }
        }

        tracing::debug!("LSP engine dispatcher stopped");
    }
}
