use std::{
    collections::HashMap,
    fmt::Write as _,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use expect_test::Expect;
use ls_types::{
    CompletionItem, CompletionTextEdit, DocumentHighlight, DocumentHighlightKind, DocumentSymbol,
    Hover, HoverContents, InlayHint, InlayHintKind, InlayHintLabel, Location, Position, Range,
    TextEdit, WorkspaceEdit,
};
use rg_lsp_proto::{
    AnalysisConfig, CapturedSourceInput, CompletionClientCapabilities, DocumentRevision,
    EditorDocumentSnapshot, EngineConfig, EngineResult, EngineService, GlobalPositionSnapshot,
    OpenDocumentSession, OpenDocumentsRevision, QueryError, QueryValue, SaveProposal,
    SavedProjectChanges, ServiceNotification, SysrootDiscovery, TargetDocumentRevision,
};
use rg_parse::LineIndex;
use tarpc::context;
use test_fixture::{
    CrateFixture, FixtureMarkers, fixture_crate_with_markers, testonly::MarkedText,
};

use crate::{
    MemoryControl, Service, ServiceNotificationsSink, service::ServiceNotificationPublisher,
};

pub(super) struct LspEngineFixture {
    fixture: CrateFixture,
    markers: FixtureMarkers,
    service: Service,
    notifications: RecordingNotifications,
    documents: Mutex<HashMap<PathBuf, TestDocument>>,
    next_revision: AtomicU64,
}

impl LspEngineFixture {
    fn expect_save_required<T>(
        outcome: Result<QueryValue<T>, QueryError>,
        operation: &str,
    ) -> PathBuf {
        match outcome {
            Err(QueryError::SaveRequired { path }) => path,
            Ok(_) => panic!("{operation} unexpectedly ran against unsaved source"),
            Err(error) => panic!("{operation} failed unexpectedly: {error}"),
        }
    }

    pub(super) async fn initialized(fixture: &str) -> Self {
        Self::initialized_with_engine_config(fixture, Self::engine_config()).await
    }

    pub(super) async fn initialized_with_cfg_test(fixture: &str, enabled: bool) -> Self {
        let mut config = Self::engine_config();
        config.analysis.cfg.test = enabled;
        Self::initialized_with_engine_config(fixture, config).await
    }

    fn new(fixture: &str) -> Self {
        let (fixture, markers) = fixture_crate_with_markers(fixture);
        let notifications = RecordingNotifications::default();
        let service = Service::spawn(
            Arc::new(()) as Arc<dyn MemoryControl>,
            ServiceNotificationsSink::from_publisher(notifications.clone()),
        );

        Self {
            fixture,
            markers,
            service,
            notifications,
            documents: Mutex::default(),
            next_revision: AtomicU64::new(1),
        }
    }

    async fn initialized_with_engine_config(fixture: &str, config: EngineConfig) -> Self {
        let fixture = Self::new(fixture);
        fixture
            .service
            .clone()
            .initialize(context::current(), fixture.fixture.path(""), config)
            .await
            .expect("fixture LSP engine should initialize");
        tokio::time::timeout(
            Duration::from_secs(5),
            fixture.notifications.wait_for_deferred_indexing(),
        )
        .await
        .expect("fixture deferred indexing should finish");
        fixture
    }

    fn engine_config() -> EngineConfig {
        // These flow tests exercise fixture-local LSP behavior. Real rust-src indexing is covered
        // by lower-level sysroot tests and would dominate every tiny protocol fixture.
        EngineConfig {
            analysis: AnalysisConfig {
                sysroot_discovery: SysrootDiscovery::Disabled,
                ..AnalysisConfig::default()
            },
            ..EngineConfig::default()
        }
    }

    pub(super) async fn check(&self, queries: &[LspQuery], expect: Expect) {
        self.check_with_markers(QueryMarkers::Saved, queries, expect)
            .await;
    }

    pub(super) async fn check_dirty(
        &self,
        dirty: &DirtyDocument,
        queries: &[LspQuery],
        expect: Expect,
    ) {
        self.check_with_markers(QueryMarkers::Dirty(dirty), queries, expect)
            .await;
    }

    async fn check_with_markers(
        &self,
        markers: QueryMarkers<'_>,
        queries: &[LspQuery],
        expect: Expect,
    ) {
        let mut rendered = String::new();

        for (idx, query) in queries.iter().enumerate() {
            if idx > 0 {
                rendered.push('\n');
            }

            self.render_query(&mut rendered, markers, query).await;
        }

        expect.assert_eq(&rendered);
    }

    pub(super) async fn did_open_saved(&self, path: &str, version: i32) {
        let text = std::fs::read_to_string(self.fixture.path(path))
            .expect("fixture saved document should be readable");
        self.record_document(self.fixture.path(path), Some(version), text);
    }

    pub(super) async fn did_open_dirty(
        &self,
        path: &'static str,
        version: i32,
        text: MarkedText,
    ) -> DirtyDocument {
        self.record_document(
            self.fixture.path(path),
            Some(version),
            text.text().to_string(),
        );

        DirtyDocument { path, text }
    }

    pub(super) async fn did_change_full(
        &self,
        path: &'static str,
        version: i32,
        text: MarkedText,
    ) -> DirtyDocument {
        self.record_document(
            self.fixture.path(path),
            Some(version),
            text.text().to_string(),
        );

        DirtyDocument { path, text }
    }

    fn record_document(&self, path: PathBuf, version: Option<i32>, text: String) {
        let revision = self.next_revision.fetch_add(1, Ordering::Relaxed);
        let source_path = rg_std::path::canonicalize(&path).unwrap_or_else(|_| path.clone());
        self.documents
            .lock()
            .expect("fixture editor documents should not be poisoned")
            .insert(
                path,
                TestDocument {
                    source_path,
                    version,
                    text,
                    revision,
                },
            );
    }

    fn capture_open_documents(
        &self,
        path: PathBuf,
    ) -> (
        TargetDocumentRevision,
        OpenDocumentsRevision,
        Vec<EditorDocumentSnapshot>,
    ) {
        let mut documents = self
            .documents
            .lock()
            .expect("fixture editor documents should not be poisoned")
            .clone();
        let target = documents
            .entry(path.clone())
            .or_insert_with(|| TestDocument {
                source_path: rg_std::path::canonicalize(&path).unwrap_or_else(|_| path.clone()),
                version: None,
                text: std::fs::read_to_string(&path)
                    .expect("fixture query document should be readable"),
                revision: 1,
            });
        let target_revision = DocumentRevision::new(target.revision);
        let open_documents_revision = documents
            .values()
            .map(|document| document.revision)
            .max()
            .unwrap_or(1);
        let open_documents = documents
            .into_iter()
            .map(|(path, document)| {
                EditorDocumentSnapshot::new(
                    path,
                    OpenDocumentSession::new(1),
                    DocumentRevision::new(document.revision),
                    document.version,
                    document.text,
                )
                .with_source_path(document.source_path)
            })
            .collect();
        let target =
            TargetDocumentRevision::new(path, OpenDocumentSession::new(1), target_revision);

        (
            target,
            OpenDocumentsRevision::new(open_documents_revision),
            open_documents,
        )
    }

    fn document_snapshot(&self, path: PathBuf) -> EditorDocumentSnapshot {
        let (target, _, documents) = self.capture_open_documents(path);
        documents
            .into_iter()
            .find(|document| document.target() == &target)
            .expect("fixture target should be present in its captured open documents")
    }

    fn global_position_snapshot(
        &self,
        path: PathBuf,
        position: Position,
    ) -> GlobalPositionSnapshot {
        let (target, open_documents_revision, documents) = self.capture_open_documents(path);
        GlobalPositionSnapshot::new(target, open_documents_revision, documents, position)
    }
    pub(super) async fn did_save_dirty(&self, dirty: &DirtyDocument) {
        let path = self.fixture.path(dirty.path);
        std::fs::write(&path, dirty.text.text())
            .expect("fixture dirty document should be writable before save");
        self.did_save_current(dirty.path)
            .await
            .expect("fixture dirty document should save");
    }

    pub(super) async fn did_save_current(&self, path: &str) -> EngineResult<u64> {
        self.notifications.clear();
        let document = self.document_snapshot(self.fixture.path(path));
        let proposal = SaveProposal::new(
            document.target().clone(),
            document.client_version(),
            document.text().to_string(),
        );

        self.service
            .clone()
            .did_save(context::current(), proposal)
            .await
    }

    pub(super) async fn external_file_changed(&self, path: &str, text: &str) {
        self.notifications.clear();
        std::fs::write(self.fixture.path(path), text)
            .expect("fixture external change should be writable");
        let path = self.fixture.path(path);
        let changes = if path.extension().and_then(std::ffi::OsStr::to_str) == Some("rs") {
            SavedProjectChanges::new(
                vec![CapturedSourceInput::new(path, text.to_string())],
                Vec::new(),
            )
        } else {
            SavedProjectChanges::new(Vec::new(), vec![path])
        };

        self.service
            .clone()
            .external_project_changes(context::current(), changes)
            .await
            .expect("fixture external file change should apply");
    }

    pub(super) fn write_file_without_notification(&self, path: &str, text: &str) {
        std::fs::write(self.fixture.path(path), text)
            .expect("fixture source should be writable without a notification");
    }

    pub(super) fn remove_file_without_notification(&self, path: &str) {
        std::fs::remove_file(self.fixture.path(path))
            .expect("fixture source should be removable without a notification");
    }

    pub(super) fn check_notification_effects(&self, expect: Expect) {
        let mut rendered = String::new();
        writeln!(rendered, "notifications").expect("snapshot should be writable");

        let mut rendered_any_notification = false;
        let mut rendered_inlay_hint_refresh = false;
        for notification in self.notifications.snapshot() {
            match notification {
                ServiceNotification::PublishDiagnostics {
                    path,
                    diagnostics,
                    saved_text,
                } => {
                    rendered_any_notification = true;
                    writeln!(
                        rendered,
                        "- publish diagnostics {} saved-text-len {:?} count {}",
                        self.render_path(path.as_path()),
                        saved_text.as_ref().map(String::len),
                        diagnostics.len(),
                    )
                    .expect("snapshot should be writable");
                }
                ServiceNotification::BeginWorkDoneProgress {
                    token,
                    title,
                    message,
                } => {
                    rendered_any_notification = true;
                    writeln!(
                        rendered,
                        "- begin progress {token:?}: {title}{}",
                        message
                            .as_deref()
                            .map(|message| format!(" ({message})"))
                            .unwrap_or_default(),
                    )
                    .expect("snapshot should be writable");
                }
                ServiceNotification::EndWorkDoneProgress { token, message } => {
                    rendered_any_notification = true;
                    writeln!(
                        rendered,
                        "- end progress {token:?}{}",
                        message
                            .as_deref()
                            .map(|message| format!(" ({message})"))
                            .unwrap_or_default(),
                    )
                    .expect("snapshot should be writable");
                }
                ServiceNotification::InlayHintRefresh => {
                    // This snapshot records stable editor-facing effects, not the exact event log.
                    // Several external paths can be reduced into one project replacement, so
                    // duplicate invalidations are intentionally collapsed.
                    if !rendered_inlay_hint_refresh {
                        rendered_any_notification = true;
                        writeln!(rendered, "- inlay hint refresh")
                            .expect("snapshot should be writable");
                        rendered_inlay_hint_refresh = true;
                    }
                }
                ServiceNotification::DeferredIndexingStarted { .. }
                | ServiceNotification::DeferredIndexingFinished { .. } => {
                    // Initial deferred indexing finishes on a background thread, so this lifecycle
                    // notification can race with later fixture operations. These snapshots describe
                    // stable editor-facing effects of the operation under test, not the whole event
                    // stream.
                }
                ServiceNotification::LogMessage { level, message } => {
                    rendered_any_notification = true;
                    writeln!(rendered, "- log {level:?}: {message}")
                        .expect("snapshot should be writable");
                }
            }
        }
        if !rendered_any_notification {
            writeln!(rendered, "- none").expect("snapshot should be writable");
        }

        expect.assert_eq(&rendered);
    }

    pub(super) async fn check_rename(
        &self,
        title: &'static str,
        marker: &'static str,
        new_name: &'static str,
        expect: Expect,
    ) {
        self.check_rename_with_markers(QueryMarkers::Saved, title, marker, new_name, expect)
            .await;
    }

    pub(super) async fn check_dirty_global_operations_require_save(
        &self,
        dirty: &DirtyDocument,
        marker: &'static str,
        new_name: &'static str,
    ) {
        let path = self.marker_path(QueryMarkers::Dirty(dirty), marker);
        let position = self.marker_position(QueryMarkers::Dirty(dirty), marker);
        let references = self
            .service
            .clone()
            .references(
                context::current(),
                self.global_position_snapshot(path.clone(), position),
                true,
            )
            .await;
        let implementation = self
            .service
            .clone()
            .goto_implementation(
                context::current(),
                self.global_position_snapshot(path.clone(), position),
            )
            .await;
        let prepare_rename = self
            .service
            .clone()
            .prepare_rename(
                context::current(),
                self.global_position_snapshot(path.clone(), position),
            )
            .await;
        let rename = self
            .service
            .clone()
            .rename(
                context::current(),
                self.global_position_snapshot(path.clone(), position),
                new_name.to_string(),
            )
            .await;

        for required_path in [
            Self::expect_save_required(references, "dirty references query"),
            Self::expect_save_required(implementation, "dirty implementation query"),
            Self::expect_save_required(prepare_rename, "dirty prepare-rename query"),
            Self::expect_save_required(rename, "dirty rename query"),
        ] {
            assert_eq!(required_path, path);
        }
    }

    pub(super) async fn check_rename_after_save(
        &self,
        document: &DirtyDocument,
        title: &'static str,
        marker: &'static str,
        new_name: &'static str,
        expect: Expect,
    ) {
        self.check_rename_with_markers(
            QueryMarkers::Dirty(document),
            title,
            marker,
            new_name,
            expect,
        )
        .await;
    }

    async fn check_rename_with_markers(
        &self,
        markers: QueryMarkers<'_>,
        title: &'static str,
        marker: &'static str,
        new_name: &'static str,
        expect: Expect,
    ) {
        let path = self.marker_path(markers, marker);
        let position = self.marker_position(markers, marker);
        let input = self.global_position_snapshot(path, position);
        let outcome = self
            .service
            .clone()
            .rename(context::current(), input, new_name.to_string())
            .await
            .expect("rename query should succeed");
        let edit = outcome.into_value();

        let mut rendered = String::new();
        writeln!(rendered, "{title}").expect("snapshot should be writable");
        match edit {
            Some(edit) => self.render_workspace_edit(&mut rendered, &edit),
            None => writeln!(rendered, "- none").expect("snapshot should be writable"),
        }
        expect.assert_eq(&rendered);
    }

    pub(super) async fn check_rename_error(
        &self,
        marker: &'static str,
        new_name: &'static str,
        expected: QueryError,
    ) {
        let path = self.marker_path(QueryMarkers::Saved, marker);
        let position = self.marker_position(QueryMarkers::Saved, marker);
        let input = self.global_position_snapshot(path, position);
        let outcome = self
            .service
            .clone()
            .rename(context::current(), input, new_name.to_string())
            .await;

        assert_eq!(outcome, Err(expected));
    }

    pub(super) async fn check_formatting(
        &self,
        title: &'static str,
        path: &'static str,
        expect: Expect,
    ) {
        let outcome = self
            .service
            .clone()
            .formatting(
                context::current(),
                self.document_snapshot(self.fixture.path(path)),
            )
            .await
            .expect("formatting query should succeed");
        let edits = outcome.into_value();

        let mut rendered = String::new();
        writeln!(rendered, "{title}").expect("snapshot should be writable");
        self.render_formatting_edits(&mut rendered, path, edits.as_deref());
        expect.assert_eq(&rendered);
    }

    pub(super) async fn shutdown(&self) {
        self.service
            .clone()
            .shutdown(context::current())
            .await
            .expect("fixture LSP engine should shut down");
    }

    async fn render_query(
        &self,
        rendered: &mut String,
        markers: QueryMarkers<'_>,
        query: &LspQuery,
    ) {
        match query {
            LspQuery::GotoDefinition { title, marker } => {
                let path = self.marker_path(markers, marker);
                let position = self.marker_position(markers, marker);
                let input = self.global_position_snapshot(path, position);
                let outcome = self
                    .service
                    .clone()
                    .goto_definition(context::current(), input)
                    .await
                    .expect("goto definition query should succeed");
                let locations = outcome.into_value();

                writeln!(rendered, "{title}").expect("snapshot should be writable");
                self.render_locations(rendered, &locations);
            }
            LspQuery::GotoTypeDefinition { title, marker } => {
                let path = self.marker_path(markers, marker);
                let position = self.marker_position(markers, marker);
                let input = self.global_position_snapshot(path, position);
                let outcome = self
                    .service
                    .clone()
                    .goto_type_definition(context::current(), input)
                    .await
                    .expect("goto type definition query should succeed");
                let locations = outcome.into_value();

                writeln!(rendered, "{title}").expect("snapshot should be writable");
                self.render_locations(rendered, &locations);
            }
            LspQuery::GotoImplementation { title, marker } => {
                let path = self.marker_path(markers, marker);
                let position = self.marker_position(markers, marker);
                let input = self.global_position_snapshot(path, position);
                let outcome = self
                    .service
                    .clone()
                    .goto_implementation(context::current(), input)
                    .await
                    .expect("goto implementation query should succeed");
                let locations = outcome.into_value();

                writeln!(rendered, "{title}").expect("snapshot should be writable");
                self.render_locations(rendered, &locations);
            }
            LspQuery::References {
                title,
                marker,
                include_declaration,
            } => {
                let path = self.marker_path(markers, marker);
                let position = self.marker_position(markers, marker);
                let input = self.global_position_snapshot(path, position);
                let outcome = self
                    .service
                    .clone()
                    .references(context::current(), input, *include_declaration)
                    .await
                    .expect("references query should succeed");
                let locations = outcome.into_value();

                writeln!(rendered, "{title}").expect("snapshot should be writable");
                self.render_locations(rendered, &locations);
            }
            LspQuery::Hover { title, marker } => {
                let path = self.marker_path(markers, marker);
                let position = self.marker_position(markers, marker);
                let input = self.document_snapshot(path.clone()).with_position(position);
                let outcome = self
                    .service
                    .clone()
                    .hover(context::current(), input)
                    .await
                    .expect("hover query should succeed");
                let hover = outcome.into_value();

                writeln!(rendered, "{title}").expect("snapshot should be writable");
                self.render_hover(rendered, path.as_path(), hover.as_ref());
            }
            LspQuery::DocumentHighlight { title, marker } => {
                let path = self.marker_path(markers, marker);
                let position = self.marker_position(markers, marker);
                let input = self.document_snapshot(path).with_position(position);
                let outcome = self
                    .service
                    .clone()
                    .document_highlight(context::current(), input)
                    .await
                    .expect("document highlight query should succeed");
                let highlights = outcome.into_value();

                writeln!(rendered, "{title}").expect("snapshot should be writable");
                Self::render_document_highlights(rendered, &highlights);
            }
            LspQuery::Completion { title, marker } => {
                let path = self.marker_path(markers, marker);
                let position = self.marker_position(markers, marker);
                let document = self.document_snapshot(path.clone());
                let current_text = document.text().to_string();
                let input = document.with_position(position);
                let outcome = self
                    .service
                    .clone()
                    .completion(
                        context::current(),
                        input,
                        CompletionClientCapabilities::default(),
                    )
                    .await
                    .expect("completion query should succeed");
                let completions = outcome.into_value();
                Self::assert_completion_edits_fit_source(&current_text, &completions);

                writeln!(rendered, "{title}").expect("snapshot should be writable");
                self.render_completions(rendered, path.as_path(), &completions);
            }
            LspQuery::DocumentSymbol { title, path } => {
                let document = self.document_snapshot(self.fixture.path(path));
                let outcome = self
                    .service
                    .clone()
                    .document_symbol(context::current(), document)
                    .await
                    .expect("document symbol query should succeed");
                let symbols = outcome.into_value();

                writeln!(rendered, "{title}").expect("snapshot should be writable");
                self.render_document_symbols(rendered, &symbols, 0);
            }
            LspQuery::InlayHint {
                title,
                path,
                start,
                end,
            } => {
                let path = self.fixture.path(path);
                let range = Range::new(
                    self.marker_position(markers, start),
                    self.marker_position(markers, end),
                );
                let input = self.document_snapshot(path).with_range(range);
                let outcome = self
                    .service
                    .clone()
                    .inlay_hint(context::current(), input)
                    .await
                    .expect("inlay hint query should succeed");
                let hints = outcome.into_value();

                writeln!(rendered, "{title}").expect("snapshot should be writable");
                Self::render_inlay_hints(rendered, &hints);
            }
        }
    }

    fn marker_path(&self, markers: QueryMarkers<'_>, marker: &str) -> std::path::PathBuf {
        match markers {
            QueryMarkers::Saved => {
                let marker = self.markers.position(marker);
                self.fixture.path(&marker.path)
            }
            QueryMarkers::Dirty(document) => self.fixture.path(document.path),
        }
    }

    fn marker_position(&self, markers: QueryMarkers<'_>, marker: &str) -> Position {
        let position = match markers {
            QueryMarkers::Saved => {
                let marker = self.markers.position(marker);
                let text = std::fs::read_to_string(self.fixture.path(&marker.path))
                    .expect("fixture marker file should be readable");
                LineIndex::new(&text).utf16_position(marker.offset)
            }
            QueryMarkers::Dirty(document) => {
                let offset = u32::try_from(document.text.offset(marker))
                    .expect("dirty marker offset should fit into u32");
                LineIndex::new(document.text.text()).utf16_position(offset)
            }
        };

        Position::new(position.line, position.column)
    }

    fn render_locations(&self, rendered: &mut String, locations: &[Location]) {
        if locations.is_empty() {
            writeln!(rendered, "- none").expect("snapshot should be writable");
            return;
        }

        for location in locations {
            writeln!(rendered, "- {}", self.render_location(location))
                .expect("snapshot should be writable");
        }
    }

    fn render_hover(&self, rendered: &mut String, path: &Path, hover: Option<&Hover>) {
        let Some(hover) = hover else {
            writeln!(rendered, "- none").expect("snapshot should be writable");
            return;
        };

        if let Some(range) = hover.range {
            writeln!(
                rendered,
                "- range: {}:{}",
                self.render_path(path),
                Self::render_range(range),
            )
            .expect("snapshot should be writable");
        }

        writeln!(rendered, "- markdown:").expect("snapshot should be writable");
        match &hover.contents {
            HoverContents::Markup(markup) => Self::write_indented(rendered, &markup.value, "  "),
            HoverContents::Scalar(marked) => {
                Self::write_indented(rendered, &format!("{marked:?}"), "  ")
            }
            HoverContents::Array(marked) => {
                for value in marked {
                    Self::write_indented(rendered, &format!("{value:?}"), "  ");
                }
            }
        }
    }

    fn render_document_highlights(rendered: &mut String, highlights: &[DocumentHighlight]) {
        if highlights.is_empty() {
            writeln!(rendered, "- none").expect("snapshot should be writable");
            return;
        }

        for highlight in highlights {
            let kind = match highlight.kind {
                Some(DocumentHighlightKind::READ) => "read",
                Some(DocumentHighlightKind::WRITE) => "write",
                Some(DocumentHighlightKind::TEXT) | None => "text",
                Some(_) => "unknown",
            };
            writeln!(rendered, "- {kind} {}", Self::render_range(highlight.range))
                .expect("snapshot should be writable");
        }
    }

    fn render_inlay_hints(rendered: &mut String, hints: &[InlayHint]) {
        if hints.is_empty() {
            writeln!(rendered, "- none").expect("snapshot should be writable");
            return;
        }

        for hint in hints {
            let label = match &hint.label {
                InlayHintLabel::String(label) => label.clone(),
                InlayHintLabel::LabelParts(parts) => {
                    parts.iter().map(|part| part.value.as_str()).collect()
                }
            };
            let kind = match hint.kind {
                Some(InlayHintKind::TYPE) => "type",
                Some(InlayHintKind::PARAMETER) => "parameter",
                Some(_) | None => "text",
            };
            writeln!(
                rendered,
                "- `{label}` {kind} @ {}:{}",
                hint.position.line, hint.position.character
            )
            .expect("snapshot should be writable");
        }
    }

    fn render_completions(
        &self,
        rendered: &mut String,
        path: &Path,
        completions: &[CompletionItem],
    ) {
        if completions.is_empty() {
            writeln!(rendered, "- none").expect("snapshot should be writable");
            return;
        }

        for completion in completions {
            let kind = completion
                .kind
                .map(|kind| format!("{kind:?}"))
                .unwrap_or_else(|| "Unknown".to_string());
            writeln!(rendered, "- {} {kind}", completion.label)
                .expect("snapshot should be writable");

            if let Some(detail) = &completion.detail {
                writeln!(rendered, "  detail: {detail}").expect("snapshot should be writable");
            }
            if let Some(filter_text) = &completion.filter_text {
                writeln!(rendered, "  filter: {filter_text}").expect("snapshot should be writable");
            }

            if let Some(edit) = &completion.text_edit {
                self.render_completion_edit(rendered, path, edit);
            }
            if let Some(edits) = &completion.additional_text_edits {
                for edit in edits {
                    writeln!(
                        rendered,
                        "  additional: {}:{} -> {}",
                        self.render_path(path),
                        Self::render_range(edit.range),
                        Self::render_text(&edit.new_text),
                    )
                    .expect("snapshot should be writable");
                }
            }
        }
    }

    /// Every protocol edit must describe a real range in the captured document.
    ///
    /// Completion fixtures often exercise text whose line layout differs from the saved file. A
    /// snapshot makes the chosen range readable; this check also proves that both UTF-16 endpoints
    /// can be converted against the exact text sent to the engine.
    fn assert_completion_edits_fit_source(source: &str, completions: &[CompletionItem]) {
        let line_index = LineIndex::new(source);
        let assert_range = |range: Range, label: &str| {
            let start = line_index
                .offset_from_utf16_position(crate::proto::position::parse_position(range.start))
                .unwrap_or_else(|| panic!("completion {label:?} has an invalid edit start"));
            let end = line_index
                .offset_from_utf16_position(crate::proto::position::parse_position(range.end))
                .unwrap_or_else(|| panic!("completion {label:?} has an invalid edit end"));
            assert!(
                start <= end,
                "completion {label:?} has a backwards edit range",
            );
        };

        for completion in completions {
            match completion.text_edit.as_ref() {
                Some(CompletionTextEdit::Edit(edit)) => {
                    assert_range(edit.range, &completion.label);
                }
                Some(CompletionTextEdit::InsertAndReplace(edit)) => {
                    assert_range(edit.insert, &completion.label);
                    assert_range(edit.replace, &completion.label);
                }
                None => {}
            }
            for edit in completion.additional_text_edits.iter().flatten() {
                assert_range(edit.range, &completion.label);
            }
        }
    }

    fn render_completion_edit(
        &self,
        rendered: &mut String,
        path: &Path,
        edit: &CompletionTextEdit,
    ) {
        match edit {
            CompletionTextEdit::Edit(edit) => {
                writeln!(
                    rendered,
                    "  edit: {}:{} -> {}",
                    self.render_path(path),
                    Self::render_range(edit.range),
                    Self::render_text(&edit.new_text),
                )
                .expect("snapshot should be writable");
            }
            CompletionTextEdit::InsertAndReplace(edit) => {
                writeln!(
                    rendered,
                    "  insert: {}:{} -> {}",
                    self.render_path(path),
                    Self::render_range(edit.insert),
                    Self::render_text(&edit.new_text),
                )
                .expect("snapshot should be writable");
                writeln!(
                    rendered,
                    "  replace: {}:{} -> {}",
                    self.render_path(path),
                    Self::render_range(edit.replace),
                    Self::render_text(&edit.new_text),
                )
                .expect("snapshot should be writable");
            }
        }
    }

    fn render_document_symbols(
        &self,
        rendered: &mut String,
        symbols: &[DocumentSymbol],
        depth: usize,
    ) {
        if symbols.is_empty() && depth == 0 {
            writeln!(rendered, "- none").expect("snapshot should be writable");
            return;
        }

        let indent = "  ".repeat(depth);
        for symbol in symbols {
            writeln!(
                rendered,
                "{indent}- {:?} {} {}",
                symbol.kind,
                symbol.name,
                Self::render_range(symbol.selection_range),
            )
            .expect("snapshot should be writable");

            if let Some(children) = &symbol.children {
                self.render_document_symbols(rendered, children, depth + 1);
            }
        }
    }

    fn render_location(&self, location: &Location) -> String {
        format!(
            "{}:{}",
            self.render_uri_path(&location.uri),
            Self::render_range(location.range)
        )
    }

    fn render_workspace_edit(&self, rendered: &mut String, edit: &WorkspaceEdit) {
        let Some(changes) = &edit.changes else {
            writeln!(rendered, "- no changes").expect("snapshot should be writable");
            return;
        };

        let mut changes = changes.iter().collect::<Vec<_>>();
        changes.sort_by(|(left, _), (right, _)| left.as_str().cmp(right.as_str()));

        for (uri, edits) in changes {
            writeln!(rendered, "- {}", self.render_uri_path(uri))
                .expect("snapshot should be writable");
            for edit in edits {
                writeln!(
                    rendered,
                    "  - {} -> {}",
                    Self::render_range(edit.range),
                    Self::render_text(&edit.new_text),
                )
                .expect("snapshot should be writable");
            }
        }
    }

    fn render_formatting_edits(
        &self,
        rendered: &mut String,
        path: &str,
        edits: Option<&[TextEdit]>,
    ) {
        let Some(edits) = edits else {
            writeln!(rendered, "- no response").expect("snapshot should be writable");
            return;
        };
        if edits.is_empty() {
            writeln!(rendered, "- no edits").expect("snapshot should be writable");
            return;
        }

        for edit in edits {
            writeln!(
                rendered,
                "- {}:{} -> {}",
                self.render_path(self.fixture.path(path).as_path()),
                Self::render_range(edit.range),
                Self::render_text(&edit.new_text),
            )
            .expect("snapshot should be writable");
        }
    }

    fn render_uri_path(&self, uri: &ls_types::Uri) -> String {
        uri.to_file_path()
            .map(|path| self.render_path(path.as_ref()))
            .unwrap_or_else(|| uri.as_str().to_string())
    }

    fn render_path(&self, path: &Path) -> String {
        let root = rg_std::path::canonicalize(self.fixture.path(""))
            .expect("fixture root should canonicalize");
        let path = rg_std::path::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());

        if let Ok(relative) = path.strip_prefix(root) {
            // Render with forward slashes so expectations stay portable across host platforms.
            let relative = relative.to_string_lossy().replace('\\', "/");
            return format!("/{relative}");
        }

        path.display().to_string()
    }

    fn render_range(range: Range) -> String {
        format!(
            "{}:{}-{}:{}",
            range.start.line, range.start.character, range.end.line, range.end.character
        )
    }

    fn render_text(text: &str) -> String {
        if text.is_empty() || text.contains('\n') {
            format!("{text:?}")
        } else {
            text.to_string()
        }
    }

    fn write_indented(rendered: &mut String, text: &str, indent: &str) {
        for line in text.lines() {
            if line.is_empty() {
                rendered.push('\n');
            } else {
                writeln!(rendered, "{indent}{line}").expect("snapshot should be writable");
            }
        }
    }
}

#[derive(Clone, Debug)]
struct TestDocument {
    source_path: PathBuf,
    version: Option<i32>,
    text: String,
    revision: u64,
}

pub(super) struct DirtyDocument {
    path: &'static str,
    text: MarkedText,
}

#[derive(Clone, Copy)]
enum QueryMarkers<'a> {
    Saved,
    Dirty(&'a DirtyDocument),
}

pub(super) enum LspQuery {
    GotoDefinition {
        title: &'static str,
        marker: &'static str,
    },
    GotoTypeDefinition {
        title: &'static str,
        marker: &'static str,
    },
    GotoImplementation {
        title: &'static str,
        marker: &'static str,
    },
    References {
        title: &'static str,
        marker: &'static str,
        include_declaration: bool,
    },
    Hover {
        title: &'static str,
        marker: &'static str,
    },
    DocumentHighlight {
        title: &'static str,
        marker: &'static str,
    },
    Completion {
        title: &'static str,
        marker: &'static str,
    },
    DocumentSymbol {
        title: &'static str,
        path: &'static str,
    },
    InlayHint {
        title: &'static str,
        path: &'static str,
        start: &'static str,
        end: &'static str,
    },
}

impl LspQuery {
    pub(super) fn goto_definition(title: &'static str, marker: &'static str) -> Self {
        Self::GotoDefinition { title, marker }
    }

    pub(super) fn goto_type_definition(title: &'static str, marker: &'static str) -> Self {
        Self::GotoTypeDefinition { title, marker }
    }

    pub(super) fn goto_implementation(title: &'static str, marker: &'static str) -> Self {
        Self::GotoImplementation { title, marker }
    }

    pub(super) fn references(
        title: &'static str,
        marker: &'static str,
        include_declaration: bool,
    ) -> Self {
        Self::References {
            title,
            marker,
            include_declaration,
        }
    }

    pub(super) fn hover(title: &'static str, marker: &'static str) -> Self {
        Self::Hover { title, marker }
    }

    pub(super) fn document_highlight(title: &'static str, marker: &'static str) -> Self {
        Self::DocumentHighlight { title, marker }
    }

    pub(super) fn completion(title: &'static str, marker: &'static str) -> Self {
        Self::Completion { title, marker }
    }

    pub(super) fn document_symbol(title: &'static str, path: &'static str) -> Self {
        Self::DocumentSymbol { title, path }
    }

    pub(super) fn inlay_hint(
        title: &'static str,
        path: &'static str,
        start: &'static str,
        end: &'static str,
    ) -> Self {
        Self::InlayHint {
            title,
            path,
            start,
            end,
        }
    }
}

#[derive(Clone, Debug, Default)]
struct RecordingNotifications {
    notifications: Arc<Mutex<Vec<ServiceNotification>>>,
    changed: Arc<tokio::sync::Notify>,
}

impl ServiceNotificationPublisher for RecordingNotifications {
    fn send(&self, notification: ServiceNotification) {
        self.notifications
            .lock()
            .expect("recorded notifications should not be poisoned")
            .push(notification);
        self.changed.notify_one();
    }
}

impl RecordingNotifications {
    fn clear(&self) {
        self.notifications
            .lock()
            .expect("recorded notifications should not be poisoned")
            .clear();
    }

    fn snapshot(&self) -> Vec<ServiceNotification> {
        self.notifications
            .lock()
            .expect("recorded notifications should not be poisoned")
            .clone()
    }

    async fn wait_for_deferred_indexing(&self) {
        loop {
            let changed = self.changed.notified();
            if self.snapshot().iter().any(|notification| {
                matches!(
                    notification,
                    ServiceNotification::DeferredIndexingFinished { .. }
                )
            }) {
                return;
            }
            changed.await;
        }
    }
}
