use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use divan::{Bencher, black_box};
use ls_types::{CompletionItem, Hover, Location, Position};
use rg_lsp_engine::{MemoryControl, Service, ServiceNotificationsSink};
use rg_lsp_proto::{
    AnalysisConfig, AnalysisOutcome, CompletionClientCapabilities, DocumentQueryResult,
    DocumentRevision, EditorDocumentSnapshot, EngineConfig, EngineService, GlobalOperationResult,
    GlobalPositionSnapshot, OpenDocumentSession, OpenDocumentsRevision, PackageResidencyPolicy,
    ServiceNotification, SysrootDiscovery, TargetDocumentRevision,
};
use rg_parse::LineIndex;
use tarpc::context;
use tokio::{runtime::Runtime, sync::mpsc::UnboundedReceiver};

const APP_SOURCE: &str = "crates/app/src/lib.rs";
const MATH_SOURCE: &str = "crates/math/src/lib.rs";
const DEFERRED_INDEXING_TIMEOUT: Duration = Duration::from_secs(120);

fn main() {
    divan::main();
}

// =============================================================================
// Saved and current-source query latency
// =============================================================================

// Initialization is intentionally outside Divan's measured closure. Every query begins from a
// settled all-offloadable project, which is the normal low-idle-memory LSP configuration.
#[divan::bench(sample_count = 10, sample_size = 1)]
fn hover_clean(bencher: Bencher<'_, '_>) {
    let engine = PreparedEngine::new();
    assert!(
        engine
            .hover(APP_SOURCE, engine.clean_hover_position)
            .is_some(),
        "clean hover benchmark should return a result",
    );

    bencher.bench_local(|| engine.hover(APP_SOURCE, black_box(engine.clean_hover_position)));

    engine.shutdown();
}

// Input preparation alternates between two document revisions. The measured query therefore pays
// for rebuilding the edited body, while declarations and traits still come from the saved project.
#[divan::bench(sample_count = 10, sample_size = 1)]
fn completion_current_body_after_edit(bencher: Bencher<'_, '_>) {
    let engine = PreparedEngine::new();
    let validation_position = engine.prepare_app_dirty();
    expect_current_body_completions(&engine.completion(APP_SOURCE, validation_position));

    bencher
        .with_inputs(|| engine.prepare_app_dirty())
        .bench_local_values(|position| engine.completion(APP_SOURCE, black_box(position)));

    engine.shutdown();
}

// A repeated query for one document revision rebuilds only its request-local current-body view.
// This catches accidental retained caches as well as avoidable repeated-query overhead.
#[divan::bench(sample_count = 10, sample_size = 1)]
fn completion_current_body_repeated(bencher: Bencher<'_, '_>) {
    let engine = PreparedEngine::new();
    let position = engine.prepare_app_dirty();
    expect_current_body_completions(&engine.completion(APP_SOURCE, position));

    bencher.bench_local(|| engine.completion(APP_SOURCE, black_box(position)));

    engine.shutdown();
}

// References read the saved global index and verify that every open document still matches it.
#[divan::bench(sample_count = 10, sample_size = 1)]
fn references_saved(bencher: Bencher<'_, '_>) {
    let engine = PreparedEngine::new();
    let validation_position = engine.math_mean_position;
    expect_mean_references(&engine.references(MATH_SOURCE, validation_position));

    bencher.bench_local(|| engine.references(MATH_SOURCE, black_box(validation_position)));

    engine.shutdown();
}

fn expect_current_body_completions(completions: &[CompletionItem]) {
    assert!(
        completions
            .iter()
            .any(|item| item.label == "query_bench_numbers"),
        "current-body completion benchmark should contain its newly typed local",
    );
}

fn expect_mean_references(locations: &[Location]) {
    assert!(
        locations.len() >= 2,
        "references benchmark should find the declaration and at least one use",
    );
}

// =============================================================================
// Prepared engine state
// =============================================================================

struct PreparedEngine {
    runtime: Runtime,
    service: Service,
    workspace_root: PathBuf,
    app_dirty_text: [String; 2],
    app_completion_position: [Position; 2],
    math_mean_position: Position,
    clean_hover_position: Position,
    next_document_version: Cell<i32>,
    documents: RefCell<HashMap<PathBuf, (i32, String)>>,
}

impl PreparedEngine {
    fn expect_ready<T>(outcome: AnalysisOutcome<T>, operation: &str) -> T {
        match outcome {
            AnalysisOutcome::Ready(ready) => ready.into_value(),
            AnalysisOutcome::Aborted(abort) => {
                panic!("query benchmark {operation} aborted unexpectedly: {abort:?}")
            }
        }
    }

    fn expect_document_ready<T>(
        outcome: AnalysisOutcome<DocumentQueryResult<T>>,
        operation: &str,
    ) -> T {
        Self::expect_ready(outcome, operation).into_value()
    }

    fn new() -> Self {
        let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../test_targets/moderate_workspace")
            .canonicalize()
            .expect("moderate workspace benchmark fixture should exist");
        let app_path = workspace_root.join(APP_SOURCE);
        let math_path = workspace_root.join(MATH_SOURCE);
        let app_saved_text =
            std::fs::read_to_string(&app_path).expect("app benchmark source should be readable");
        let math_saved_text =
            std::fs::read_to_string(&math_path).expect("math benchmark source should be readable");
        let app_dirty_text = [
            Self::app_dirty_text(&app_saved_text, 1),
            Self::app_dirty_text(&app_saved_text, 2),
        ];
        let app_completion_position = app_dirty_text
            .each_ref()
            .map(|text| Self::position_after(text, "let _query_bench_selected = query_bench_"));
        let math_mean_position =
            Self::position_inside(&math_saved_text, "pub fn mean", "pub fn ".len() + 1);
        let clean_hover_position = Self::position_inside(&app_saved_text, "mean(numbers)", 1);

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("query benchmark Tokio runtime should build");
        let (notifications, receiver) = ServiceNotificationsSink::channel();
        let service = Service::spawn(Arc::new(()) as Arc<dyn MemoryControl>, notifications);
        runtime.block_on(Self::initialize(service.clone(), receiver, &workspace_root));

        Self {
            runtime,
            service,
            workspace_root,
            app_dirty_text,
            app_completion_position,
            math_mean_position,
            clean_hover_position,
            next_document_version: Cell::new(2),
            documents: RefCell::default(),
        }
    }

    /// Publish a settled project before any benchmark is allowed to prepare an edit.
    async fn initialize(
        service: Service,
        mut notifications: UnboundedReceiver<ServiceNotification>,
        workspace_root: &Path,
    ) {
        let config = EngineConfig {
            analysis: AnalysisConfig {
                package_residency_policy: PackageResidencyPolicy::AllOffloadable,
                // Keep this benchmark fixture-local and independent of whether a CI or developer
                // toolchain happens to have rust-src installed.
                sysroot_discovery: SysrootDiscovery::Disabled,
                ..AnalysisConfig::default()
            },
            ..EngineConfig::default()
        };
        service
            .clone()
            .initialize(context::current(), workspace_root.to_path_buf(), config)
            .await
            .expect("query benchmark engine should initialize");
        service
            .clone()
            .initialized(context::current())
            .await
            .expect("query benchmark engine should accept initialized notification");

        tokio::time::timeout(DEFERRED_INDEXING_TIMEOUT, async {
            loop {
                let notification = notifications
                    .recv()
                    .await
                    .expect("query benchmark notification channel should remain open");
                if matches!(
                    notification,
                    ServiceNotification::DeferredIndexingFinished { root }
                        if root == workspace_root
                ) {
                    break;
                }
            }
        })
        .await
        .expect("query benchmark deferred indexing should finish before its deadline");

        // Later lifecycle notifications are deliberately ignored, but keeping the receiver active
        // preserves the service's ordinary fire-and-forget behavior during measured requests.
        tokio::spawn(async move { while notifications.recv().await.is_some() {} });
    }

    fn prepare_app_dirty(&self) -> Position {
        let (version, variant) = self.next_version_and_variant();
        self.change_document(APP_SOURCE, version, self.app_dirty_text[variant].clone());
        self.app_completion_position[variant]
    }

    fn change_document(&self, relative_path: &str, version: i32, text: String) {
        self.documents
            .borrow_mut()
            .insert(self.workspace_root.join(relative_path), (version, text));
    }

    fn hover(&self, relative_path: &str, position: Position) -> Option<Hover> {
        let input = self
            .document_snapshot(relative_path)
            .with_position(position);
        let outcome = self
            .runtime
            .block_on(self.service.clone().hover(context::current(), input))
            .expect("query benchmark hover should succeed");
        Self::expect_document_ready(outcome, "hover")
    }

    fn completion(&self, relative_path: &str, position: Position) -> Vec<CompletionItem> {
        let input = self
            .document_snapshot(relative_path)
            .with_position(position);
        let outcome = self
            .runtime
            .block_on(self.service.clone().completion(
                context::current(),
                input,
                CompletionClientCapabilities::default(),
            ))
            .expect("query benchmark completion should succeed");
        Self::expect_ready(outcome, "completion").into_value()
    }

    fn references(&self, relative_path: &str, position: Position) -> Vec<Location> {
        let input = self.global_position_snapshot(relative_path, position);
        let outcome = self
            .runtime
            .block_on(
                self.service
                    .clone()
                    .references(context::current(), input, true),
            )
            .expect("query benchmark references should succeed");
        match Self::expect_ready(outcome, "references") {
            GlobalOperationResult::Ready(locations) => locations,
            GlobalOperationResult::SaveRequired { path } => {
                panic!(
                    "clean references benchmark unexpectedly requires saving {}",
                    path.display(),
                )
            }
        }
    }

    fn capture_open_documents(
        &self,
        relative_path: &str,
    ) -> (
        TargetDocumentRevision,
        OpenDocumentsRevision,
        Vec<EditorDocumentSnapshot>,
    ) {
        let path = self.workspace_root.join(relative_path);
        let mut documents = self.documents.borrow().clone();
        let (target_version, _) = documents.entry(path.clone()).or_insert_with(|| {
            (
                1,
                std::fs::read_to_string(&path).expect("query benchmark source should be readable"),
            )
        });
        let target_revision =
            u64::try_from(*target_version).expect("benchmark versions should be positive");
        let open_documents_revision = documents
            .values()
            .map(|(version, _)| *version)
            .max()
            .and_then(|version| u64::try_from(version).ok())
            .unwrap_or(target_revision);
        let open_documents = documents
            .into_iter()
            .map(|(path, (version, text))| {
                let revision =
                    u64::try_from(version).expect("benchmark versions should be positive");
                EditorDocumentSnapshot::new(
                    path,
                    OpenDocumentSession::new(1),
                    DocumentRevision::new(revision),
                    Some(version),
                    text,
                )
            })
            .collect();
        let target = TargetDocumentRevision::new(
            path,
            OpenDocumentSession::new(1),
            DocumentRevision::new(target_revision),
        );
        (
            target,
            OpenDocumentsRevision::new(open_documents_revision),
            open_documents,
        )
    }

    fn document_snapshot(&self, relative_path: &str) -> EditorDocumentSnapshot {
        let (target, _, documents) = self.capture_open_documents(relative_path);
        documents
            .into_iter()
            .find(|document| document.target() == &target)
            .expect("query benchmark snapshot should contain its target")
    }

    fn global_position_snapshot(
        &self,
        relative_path: &str,
        position: Position,
    ) -> GlobalPositionSnapshot {
        let (target, open_documents_revision, documents) =
            self.capture_open_documents(relative_path);
        GlobalPositionSnapshot::new(target, open_documents_revision, documents, position)
    }

    fn shutdown(&self) {
        self.runtime
            .block_on(self.service.clone().shutdown(context::current()))
            .expect("query benchmark engine should shut down");
    }

    fn next_version_and_variant(&self) -> (i32, usize) {
        let version = self.next_document_version.get();
        self.next_document_version.set(
            version
                .checked_add(1)
                .expect("query benchmark document version should remain in range"),
        );
        let variant = usize::try_from(version)
            .expect("query benchmark document versions should stay positive")
            % 2;
        (version, variant)
    }

    fn app_dirty_text(saved: &str, revision: u8) -> String {
        let unique = "    let unique = collect_unique_words(body).len();\n";
        let dirty = saved.replacen(
            unique,
            &format!(
                concat!(
                    "{unique}",
                    "    let query_bench_numbers = numbers;\n",
                    "    let _query_bench_revision = {revision}_u8;\n",
                    "    let _query_bench_selected = query_bench_;\n",
                ),
                unique = unique,
                revision = revision
            ),
            1,
        );
        assert_ne!(dirty, saved, "app benchmark expression marker should exist",);
        dirty
    }

    fn position_after(text: &str, marker: &str) -> Position {
        Self::position_inside(text, marker, marker.len())
    }

    fn position_inside(text: &str, marker: &str, delta: usize) -> Position {
        let marker_offset = text
            .find(marker)
            .unwrap_or_else(|| panic!("query benchmark marker `{marker}` should exist"));
        let offset = marker_offset
            .checked_add(delta)
            .expect("query benchmark marker offset should remain in range");
        let offset =
            u32::try_from(offset).expect("query benchmark marker offset should fit into u32");
        let position = LineIndex::new(text).utf16_position(offset);
        Position::new(position.line, position.column)
    }
}
