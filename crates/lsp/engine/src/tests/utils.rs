use std::{
    cmp::Reverse,
    collections::HashMap,
    ffi::OsStr,
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use expect_test::Expect;
use gen_lsp_types::{
    CodeAction, CodeActionContext, CodeActionKind, CodeActionTriggerKind, CompletionItem,
    CompletionItemTextEdit, Contents, DocumentChange, DocumentHighlight, DocumentHighlightKind,
    DocumentSymbol, Edit, FoldingRange, FoldingRangeKind, Hover, InlayHint, InlayHintKind, Label,
    Location, Position, Range, SignatureHelp, TextEdit, Tooltip, Uri, WorkspaceEdit,
};
use rg_lsp_proto::{
    AnalysisConfig, CapturedSourceInput, CodeActionRequestContext, CompletionClientCapabilities,
    CompletionImport, CompletionImportCandidate, CompletionImportSite, DocumentPositionSnapshot,
    DocumentRevision, DocumentSnapshot, EditorDocumentSnapshot, EditorSnapshot, EngineConfig,
    EngineResult, EngineService, FoldingClientCapabilities, OpenDocumentSession,
    OpenDocumentsRevision, QueryError, QueryScope, QueryValue, SaveProposal, SavedProjectChanges,
    ServiceNotification, SignatureHelpClientCapabilities, SysrootDiscovery, file_uri_to_path,
    path_to_file_uri, semantic_tokens_legend,
};
use rg_parse::LineIndex;
use rg_workspace::SysrootSources;
use tarpc::context;
use test_fixture::{
    CrateFixture, FixtureMarkers, fixture_path_for_snapshot,
    testonly::{
        MarkedText,
        inline::{InlineArgument, InlineCall, InlineFixture, InlineSource},
        render_responses,
    },
};

use crate::{
    MemoryControl, Service, ServiceNotificationsSink, engine::ProjectConfiguration,
    proto::position, service::ServiceNotificationPublisher,
};

pub(super) struct SelectedAutoImport {
    pub(super) item: CompletionItem,
    import: CompletionImport,
    site: CompletionImportSite,
}

pub(super) struct LspEngineFixture {
    fixture: CrateFixture,
    markers: FixtureMarkers,
    queries: Vec<LspQuery>,
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

    pub(super) async fn initialized_with_fake_sysroot(source: &str) -> Self {
        let mut fixture = Self::new(source);
        fixture.fixture = fixture.fixture.with_fake_sysroot();
        let sysroot = SysrootSources::from_library_root(fixture.fixture.path("sysroot/library"))
            .expect("fake sysroot should be complete");
        let config = Self::engine_config();
        let mut configuration = ProjectConfiguration::from(config.analysis.clone());
        configuration.sysroot_sources = Some(sysroot);
        fixture
            .service
            .clone()
            .initialize_project(fixture.fixture.path(""), config, configuration)
            .await
            .expect("fixture LSP engine should initialize");
        fixture
    }

    pub(super) async fn initialized_with_cfg_test(fixture: &str, enabled: bool) -> Self {
        let mut config = Self::engine_config();
        config.analysis.cfg.test = enabled;
        Self::initialized_with_engine_config(fixture, config).await
    }

    fn new(fixture: &str) -> Self {
        let parsed = InlineFixture::parse(fixture, LspQuery::INLINE_NAMES);
        let queries = parsed
            .queries
            .iter()
            .map(|query| LspQuery::inline(&query.call, &query.marker))
            .collect();
        let fixture = CrateFixture::from_files(parsed.files);
        let markers = parsed.markers;
        let notifications = RecordingNotifications::default();
        let service = Service::spawn(
            Arc::new(()) as Arc<dyn MemoryControl>,
            ServiceNotificationsSink::from_publisher(notifications.clone()),
        );

        Self {
            fixture,
            markers,
            queries,
            service,
            notifications,
            documents: Mutex::default(),
            next_revision: AtomicU64::new(1),
        }
    }

    pub(super) async fn initialized_with_engine_config(
        fixture: &str,
        config: EngineConfig,
    ) -> Self {
        let fixture = Self::new(fixture);
        fixture
            .service
            .clone()
            .initialize(context::current(), fixture.fixture.path(""), config)
            .await
            .expect("fixture LSP engine should initialize");
        fixture
    }

    pub(super) fn engine_config() -> EngineConfig {
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

    pub(super) async fn check(&self, expect: Expect) {
        assert!(
            !self.queries.is_empty(),
            "fixture should contain inline LSP queries"
        );
        self.check_queries(&self.queries, expect).await;
    }

    pub(super) async fn check_dirty(&self, dirty: &DirtyDocument, expect: Expect) {
        assert!(
            !dirty.queries.is_empty(),
            "dirty document should contain inline LSP queries"
        );
        self.check_dirty_queries(dirty, &dirty.queries, expect)
            .await;
    }

    pub(super) async fn check_queries(&self, queries: &[LspQuery], expect: Expect) {
        self.check_with_markers(QueryMarkers::Saved, queries, expect)
            .await;
    }

    pub(super) async fn check_dirty_queries(
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
        let mut responses = Vec::new();
        for query in queries {
            let mut response = String::new();
            self.render_query(&mut response, markers, query).await;
            responses.push((query.title(), response));
        }
        expect.assert_eq(&render_responses(responses));
    }

    pub(super) async fn did_open_saved(&self, path: &str, version: i32) {
        let text = fs::read_to_string(self.fixture.path(path))
            .expect("fixture saved document should be readable");
        self.record_document(self.fixture.path(path), Some(version), text);
    }

    pub(super) async fn did_open_dirty(
        &self,
        path: &'static str,
        version: i32,
        text: &str,
    ) -> DirtyDocument {
        let parsed = InlineSource::parse(path, text, LspQuery::INLINE_NAMES);
        let queries = parsed
            .queries
            .iter()
            .map(|query| LspQuery::inline(&query.call, &query.marker))
            .collect();
        let text = parsed.text;
        self.record_document(
            self.fixture.path(path),
            Some(version),
            text.text().to_string(),
        );

        DirtyDocument {
            path,
            text,
            queries,
        }
    }

    pub(super) async fn did_change_full(
        &self,
        path: &'static str,
        version: i32,
        text: &str,
    ) -> DirtyDocument {
        self.did_open_dirty(path, version, text).await
    }

    fn record_document(&self, path: PathBuf, version: Option<i32>, text: String) {
        let revision = self.next_revision.fetch_add(1, Ordering::Relaxed);
        let source_path = path.canonicalize().unwrap_or_else(|_| path.clone());
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

    fn editor_snapshot(&self, target: Option<PathBuf>) -> EditorSnapshot {
        let mut documents = self.documents.lock().expect("fixture documents").clone();
        if let Some(path) = target {
            documents
                .entry(path.clone())
                .or_insert_with(|| TestDocument {
                    source_path: path.canonicalize().expect("fixture source path"),
                    version: None,
                    text: fs::read_to_string(&path).expect("fixture source"),
                    revision: 1,
                });
        }
        let revision = documents
            .values()
            .map(|document| document.revision)
            .max()
            .unwrap_or(1);
        EditorSnapshot::new(
            OpenDocumentsRevision::new(revision),
            documents
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
                .collect(),
        )
    }

    fn document_snapshot(&self, path: PathBuf) -> DocumentSnapshot {
        let editor = self.editor_snapshot(Some(path.clone()));
        let target = editor
            .documents()
            .iter()
            .find(|document| document.path() == path)
            .expect("target document")
            .target()
            .clone();
        editor
            .select(&target)
            .expect("fixture capture contains its target")
    }

    pub(super) async fn check_workspace_symbols(&self, query: &str, expected: &[&str]) {
        let editor = self.editor_snapshot(None);
        let revision = editor.revision();
        let response = self
            .service
            .clone()
            .workspace_symbol(context::current(), editor, query.to_owned())
            .await
            .expect("editor workspace search");
        assert_eq!(
            response.scope(),
            &QueryScope::Editor {
                target: None,
                open_documents_revision: revision
            }
        );
        let names = response
            .value()
            .iter()
            .map(|symbol| symbol.base_symbol_information.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, expected);
    }

    fn position_snapshot(&self, path: PathBuf, position: Position) -> DocumentPositionSnapshot {
        self.document_snapshot(path).with_position(position)
    }

    pub(super) async fn signature_help(
        &self,
        document: &DirtyDocument,
        marker: &str,
        client_capabilities: SignatureHelpClientCapabilities,
    ) -> Option<SignatureHelp> {
        let position = self.marker_position(QueryMarkers::Dirty(document), marker);
        self.service
            .clone()
            .signature_help(
                context::current(),
                self.position_snapshot(self.fixture.path(document.path), position),
                client_capabilities,
            )
            .await
            .expect("signature help query should succeed")
            .into_value()
    }

    pub(super) async fn did_save_dirty(&self, dirty: &DirtyDocument) {
        let path = self.fixture.path(dirty.path);
        fs::write(&path, dirty.text.text())
            .expect("fixture dirty document should be writable before save");
        self.did_save_current(dirty.path)
            .await
            .expect("fixture dirty document should save");
    }

    pub(super) async fn did_save_current(&self, path: &str) -> EngineResult<u64> {
        self.notifications.clear();
        let document = self.document_snapshot(self.fixture.path(path));
        let proposal = SaveProposal::new(
            document.document().target().clone(),
            document.document().client_version(),
            document.document().text().to_string(),
        );

        self.service
            .clone()
            .did_save(context::current(), proposal)
            .await
    }

    pub(super) async fn external_file_changed(&self, path: &str, text: &str) {
        self.notifications.clear();
        fs::write(self.fixture.path(path), text)
            .expect("fixture external change should be writable");
        let path = self.fixture.path(path);
        let changes = if path.extension().and_then(OsStr::to_str) == Some("rs") {
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
        fs::write(self.fixture.path(path), text)
            .expect("fixture source should be writable without a notification");
    }

    pub(super) fn remove_file_without_notification(&self, path: &str) {
        fs::remove_file(self.fixture.path(path))
            .expect("fixture source should be removable without a notification");
    }

    pub(super) fn check_notification_effects(&self, expect: Expect) {
        let mut rendered = String::new();

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
                        "publish diagnostics {} saved-text-len {:?} count {}",
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
                        "begin progress {token:?}: {title}{}",
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
                        "end progress {token:?}{}",
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
                        writeln!(rendered, "inlay hint refresh")
                            .expect("snapshot should be writable");
                        rendered_inlay_hint_refresh = true;
                    }
                }
                ServiceNotification::LogMessage { level, message } => {
                    rendered_any_notification = true;
                    writeln!(rendered, "log {level:?}: {message}")
                        .expect("snapshot should be writable");
                }
            }
        }
        if !rendered_any_notification {
            writeln!(rendered, "none").expect("snapshot should be writable");
        }

        expect.assert_eq(&render_responses([("notifications", rendered)]));
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

    pub(super) async fn check_dirty_global_operations(
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
                self.position_snapshot(path.clone(), position),
                true,
            )
            .await;
        let implementation = self
            .service
            .clone()
            .goto_implementation(
                context::current(),
                self.position_snapshot(path.clone(), position),
            )
            .await;
        let prepare_rename = self
            .service
            .clone()
            .prepare_rename(
                context::current(),
                self.position_snapshot(path.clone(), position),
            )
            .await;
        let rename = self
            .service
            .clone()
            .rename(
                context::current(),
                self.position_snapshot(path.clone(), position),
                new_name.to_string(),
                false,
            )
            .await;

        assert!(
            !references
                .expect("dirty references")
                .into_value()
                .is_empty()
        );
        implementation.expect("dirty implementation query");
        assert!(
            prepare_rename
                .expect("dirty prepare rename")
                .into_value()
                .is_some()
        );
        Self::expect_save_required(rename, "rename without versioned edits");
        let edit = self
            .service
            .clone()
            .rename(
                context::current(),
                self.position_snapshot(path, position),
                new_name.to_string(),
                true,
            )
            .await
            .expect("versioned dirty rename")
            .into_value()
            .expect("rename edit");
        let mut rendered = String::new();
        self.render_workspace_edit(&mut rendered, &edit);
        assert_eq!(
            rendered
                .lines()
                .filter(|line| line.contains(" -> "))
                .count(),
            4
        );
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
        let input = self.position_snapshot(path, position);
        let outcome = self
            .service
            .clone()
            .rename(context::current(), input, new_name.to_string(), true)
            .await
            .expect("rename query should succeed");
        let edit = outcome.into_value();

        let mut rendered = String::new();
        match edit {
            Some(edit) => self.render_workspace_edit(&mut rendered, &edit),
            None => writeln!(rendered, "none").expect("snapshot should be writable"),
        }
        expect.assert_eq(&render_responses([(title, rendered)]));
    }

    pub(super) async fn check_auto_import(
        &self,
        marker: &str,
        label: &str,
        expected_edit: &str,
    ) -> SelectedAutoImport {
        let markers = QueryMarkers::Saved;
        let input = self.position_snapshot(
            self.marker_path(markers, marker),
            self.marker_position(markers, marker),
        );
        let list = self
            .service
            .clone()
            .completion(
                context::current(),
                input.clone(),
                CompletionClientCapabilities {
                    resolve_additional_text_edits: true,
                    snippet_support: true,
                },
            )
            .await
            .expect("completion list")
            .into_value();
        assert!(list.is_incomplete);
        let candidate = list
            .items
            .into_iter()
            .find(|candidate| candidate.item.label == label)
            .unwrap_or_else(|| panic!("missing auto-import completion {label}"));
        let site = list.import_site.expect("original completion site");
        let import = candidate.import.expect("auto-import payload");
        assert!(candidate.item.additional_text_edits.is_none());

        let edit = self
            .service
            .clone()
            .resolve_completion_import(context::current(), input.clone(), import.clone(), None)
            .await
            .expect("resolve completion import")
            .into_value()
            .expect("import edit");
        Self::assert_completion_edits_fit_source(
            input.document().text(),
            &[CompletionItem {
                additional_text_edits: Some(vec![edit.clone()]),
                ..candidate.item.clone()
            }],
        );
        assert_eq!(edit.new_text.trim(), expected_edit);
        SelectedAutoImport {
            item: candidate.item,
            import,
            site,
        }
    }

    pub(super) async fn check_revalidated_auto_import(
        &self,
        path: &str,
        original: &SelectedAutoImport,
        expected_edit: Option<&str>,
    ) {
        let snapshot = self.document_snapshot(self.fixture.path(path));
        let text = snapshot.document().text();
        let offset = original
            .site
            .offset_in(text, &original.item.label)
            .expect("completion site in edited text");
        let position = position::position(
            &LineIndex::new(text),
            offset.try_into().expect("fixture offset"),
        );
        let input = snapshot.with_position(position);
        let candidate = CompletionImportCandidate {
            label: original.item.label.clone(),
            kind: original.item.kind.expect("auto-import kind"),
            inline_module_path: Some(original.site.inline_module_path.clone()),
        };
        let edit = self
            .service
            .clone()
            .resolve_completion_import(
                context::current(),
                input.clone(),
                original.import.clone(),
                Some(candidate),
            )
            .await
            .expect("revalidate completion import")
            .into_value();
        assert_eq!(
            edit.as_ref().map(|edit| edit.new_text.trim()),
            expected_edit
        );
        Self::assert_completion_edits_fit_source(
            input.document().text(),
            &[CompletionItem {
                additional_text_edits: edit.map(|edit| vec![edit]),
                ..Default::default()
            }],
        );
    }

    pub(super) fn accept_completion(&self, path: &str, version: i32, item: &CompletionItem) {
        let path = self.fixture.path(path);
        let snapshot = self.document_snapshot(path.clone());
        let mut text = snapshot.document().text().to_owned();
        let Some(CompletionItemTextEdit::TextEdit(edit)) = &item.text_edit else {
            panic!("completion should have a primary edit");
        };
        let line_index = LineIndex::new(&text);
        let offset = |position| {
            line_index
                .offset_from_utf16_position(position::parse_position(position))
                .expect("completion position in fixture") as usize
        };
        // These fixtures use only a final cursor stop, so the editor inserts the rest verbatim.
        let new_text = edit.new_text.trim_end_matches("$0");
        assert!(
            !new_text.contains('$'),
            "fixture uses an unsupported snippet"
        );
        text.replace_range(offset(edit.range.start)..offset(edit.range.end), new_text);
        self.record_document(path, Some(version), text);
    }

    pub(super) async fn check_dirty_completion_error(
        &self,
        document: &DirtyDocument,
        expected: QueryError,
    ) {
        let input = self
            .document_snapshot(self.fixture.path(document.path))
            .with_position(Position::new(0, 0));
        let outcome = self
            .service
            .clone()
            .completion(
                context::current(),
                input,
                CompletionClientCapabilities::default(),
            )
            .await;
        assert_eq!(outcome, Err(expected));
    }

    pub(super) async fn check_rename_error(
        &self,
        marker: &'static str,
        new_name: &'static str,
        expected: QueryError,
    ) {
        let path = self.marker_path(QueryMarkers::Saved, marker);
        let position = self.marker_position(QueryMarkers::Saved, marker);
        let input = self.position_snapshot(path, position);
        let outcome = self
            .service
            .clone()
            .rename(context::current(), input, new_name.to_string(), true)
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
                self.document_snapshot(self.fixture.path(path))
                    .document()
                    .clone(),
            )
            .await
            .expect("formatting query should succeed");
        let edits = outcome.into_value();

        let mut rendered = String::new();
        self.render_formatting_edits(&mut rendered, path, edits.as_deref());
        expect.assert_eq(&render_responses([(title, rendered)]));
    }

    pub(super) async fn check_folding(
        &self,
        title: &'static str,
        path: &'static str,
        line_folding_only: bool,
        expect: Expect,
    ) {
        let outcome = self
            .service
            .clone()
            .folding_range(
                context::current(),
                self.document_snapshot(self.fixture.path(path))
                    .document()
                    .clone(),
                FoldingClientCapabilities { line_folding_only },
            )
            .await
            .expect("folding range query should succeed");
        let ranges = outcome.into_value();

        let mut rendered = String::new();
        Self::render_folding_ranges(&mut rendered, &ranges);
        expect.assert_eq(&render_responses([(title, rendered)]));
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
            LspQuery::GotoDefinition { marker, .. } => {
                let path = self.marker_path(markers, marker);
                let position = self.marker_position(markers, marker);
                let input = self.position_snapshot(path, position);
                let outcome = self
                    .service
                    .clone()
                    .goto_definition(context::current(), input)
                    .await
                    .expect("goto definition query should succeed");
                let locations = outcome.into_value();

                self.render_locations(rendered, &locations);
            }
            LspQuery::GotoTypeDefinition { marker, .. } => {
                let path = self.marker_path(markers, marker);
                let position = self.marker_position(markers, marker);
                let input = self.position_snapshot(path, position);
                let outcome = self
                    .service
                    .clone()
                    .goto_type_definition(context::current(), input)
                    .await
                    .expect("goto type definition query should succeed");
                let locations = outcome.into_value();

                self.render_locations(rendered, &locations);
            }
            LspQuery::GotoImplementation { marker, .. } => {
                let path = self.marker_path(markers, marker);
                let position = self.marker_position(markers, marker);
                let input = self.position_snapshot(path, position);
                let outcome = self
                    .service
                    .clone()
                    .goto_implementation(context::current(), input)
                    .await
                    .expect("goto implementation query should succeed");
                let locations = outcome.into_value();

                self.render_locations(rendered, &locations);
            }
            LspQuery::References {
                marker,
                include_declaration,
                ..
            } => {
                let path = self.marker_path(markers, marker);
                let position = self.marker_position(markers, marker);
                let input = self.position_snapshot(path, position);
                let outcome = self
                    .service
                    .clone()
                    .references(context::current(), input, *include_declaration)
                    .await
                    .expect("references query should succeed");
                let mut locations = outcome.into_value();

                // Parallel source discovery can assign different file IDs to the same locations.
                locations.sort_unstable_by(|left, right| {
                    (left.uri.as_str(), left.range).cmp(&(right.uri.as_str(), right.range))
                });
                self.render_locations(rendered, &locations);
            }
            LspQuery::Hover { marker, .. } => {
                let path = self.marker_path(markers, marker);
                let position = self.marker_position(markers, marker);
                let input = self.position_snapshot(path.clone(), position);
                let outcome = self
                    .service
                    .clone()
                    .hover(context::current(), input)
                    .await
                    .expect("hover query should succeed");
                let hover = outcome.into_value();

                self.render_hover(rendered, path.as_path(), hover.as_ref());
            }
            LspQuery::DocumentHighlight { marker, .. } => {
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

                Self::render_document_highlights(rendered, &highlights);
            }
            LspQuery::Completion { marker, .. } => {
                let path = self.marker_path(markers, marker);
                let position = self.marker_position(markers, marker);
                let document = self.document_snapshot(path.clone());
                let current_text = document.document().text().to_string();
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
                let completions = outcome
                    .into_value()
                    .items
                    .into_iter()
                    .map(|candidate| candidate.item)
                    .collect::<Vec<_>>();
                Self::assert_completion_edits_fit_source(&current_text, &completions);

                self.render_completions(rendered, path.as_path(), &completions);
            }
            LspQuery::CodeAction {
                marker,
                only,
                automatic,
                ..
            } => {
                let path = self.marker_path(markers, marker);
                let position = self.marker_position(markers, marker);
                let document = self.document_snapshot(path.clone());
                let current_text = document.document().text().to_string();
                let input = document.with_range(Range::new(position, position));
                let lsp_context = CodeActionContext {
                    diagnostics: Vec::new(),
                    only: only.clone().map(|kind| vec![kind]),
                    trigger_kind: Some(if *automatic {
                        CodeActionTriggerKind::Automatic
                    } else {
                        CodeActionTriggerKind::Invoked
                    }),
                };
                let outcome = self
                    .service
                    .clone()
                    .code_action(
                        context::current(),
                        input,
                        CodeActionRequestContext::from_lsp(&lsp_context),
                    )
                    .await
                    .expect("code action query should succeed");
                let actions = outcome.into_value();

                self.render_code_actions(rendered, path.as_path(), &current_text, &actions);
            }
            LspQuery::DocumentSymbol { path, .. } => {
                let document = self.document_snapshot(self.fixture.path(path));
                let outcome = self
                    .service
                    .clone()
                    .document_symbol(context::current(), document)
                    .await
                    .expect("document symbol query should succeed");
                let symbols = outcome.into_value();

                self.render_document_symbols(rendered, &symbols, 0);
            }
            LspQuery::SemanticTokens { path, range, .. } => {
                let document = self.document_snapshot(self.fixture.path(path));
                let range = range.map(|(start, end)| {
                    Range::new(
                        self.marker_position(markers, start),
                        self.marker_position(markers, end),
                    )
                });
                let outcome = self
                    .service
                    .clone()
                    .semantic_tokens(context::current(), document.clone(), range)
                    .await
                    .expect("semantic tokens should succeed");
                let tokens = outcome.into_value();
                let legend = semantic_tokens_legend();
                let index = LineIndex::new(document.document().text());
                let mut position = Position::default();
                let mut previous_end = Position::default();
                for token in tokens.data {
                    position = Position::new(
                        position.line + token.delta_line,
                        if token.delta_line == 0 {
                            position.character + token.delta_start
                        } else {
                            token.delta_start
                        },
                    );
                    let end = Position::new(position.line, position.character + token.length);
                    assert!(
                        token.length > 0 && position >= previous_end,
                        "tokens are nonempty and nonoverlapping"
                    );
                    assert!(range.is_none_or(|range| range.start <= position && end <= range.end));
                    previous_end = end;
                    let start_byte = index
                        .offset_from_utf16_position(position::parse_position(position))
                        .expect("token start is a UTF-16 boundary")
                        as usize;
                    let end_byte = index
                        .offset_from_utf16_position(position::parse_position(end))
                        .expect("token end is a UTF-16 boundary")
                        as usize;
                    let kind = &legend.token_types[token.token_type as usize];
                    let modifiers = legend
                        .token_modifiers
                        .iter()
                        .enumerate()
                        .filter(|(index, _)| token.token_modifiers_bitset & (1 << index) != 0)
                        .map(|(_, modifier)| format!(".{}", modifier.as_str()))
                        .collect::<String>();
                    writeln!(
                        rendered,
                        "{}:{}-{}:{} {}{} {:?}",
                        position.line,
                        position.character,
                        end.line,
                        end.character,
                        kind.as_str(),
                        modifiers,
                        &document.document().text()[start_byte..end_byte]
                    )
                    .expect("snapshot should be writable");
                }
            }
            LspQuery::InlayHint {
                path, start, end, ..
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

                Self::render_inlay_hints(rendered, &hints);
            }
        }
    }

    fn marker_path(&self, markers: QueryMarkers<'_>, marker: &str) -> PathBuf {
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
                let text = fs::read_to_string(self.fixture.path(&marker.path))
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
            writeln!(rendered, "none").expect("snapshot should be writable");
            return;
        }

        let mut locations = locations
            .iter()
            .map(|location| self.render_location(location))
            .collect::<Vec<_>>();
        locations.sort();
        for location in locations {
            writeln!(rendered, "{location}").expect("snapshot should be writable");
        }
    }

    fn render_hover(&self, rendered: &mut String, path: &Path, hover: Option<&Hover>) {
        let Some(hover) = hover else {
            writeln!(rendered, "none").expect("snapshot should be writable");
            return;
        };

        if let Some(range) = hover.range {
            writeln!(
                rendered,
                "range: {}:{}",
                self.render_path(path),
                Self::render_range(range),
            )
            .expect("snapshot should be writable");
        }

        writeln!(rendered, "markdown:").expect("snapshot should be writable");
        match &hover.contents {
            Contents::MarkupContent(markup) => {
                // Open-document paths and Cargo's saved paths can differ through symlinks.
                let root = self.fixture.path("");
                let canonical_root = root.canonicalize().expect("fixture root should exist");
                let mut markdown = markup.value.clone();
                for root in [root, canonical_root] {
                    let uri = path_to_file_uri(&root).expect("fixture root should have a file URI");
                    markdown = markdown.replace(uri.as_str().trim_end_matches('/'), "file://$ROOT");
                }
                Self::write_indented(rendered, &markdown, "  ");
            }
            Contents::MarkedString(marked) => {
                Self::write_indented(rendered, &format!("{marked:?}"), "  ")
            }
            Contents::MarkedStringList(marked) => {
                for value in marked {
                    Self::write_indented(rendered, &format!("{value:?}"), "  ");
                }
            }
        }
    }

    fn render_document_highlights(rendered: &mut String, highlights: &[DocumentHighlight]) {
        if highlights.is_empty() {
            writeln!(rendered, "none").expect("snapshot should be writable");
            return;
        }

        for highlight in highlights {
            let kind = match highlight.kind {
                Some(DocumentHighlightKind::Read) => "read",
                Some(DocumentHighlightKind::Write) => "write",
                Some(DocumentHighlightKind::Text) | None => "text",
                Some(_) => "unknown",
            };
            writeln!(rendered, "{kind} {}", Self::render_range(highlight.range))
                .expect("snapshot should be writable");
        }
    }

    fn render_inlay_hints(rendered: &mut String, hints: &[InlayHint]) {
        if hints.is_empty() {
            writeln!(rendered, "none").expect("snapshot should be writable");
            return;
        }

        for hint in hints {
            let label = match &hint.label {
                Label::String(label) => label.clone(),
                Label::InlayHintLabelPartList(parts) => {
                    parts.iter().map(|part| part.value.as_str()).collect()
                }
            };
            let kind = match hint.kind {
                Some(InlayHintKind::Type) => "type",
                Some(InlayHintKind::Parameter) => "parameter",
                Some(_) | None => "text",
            };
            writeln!(
                rendered,
                "`{label}` {kind} @ {}:{}",
                hint.position.line, hint.position.character
            )
            .expect("snapshot should be writable");
            if let Some(tooltip) = &hint.tooltip {
                let text = match tooltip {
                    Tooltip::String(text) => text,
                    Tooltip::MarkupContent(markup) => &markup.value,
                };
                writeln!(rendered, "  tooltip:").expect("snapshot should be writable");
                for line in text.lines() {
                    writeln!(rendered, "    {line}").expect("snapshot should be writable");
                }
            }
        }
    }

    fn render_completions(
        &self,
        rendered: &mut String,
        path: &Path,
        completions: &[CompletionItem],
    ) {
        if completions.is_empty() {
            writeln!(rendered, "none").expect("snapshot should be writable");
            return;
        }

        for completion in completions {
            let kind = completion
                .kind
                .map(|kind| format!("{kind:?}"))
                .unwrap_or_else(|| "Unknown".to_string());
            writeln!(rendered, "{} {kind}", completion.label).expect("snapshot should be writable");

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

    fn render_code_actions(
        &self,
        rendered: &mut String,
        path: &Path,
        source: &str,
        actions: &[CodeAction],
    ) {
        if actions.is_empty() {
            writeln!(rendered, "none").expect("snapshot should be writable");
            return;
        }

        let line_index = LineIndex::new(source);
        for action in actions {
            let kind = action
                .kind
                .as_ref()
                .map_or("<none>", CodeActionKind::as_str);
            writeln!(rendered, "{kind} {}", action.title).expect("snapshot should be writable");
            writeln!(
                rendered,
                "  preferred: {}",
                action
                    .is_preferred
                    .map_or("unset".to_string(), |preferred| preferred.to_string())
            )
            .expect("snapshot should be writable");

            let Some(edit) = action.edit.as_ref() else {
                writeln!(rendered, "  edit: none").expect("snapshot should be writable");
                continue;
            };
            let Some(documents) = edit.document_changes.as_ref() else {
                panic!("code action should contain only versioned document edits");
            };
            assert_eq!(documents.len(), 1, "code action should edit one document");
            let DocumentChange::TextDocumentEdit(document) = &documents[0] else {
                panic!("code action should contain a versioned document edit");
            };
            assert_eq!(
                document
                    .text_document
                    .text_document_identifier
                    .uri
                    .to_file_path()
                    .as_deref(),
                Ok(path)
            );
            writeln!(
                rendered,
                "  document: {} version {}",
                self.render_uri_path(&document.text_document.text_document_identifier.uri),
                document
                    .text_document
                    .version
                    .map_or("none".to_string(), |version| version.to_string())
            )
            .expect("snapshot should be writable");

            let mut source_edits = Vec::new();
            for edit in &document.edits {
                let edit = match edit {
                    Edit::TextEdit(edit) => edit,
                    Edit::AnnotatedTextEdit(edit) => &edit.text_edit,
                    Edit::SnippetTextEdit(_) => {
                        panic!("code actions should contain plain text edits")
                    }
                };
                writeln!(
                    rendered,
                    "  edit: {} -> {}",
                    Self::render_range(edit.range),
                    Self::render_text(&edit.new_text),
                )
                .expect("snapshot should be writable");
                let start = line_index
                    .offset_from_utf16_position(position::parse_position(edit.range.start))
                    .expect("code action edit start should fit captured source");
                let end = line_index
                    .offset_from_utf16_position(position::parse_position(edit.range.end))
                    .expect("code action edit end should fit captured source");
                assert!(start <= end, "code action edit should not be backwards");
                source_edits.push((start, end, edit.new_text.as_str()));
            }

            source_edits.sort_by_key(|(start, end, _)| Reverse((*start, *end)));
            let mut result = source.to_string();
            for (start, end, new_text) in source_edits {
                result.replace_range(
                    usize::try_from(start).expect("code action edit start should fit usize")
                        ..usize::try_from(end).expect("code action edit end should fit usize"),
                    new_text,
                );
            }
            writeln!(rendered, "  result:").expect("snapshot should be writable");
            for line in result.lines() {
                if line.is_empty() {
                    writeln!(rendered).expect("snapshot should be writable");
                } else {
                    writeln!(rendered, "    {line}").expect("snapshot should be writable");
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
                .offset_from_utf16_position(position::parse_position(range.start))
                .unwrap_or_else(|| panic!("completion {label:?} has an invalid edit start"));
            let end = line_index
                .offset_from_utf16_position(position::parse_position(range.end))
                .unwrap_or_else(|| panic!("completion {label:?} has an invalid edit end"));
            assert!(
                start <= end,
                "completion {label:?} has a backwards edit range",
            );
        };

        for completion in completions {
            match completion.text_edit.as_ref() {
                Some(CompletionItemTextEdit::TextEdit(edit)) => {
                    assert_range(edit.range, &completion.label);
                }
                Some(CompletionItemTextEdit::InsertReplaceEdit(edit)) => {
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
        edit: &CompletionItemTextEdit,
    ) {
        match edit {
            CompletionItemTextEdit::TextEdit(edit) => {
                writeln!(
                    rendered,
                    "  edit: {}:{} -> {}",
                    self.render_path(path),
                    Self::render_range(edit.range),
                    Self::render_text(&edit.new_text),
                )
                .expect("snapshot should be writable");
            }
            CompletionItemTextEdit::InsertReplaceEdit(edit) => {
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
            writeln!(rendered, "none").expect("snapshot should be writable");
            return;
        }

        let indent = "  ".repeat(depth);
        for symbol in symbols {
            writeln!(
                rendered,
                "{indent}{:?} {} {}",
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

    fn render_folding_ranges(rendered: &mut String, ranges: &[FoldingRange]) {
        if ranges.is_empty() {
            writeln!(rendered, "none").expect("snapshot should be writable");
            return;
        }

        for range in ranges {
            let kind = match range.kind {
                Some(FoldingRangeKind::Comment) => "comment",
                Some(FoldingRangeKind::Imports) => "imports",
                Some(_) => "other",
                None => "code",
            };
            let start_character = range
                .start_character
                .map(|character| character.to_string())
                .unwrap_or_else(|| "*".to_string());
            let end_character = range
                .end_character
                .map(|character| character.to_string())
                .unwrap_or_else(|| "*".to_string());
            writeln!(
                rendered,
                "{kind} {}:{start_character}-{}:{end_character}",
                range.start_line, range.end_line,
            )
            .expect("snapshot should be writable");
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
        let mut changes = if let Some(documents) = &edit.document_changes {
            documents
                .iter()
                .map(|change| {
                    let DocumentChange::TextDocumentEdit(change) = change else {
                        panic!("rename uses text edits");
                    };
                    let uri = &change.text_document.text_document_identifier.uri;
                    let path = file_uri_to_path(uri).expect("fixture edit path");
                    let documents = self.documents.lock().expect("fixture documents");
                    let expected_version =
                        documents.get(&path).and_then(|document| document.version);
                    assert_eq!(
                        change.text_document.version, expected_version,
                        "rename uses the captured version"
                    );
                    let edits = change
                        .edits
                        .iter()
                        .map(|edit| {
                            let Edit::TextEdit(edit) = edit else {
                                panic!("rename uses plain text edits");
                            };
                            edit
                        })
                        .collect::<Vec<_>>();
                    (uri, edits)
                })
                .collect::<Vec<_>>()
        } else if let Some(changes) = &edit.changes {
            changes
                .iter()
                .map(|(uri, edits)| (uri, edits.iter().collect()))
                .collect()
        } else {
            writeln!(rendered, "no changes").expect("snapshot should be writable");
            return;
        };

        changes.sort_by_key(|(uri, _)| self.render_uri_path(uri));

        for (uri, edits) in changes {
            writeln!(rendered, "{}", self.render_uri_path(uri))
                .expect("snapshot should be writable");
            for edit in edits {
                writeln!(
                    rendered,
                    "  {} -> {}",
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
            writeln!(rendered, "no response").expect("snapshot should be writable");
            return;
        };
        if edits.is_empty() {
            writeln!(rendered, "no edits").expect("snapshot should be writable");
            return;
        }

        for edit in edits {
            writeln!(
                rendered,
                "{}:{} -> {}",
                self.render_path(self.fixture.path(path).as_path()),
                Self::render_range(edit.range),
                Self::render_text(&edit.new_text),
            )
            .expect("snapshot should be writable");
        }
    }

    fn render_uri_path(&self, uri: &Uri) -> String {
        uri.to_file_path()
            .map(|path| self.render_path(path.as_ref()))
            .unwrap_or_else(|()| uri.as_str().to_string())
    }

    fn render_path(&self, path: &Path) -> String {
        let display_root = self.fixture.path("");
        let root = display_root
            .canonicalize()
            .expect("fixture root should canonicalize");
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let relative = path
            .strip_prefix(&root)
            .or_else(|_| path.strip_prefix(&display_root))
            .expect("LSP snapshot path should stay inside its fixture root");

        format!("/{}", fixture_path_for_snapshot(relative))
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
    queries: Vec<LspQuery>,
}

#[derive(Clone, Copy)]
enum QueryMarkers<'a> {
    Saved,
    Dirty(&'a DirtyDocument),
}

pub(super) enum LspQuery {
    GotoDefinition {
        title: String,
        marker: String,
    },
    GotoTypeDefinition {
        title: String,
        marker: String,
    },
    GotoImplementation {
        title: String,
        marker: String,
    },
    References {
        title: String,
        marker: String,
        include_declaration: bool,
    },
    Hover {
        title: String,
        marker: String,
    },
    DocumentHighlight {
        title: String,
        marker: String,
    },
    Completion {
        title: String,
        marker: String,
    },
    CodeAction {
        title: String,
        marker: String,
        only: Option<CodeActionKind>,
        automatic: bool,
    },
    DocumentSymbol {
        title: String,
        path: &'static str,
    },
    SemanticTokens {
        title: String,
        path: &'static str,
        range: Option<(&'static str, &'static str)>,
    },
    InlayHint {
        title: String,
        path: &'static str,
        start: &'static str,
        end: &'static str,
    },
}

impl LspQuery {
    const INLINE_NAMES: &'static [&'static str] = &[
        "goto",
        "goto_type",
        "goto_impl",
        "references",
        "hover",
        "highlight",
        "complete",
        "code_actions",
        "automatic_code_actions",
    ];

    fn title(&self) -> &str {
        match self {
            Self::GotoDefinition { title, .. }
            | Self::GotoTypeDefinition { title, .. }
            | Self::GotoImplementation { title, .. }
            | Self::References { title, .. }
            | Self::Hover { title, .. }
            | Self::DocumentHighlight { title, .. }
            | Self::Completion { title, .. }
            | Self::CodeAction { title, .. }
            | Self::DocumentSymbol { title, .. }
            | Self::SemanticTokens { title, .. }
            | Self::InlayHint { title, .. } => title,
        }
    }

    fn inline(call: &InlineCall, marker: &str) -> Self {
        use InlineArgument::{Bool, String};
        let constructor = match call.name.as_str() {
            "goto" => Self::goto_definition,
            "goto_type" => Self::goto_type_definition,
            "goto_impl" => Self::goto_implementation,
            "hover" => Self::hover,
            "highlight" => Self::document_highlight,
            "complete" => Self::completion,
            "code_actions" => Self::code_action,
            "automatic_code_actions" => Self::automatic_code_action,
            "references" => {
                let [String(title), Bool(include_declaration)] = call.arguments.as_slice() else {
                    panic!("expected references(\"title\", include_declaration)");
                };
                assert!(
                    call.modifiers.is_empty(),
                    "reference queries do not take modifiers"
                );
                return Self::references(title, marker, *include_declaration);
            }
            name => panic!("unknown inline LSP query `{name}`"),
        };
        let [String(title)] = call.arguments.as_slice() else {
            panic!("inline LSP query takes one title string");
        };
        let mut query = constructor(title, marker);
        for modifier in &call.modifiers {
            match (
                &mut query,
                modifier.name.as_str(),
                modifier.arguments.as_slice(),
            ) {
                (Self::CodeAction { only, .. }, "only", [String(kind)]) => {
                    *only = Some(CodeActionKind::from(kind.clone()));
                }
                _ => panic!("inline code actions support .only(\"kind\")"),
            }
        }
        query
    }

    pub(super) fn goto_definition(title: &str, marker: &str) -> Self {
        Self::GotoDefinition {
            title: title.to_string(),
            marker: marker.to_string(),
        }
    }

    pub(super) fn goto_type_definition(title: &str, marker: &str) -> Self {
        Self::GotoTypeDefinition {
            title: title.to_string(),
            marker: marker.to_string(),
        }
    }

    pub(super) fn goto_implementation(title: &str, marker: &str) -> Self {
        Self::GotoImplementation {
            title: title.to_string(),
            marker: marker.to_string(),
        }
    }

    pub(super) fn references(title: &str, marker: &str, include_declaration: bool) -> Self {
        Self::References {
            title: title.to_string(),
            marker: marker.to_string(),
            include_declaration,
        }
    }

    pub(super) fn hover(title: &str, marker: &str) -> Self {
        Self::Hover {
            title: title.to_string(),
            marker: marker.to_string(),
        }
    }

    pub(super) fn document_highlight(title: &str, marker: &str) -> Self {
        Self::DocumentHighlight {
            title: title.to_string(),
            marker: marker.to_string(),
        }
    }

    pub(super) fn completion(title: &str, marker: &str) -> Self {
        Self::Completion {
            title: title.to_string(),
            marker: marker.to_string(),
        }
    }

    pub(super) fn code_action(title: &str, marker: &str) -> Self {
        Self::CodeAction {
            title: title.to_string(),
            marker: marker.to_string(),
            only: None,
            automatic: false,
        }
    }

    pub(super) fn code_action_only(title: &str, marker: &str, only: CodeActionKind) -> Self {
        Self::CodeAction {
            title: title.to_string(),
            marker: marker.to_string(),
            only: Some(only),
            automatic: false,
        }
    }

    pub(super) fn automatic_code_action(title: &str, marker: &str) -> Self {
        Self::CodeAction {
            title: title.to_string(),
            marker: marker.to_string(),
            only: None,
            automatic: true,
        }
    }

    pub(super) fn document_symbol(title: &str, path: &'static str) -> Self {
        Self::DocumentSymbol {
            title: title.to_string(),
            path,
        }
    }

    pub(super) fn semantic_tokens(
        title: &str,
        path: &'static str,
        range: Option<(&'static str, &'static str)>,
    ) -> Self {
        Self::SemanticTokens {
            title: title.to_string(),
            path,
            range,
        }
    }

    pub(super) fn inlay_hint(
        title: &str,
        path: &'static str,
        start: &'static str,
        end: &'static str,
    ) -> Self {
        Self::InlayHint {
            title: title.to_string(),
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
}
