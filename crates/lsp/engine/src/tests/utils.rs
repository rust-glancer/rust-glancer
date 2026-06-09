use std::{
    fmt::Write as _,
    path::Path,
    sync::{Arc, Mutex},
};

use expect_test::Expect;
use ls_types::{
    CompletionItem, CompletionTextEdit, DocumentSymbol, Hover, HoverContents, Location, Position,
    Range,
};
use rg_lsp_proto::{
    CompletionClientCapabilities, EngineConfig, EngineService, ServiceNotification,
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
}

impl LspEngineFixture {
    pub(super) async fn initialized(fixture: &str) -> Self {
        let mut fixture = Self::new(fixture);
        fixture.initialize().await;
        fixture
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
        }
    }

    async fn initialize(&mut self) {
        self.service
            .clone()
            .initialize(
                context::current(),
                self.fixture.path(""),
                EngineConfig::default(),
            )
            .await
            .expect("fixture LSP engine should initialize");
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

        self.service
            .clone()
            .did_open(
                context::current(),
                self.fixture.path(path),
                Some(version),
                text,
            )
            .await
            .expect("fixture saved document should open");
    }

    pub(super) async fn did_change_full(
        &self,
        path: &'static str,
        version: i32,
        text: MarkedText,
    ) -> DirtyDocument {
        self.service
            .clone()
            .did_change(
                context::current(),
                self.fixture.path(path),
                Some(version),
                Some(text.text().to_string()),
                1,
            )
            .await
            .expect("fixture dirty document should change");

        DirtyDocument { path, text }
    }

    pub(super) async fn did_save_dirty(&self, dirty: &DirtyDocument) {
        self.notifications.clear();
        std::fs::write(self.fixture.path(dirty.path), dirty.text.text())
            .expect("fixture dirty document should be writable before save");

        self.service
            .clone()
            .did_save(
                context::current(),
                self.fixture.path(dirty.path),
                Some(dirty.text.text().to_string()),
            )
            .await
            .expect("fixture dirty document should save");
    }

    pub(super) fn check_notification_effects(&self, expect: Expect) {
        let mut rendered = String::new();
        writeln!(rendered, "notifications").expect("snapshot should be writable");

        let notifications = self.notifications.snapshot();
        if notifications.is_empty() {
            writeln!(rendered, "- none").expect("snapshot should be writable");
        }

        let mut rendered_inlay_hint_refresh = false;
        for notification in notifications {
            match notification {
                ServiceNotification::PublishDiagnostics {
                    path,
                    diagnostics,
                    version,
                } => {
                    writeln!(
                        rendered,
                        "- publish diagnostics {} version {:?} count {}",
                        self.render_path(path.as_path()),
                        version,
                        diagnostics.len(),
                    )
                    .expect("snapshot should be writable");
                }
                ServiceNotification::BeginWorkDoneProgress {
                    token,
                    title,
                    message,
                } => {
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
                    // `didChange` may have a pending debounced refresh while `didSave` sends an
                    // immediate refresh, so duplicate invalidations are intentionally collapsed.
                    if !rendered_inlay_hint_refresh {
                        writeln!(rendered, "- inlay hint refresh")
                            .expect("snapshot should be writable");
                        rendered_inlay_hint_refresh = true;
                    }
                }
                ServiceNotification::LogMessage { level, message } => {
                    writeln!(rendered, "- log {level:?}: {message}")
                        .expect("snapshot should be writable");
                }
            }
        }

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
                let locations = self
                    .service
                    .clone()
                    .goto_definition(context::current(), path, position)
                    .await
                    .expect("goto definition query should succeed");

                writeln!(rendered, "{title}").expect("snapshot should be writable");
                self.render_locations(rendered, &locations);
            }
            LspQuery::Hover { title, marker } => {
                let path = self.marker_path(markers, marker);
                let position = self.marker_position(markers, marker);
                let hover = self
                    .service
                    .clone()
                    .hover(context::current(), path.clone(), position)
                    .await
                    .expect("hover query should succeed");

                writeln!(rendered, "{title}").expect("snapshot should be writable");
                self.render_hover(rendered, path.as_path(), hover.as_ref());
            }
            LspQuery::Completion { title, marker } => {
                let path = self.marker_path(markers, marker);
                let position = self.marker_position(markers, marker);
                let completions = self
                    .service
                    .clone()
                    .completion(
                        context::current(),
                        path.clone(),
                        position,
                        CompletionClientCapabilities::default(),
                    )
                    .await
                    .expect("completion query should succeed");

                writeln!(rendered, "{title}").expect("snapshot should be writable");
                self.render_completions(rendered, path.as_path(), &completions);
            }
            LspQuery::DocumentSymbol { title, path } => {
                let symbols = self
                    .service
                    .clone()
                    .document_symbol(context::current(), self.fixture.path(path))
                    .await
                    .expect("document symbol query should succeed");

                writeln!(rendered, "{title}").expect("snapshot should be writable");
                self.render_document_symbols(rendered, &symbols, 0);
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

            if let Some(edit) = &completion.text_edit {
                self.render_completion_edit(rendered, path, edit);
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

    fn render_uri_path(&self, uri: &ls_types::Uri) -> String {
        uri.to_file_path()
            .map(|path| self.render_path(path.as_ref()))
            .unwrap_or_else(|| uri.as_str().to_string())
    }

    fn render_path(&self, path: &Path) -> String {
        let root = self
            .fixture
            .path("")
            .canonicalize()
            .expect("fixture root should canonicalize");
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

        if let Ok(relative) = path.strip_prefix(root) {
            return format!("/{}", relative.display());
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
        if text.contains('\n') {
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
    Hover {
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
}

impl LspQuery {
    pub(super) fn goto_definition(title: &'static str, marker: &'static str) -> Self {
        Self::GotoDefinition { title, marker }
    }

    pub(super) fn hover(title: &'static str, marker: &'static str) -> Self {
        Self::Hover { title, marker }
    }

    pub(super) fn completion(title: &'static str, marker: &'static str) -> Self {
        Self::Completion { title, marker }
    }

    pub(super) fn document_symbol(title: &'static str, path: &'static str) -> Self {
        Self::DocumentSymbol { title, path }
    }
}

#[derive(Clone, Debug, Default)]
struct RecordingNotifications {
    notifications: Arc<Mutex<Vec<ServiceNotification>>>,
}

impl ServiceNotificationPublisher for RecordingNotifications {
    fn send(&self, notification: ServiceNotification) {
        self.notifications
            .lock()
            .expect("recorded notifications should not be poisoned")
            .push(notification);
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
