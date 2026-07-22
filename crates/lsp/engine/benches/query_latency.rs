use std::{
    cell::Cell,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use divan::{Bencher, black_box};
use ls_types::{CompletionItem, Hover, Location, Position};
use rg_lsp_engine::{MemoryControl, Service, ServiceNotificationsSink};
use rg_lsp_proto::{
    AnalysisConfig, CompletionClientCapabilities, EngineConfig, EngineService,
    PackageResidencyPolicy, ServiceNotification, SysrootDiscovery,
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
// Saved and dirty query latency
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

// `didChange` runs as input preparation. The measured completion therefore starts at the same
// boundary as an editor request while still paying for the lazily built dirty overlay.
#[divan::bench(sample_count = 10, sample_size = 1)]
fn completion_dirty_miss(bencher: Bencher<'_, '_>) {
    let engine = PreparedEngine::new();
    let validation_position = engine.prepare_app_dirty();
    expect_report_field_completions(&engine.completion(APP_SOURCE, validation_position));

    bencher
        .with_inputs(|| engine.prepare_app_dirty())
        .bench_local_values(|position| engine.completion(APP_SOURCE, black_box(position)));

    engine.shutdown();
}

// A second query for the same document version reuses the overlay but still goes through normal
// request-memory release and reload behavior between requests.
#[divan::bench(sample_count = 10, sample_size = 1)]
fn completion_dirty_hit(bencher: Bencher<'_, '_>) {
    let engine = PreparedEngine::new();
    let position = engine.prepare_app_dirty();
    expect_report_field_completions(&engine.completion(APP_SOURCE, position));

    bencher.bench_local(|| engine.completion(APP_SOURCE, black_box(position)));

    engine.shutdown();
}

// References require the reverse-dependency closure, so this covers the broad dirty-overlay path
// separately from file-local hover and completion.
#[divan::bench(sample_count = 10, sample_size = 1)]
fn references_dirty_miss(bencher: Bencher<'_, '_>) {
    let engine = PreparedEngine::new();
    let validation_position = engine.prepare_math_dirty();
    expect_mean_references(&engine.references(MATH_SOURCE, validation_position));

    bencher
        .with_inputs(|| engine.prepare_math_dirty())
        .bench_local_values(|position| engine.references(MATH_SOURCE, black_box(position)));

    engine.shutdown();
}

// Editors commonly ask a local question and then a workspace-wide one for the same edit. Preparing
// the hover outside measurement leaves a narrow overlay cached; references must replace it with a
// reverse-dependency overlay inside the measured request.
#[divan::bench(sample_count = 10, sample_size = 1)]
fn references_dirty_scope_upgrade(bencher: Bencher<'_, '_>) {
    let engine = PreparedEngine::new();
    let validation_position = engine.prepare_math_scope_upgrade();
    expect_mean_references(&engine.references(MATH_SOURCE, validation_position));

    bencher
        .with_inputs(|| engine.prepare_math_scope_upgrade())
        .bench_local_values(|position| engine.references(MATH_SOURCE, black_box(position)));

    engine.shutdown();
}

fn expect_report_field_completions(completions: &[CompletionItem]) {
    for expected in ["average", "total"] {
        assert!(
            completions.iter().any(|item| item.label == expected),
            "dirty completion benchmark should contain the `{expected}` field",
        );
    }
}

fn expect_mean_references(locations: &[Location]) {
    assert!(
        locations.len() >= 2,
        "dirty references benchmark should find the declaration and at least one use",
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
    math_dirty_text: [String; 2],
    math_mean_position: [Position; 2],
    clean_hover_position: Position,
    next_document_version: Cell<i32>,
}

impl PreparedEngine {
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
            .map(|text| Self::position_after(text, "query_bench_report."));
        let math_dirty_text = [
            Self::math_dirty_text(&math_saved_text, 1),
            Self::math_dirty_text(&math_saved_text, 2),
        ];
        let math_mean_position = math_dirty_text
            .each_ref()
            .map(|text| Self::position_inside(text, "pub fn mean", "pub fn ".len() + 1));
        let clean_hover_position = Self::position_inside(&app_saved_text, "mean(numbers)", 1);

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("query benchmark Tokio runtime should build");
        let (notifications, receiver) = ServiceNotificationsSink::channel();
        let service = Service::spawn(Arc::new(()) as Arc<dyn MemoryControl>, notifications);
        runtime.block_on(Self::initialize(
            service.clone(),
            receiver,
            &workspace_root,
            [(&app_path, &app_saved_text), (&math_path, &math_saved_text)],
        ));

        Self {
            runtime,
            service,
            workspace_root,
            app_dirty_text,
            app_completion_position,
            math_dirty_text,
            math_mean_position,
            clean_hover_position,
            next_document_version: Cell::new(2),
        }
    }

    /// Publish a settled project before any benchmark is allowed to prepare an edit.
    async fn initialize(
        service: Service,
        mut notifications: UnboundedReceiver<ServiceNotification>,
        workspace_root: &Path,
        opened_sources: [(&Path, &str); 2],
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

        for (path, text) in opened_sources {
            service
                .clone()
                .did_open(
                    context::current(),
                    path.to_path_buf(),
                    Some(1),
                    text.to_string(),
                )
                .await
                .expect("query benchmark source should open");
        }

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

    fn prepare_math_dirty(&self) -> Position {
        let (version, variant) = self.next_version_and_variant();
        self.change_document(MATH_SOURCE, version, self.math_dirty_text[variant].clone());
        self.math_mean_position[variant]
    }

    fn prepare_math_scope_upgrade(&self) -> Position {
        let position = self.prepare_math_dirty();
        let hover = self.hover(MATH_SOURCE, position);
        assert!(
            hover.is_some(),
            "scope-upgrade setup hover should return a result",
        );
        position
    }

    fn change_document(&self, relative_path: &str, version: i32, text: String) {
        self.runtime
            .block_on(self.service.clone().did_change(
                context::current(),
                self.workspace_root.join(relative_path),
                Some(version),
                Some(text),
                1,
            ))
            .expect("query benchmark dirty document should change");
    }

    fn hover(&self, relative_path: &str, position: Position) -> Option<Hover> {
        self.runtime
            .block_on(self.service.clone().hover(
                context::current(),
                self.workspace_root.join(relative_path),
                position,
            ))
            .expect("query benchmark hover should succeed")
    }

    fn completion(&self, relative_path: &str, position: Position) -> Vec<CompletionItem> {
        self.runtime
            .block_on(self.service.clone().completion(
                context::current(),
                self.workspace_root.join(relative_path),
                position,
                CompletionClientCapabilities::default(),
            ))
            .expect("query benchmark completion should succeed")
    }

    fn references(&self, relative_path: &str, position: Position) -> Vec<Location> {
        self.runtime
            .block_on(self.service.clone().references(
                context::current(),
                self.workspace_root.join(relative_path),
                position,
                true,
            ))
            .expect("query benchmark references should succeed")
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
        let import = "use moderate_workspace_text::{collect_unique_words, first_line};\n";
        let with_report = saved.replacen(
            import,
            &format!(
                "{import}\nstruct QueryBenchReport {{\n    total: i64,\n    average: f64,\n}}\n"
            ),
            1,
        );
        assert_ne!(
            with_report, saved,
            "app benchmark import marker should exist"
        );

        let unique = "    let unique = collect_unique_words(body).len();\n";
        let dirty = with_report.replacen(
            unique,
            &format!(
                concat!(
                    "{unique}",
                    "    let query_bench_report = QueryBenchReport {{ total, average: avg }};\n",
                    "    let _query_bench_revision = {revision}_u8;\n",
                    "    let _query_bench_selected = query_bench_report.;\n",
                ),
                unique = unique,
                revision = revision
            ),
            1,
        );
        assert_ne!(
            dirty, with_report,
            "app benchmark expression marker should exist",
        );
        dirty
    }

    fn math_dirty_text(saved: &str, revision: u8) -> String {
        let calculation =
            "    let total = sum(values);\n    Some(total as f64 / values.len() as f64)\n";
        let dirty = saved.replacen(
            calculation,
            &format!(
                concat!(
                    "    let total = sum(values);\n",
                    "    let denominator = values.len() as f64;\n",
                    "    let _query_bench_revision = {revision}_u8;\n",
                    "    Some(total as f64 / denominator)\n",
                ),
                revision = revision
            ),
            1,
        );
        assert_ne!(
            dirty, saved,
            "math benchmark calculation marker should exist"
        );
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
