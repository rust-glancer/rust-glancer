//! FIFO command routing for the single semantic execution lane.
//!
//! RPC tasks may enqueue work concurrently, but saved-project changes and semantic queries run one
//! at a time here. This is intentional: query-time materialization mutates package residency, so a
//! pool of otherwise read-only queries would still need to coordinate ownership of the project.
//! Background deferred indexing follows the same rule by returning its result as another command.
//!
//! Adjacent watcher batches are the only commands allowed to coalesce. The first interactive or
//! lifecycle command ends the batch and keeps its original place in the queue.
//!
//! For example, `watch(a), watch(b), hover, watch(c)` runs as one `{a, b}` rebuild, then hover,
//! then the `{c}` rebuild. Coalescing never pulls the last watcher command across hover.

use std::{
    path::PathBuf,
    sync::{
        Arc,
        mpsc::{Receiver, Sender},
    },
};

use crate::{
    dirty_state::DirtyState,
    engine::{
        QueuedEngineCommand,
        command::{EngineCommand, EngineResponse},
        project::ProjectCoordinator,
        query::{QueryContext, QueryRunner},
    },
    memory::MemoryControl,
    service::ServiceNotificationsSink,
};

/// Receives commands in arrival order and preserves one command read ahead during coalescing.
///
/// `std::sync::mpsc` has no peek operation. `pending` holds the first non-watcher command consumed
/// while looking for another adjacent watcher batch, so `next` can return it before reading again.
struct CommandQueue {
    receiver: Receiver<QueuedEngineCommand>,
    pending: Option<QueuedEngineCommand>,
}

impl CommandQueue {
    fn new(receiver: Receiver<QueuedEngineCommand>) -> Self {
        Self {
            receiver,
            pending: None,
        }
    }

    fn next(&mut self) -> Option<QueuedEngineCommand> {
        self.pending.take().or_else(|| self.receiver.recv().ok())
    }

    /// Merge only immediately adjacent watcher batches.
    ///
    /// The first non-project command is retained in the FIFO rather than skipped, and commands
    /// behind it remain unread so an interactive request keeps its original ordering.
    fn collect_project_path_changes(
        &mut self,
        paths: Vec<PathBuf>,
        respond_to: EngineResponse<()>,
    ) -> (Vec<PathBuf>, Vec<EngineResponse<()>>) {
        let mut paths = paths;
        let mut responders = vec![respond_to];

        while let Ok(queued) = self.receiver.try_recv() {
            match queued.command {
                EngineCommand::ProjectPathsChanged {
                    paths: next_paths,
                    respond_to,
                } => {
                    paths.extend(next_paths);
                    responders.push(respond_to);
                }
                command => {
                    self.pending = Some(QueuedEngineCommand {
                        command,
                        enqueued_at: queued.enqueued_at,
                    });
                    break;
                }
            }
        }

        // Watcher arrival order carries no semantics. Canonicalize the batch lexically so rebuild
        // logs and downstream traversal stay deterministic, then remove adjacent duplicates.
        paths.sort();
        paths.dedup();
        (paths, responders)
    }
}

/// Owns the engine lane and routes each command to its narrower subsystem.
///
/// `ProjectCoordinator` owns long-lived saved state. A `QueryRunner` is borrowed only for one
/// request, making it impossible for query helpers to outlive the command or retain a project
/// snapshot across the next mutation.
#[derive(Debug)]
pub(super) struct EngineDispatcher {
    project: ProjectCoordinator,
    dirty_state: DirtyState,
    memory_control: Arc<dyn MemoryControl>,
}

impl EngineDispatcher {
    pub(super) fn new(
        sender: Sender<QueuedEngineCommand>,
        memory_control: Arc<dyn MemoryControl>,
        dirty_state: DirtyState,
        notifications: ServiceNotificationsSink,
    ) -> Self {
        Self {
            project: ProjectCoordinator::new(sender, Arc::clone(&memory_control), notifications),
            dirty_state,
            memory_control,
        }
    }

    fn query_runner(&mut self) -> QueryRunner<'_> {
        QueryRunner::new(
            &mut self.project,
            &self.dirty_state,
            Arc::clone(&self.memory_control),
        )
    }

    /// Drain commands until shutdown or until every sender has been dropped.
    ///
    /// Lifecycle commands mutate the coordinator directly. Analysis commands are wrapped in a
    /// `QueryContext`, then delegated to the shared query lifecycle before their response is sent.
    pub(super) fn run(mut self, receiver: Receiver<QueuedEngineCommand>) {
        tracing::debug!("LSP engine dispatcher started");
        let mut queue = CommandQueue::new(receiver);

        while let Some(queued) = queue.next() {
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
                EngineCommand::ProjectPathsChanged { paths, respond_to } => {
                    let (paths, responders) = queue.collect_project_path_changes(paths, respond_to);
                    tracing::trace!(
                        path_count = paths.len(),
                        request_count = responders.len(),
                        "engine command started: project_paths_changed"
                    );
                    Self::respond_to_project_path_changes(
                        responders,
                        self.project.project_paths_changed(paths),
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
                    self.query_runner()
                        .respond_to_query(context, respond_to, |runner| {
                            runner.goto_definition(path.clone(), position, dirty.clone())
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
                    self.query_runner()
                        .respond_to_query(context, respond_to, |runner| {
                            runner.goto_type_definition(path.clone(), position, dirty.clone())
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
                    self.query_runner()
                        .respond_to_query(context, respond_to, |runner| {
                            runner.goto_implementation(path.clone(), position, dirty.clone())
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
                    self.query_runner()
                        .respond_to_query(context, respond_to, |runner| {
                            runner.references(
                                path.clone(),
                                position,
                                include_declaration,
                                dirty.clone(),
                            )
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
                    self.query_runner()
                        .respond_to_query(context, respond_to, |runner| {
                            runner.prepare_rename(path.clone(), position, dirty.clone())
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
                    self.query_runner()
                        .respond_to_query(context, respond_to, |runner| {
                            runner.rename(path.clone(), position, new_name.clone(), dirty.clone())
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
                    self.query_runner()
                        .respond_to_query(context, respond_to, |runner| {
                            runner.document_highlight(path.clone(), position, dirty.clone())
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
                    self.query_runner()
                        .respond_to_query(context, respond_to, |runner| {
                            runner.hover(path.clone(), position, dirty.clone())
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
                    self.query_runner()
                        .respond_to_query(context, respond_to, |runner| {
                            runner.completion(
                                path.clone(),
                                position,
                                client_capabilities,
                                dirty.clone(),
                            )
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
                    self.query_runner()
                        .respond_to_query(context, respond_to, |runner| {
                            runner.formatting(path.clone(), Arc::clone(&text))
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
                    self.query_runner()
                        .respond_to_query(context, respond_to, |runner| {
                            runner.document_symbol(path.clone(), dirty.clone())
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
                    self.query_runner()
                        .respond_to_query(context, respond_to, |runner| {
                            runner.inlay_hint(path.clone(), range, dirty.clone())
                        });
                }
                EngineCommand::WorkspaceSymbol { query, respond_to } => {
                    tracing::trace!(query = %query, "engine command started: workspace_symbol");
                    let context = QueryContext::new("workspace_symbol", queue_elapsed);
                    self.query_runner()
                        .respond_to_query(context, respond_to, |runner| {
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

    /// Answer every request merged into one watcher batch with the same outcome.
    ///
    /// `anyhow::Error` is not cloneable, so the first caller receives the original context chain
    /// and later callers receive an error rebuilt from its fully rendered message.
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
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, sync::mpsc};

    use tokio::sync::oneshot;

    use super::{CommandQueue, EngineDispatcher};
    use crate::engine::{QueuedEngineCommand, command::EngineCommand};

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

        let mut queue = CommandQueue::new(receiver);
        let (paths, responders) =
            queue.collect_project_path_changes(vec![test_path("b")], first_respond_to);

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

        let deferred = queue
            .next()
            .expect("first non-project command should be deferred");
        match deferred.command {
            EngineCommand::WorkspaceSymbol { query, .. } => {
                assert_eq!(query, "needle");
            }
            command => panic!("unexpected deferred command: {command:?}"),
        }

        let queued_after_non_project = queue
            .next()
            .expect("commands after the first non-project command should stay queued");
        match queued_after_non_project.command {
            EngineCommand::ProjectPathsChanged { paths, .. } => {
                assert_eq!(paths, vec![test_path("c")]);
            }
            command => panic!("unexpected command left in queue: {command:?}"),
        }

        EngineDispatcher::respond_to_project_path_changes(responders, Ok(()));
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

        EngineDispatcher::respond_to_project_path_changes(
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

    fn test_path(name: &str) -> PathBuf {
        PathBuf::from(format!("/workspace/src/{name}.rs"))
    }
}
