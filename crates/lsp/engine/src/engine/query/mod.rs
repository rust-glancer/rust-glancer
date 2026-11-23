//! Runs semantic queries against one complete captured editor view.
//!
//! The `lifecycle` module wraps this feature work with cancellation, stale-source recovery, result
//! tagging, and cleanup of data loaded for the request.

mod lifecycle;
mod navigation;
mod references;
mod semantic_tokens;
mod source;

use std::{sync::Arc, time::Instant};

use anyhow::Context as _;
use gen_lsp_types::{
    CodeAction, DocumentSymbol, FoldingRange, Hover, InlayHint, Position, Range, SignatureHelp,
    TextEdit, WorkspaceSymbol,
};
use rg_analysis::{
    Analysis, CodeActionKinds, CodeActionQuery, CodeActionTrigger,
    CompletionClientCapabilities as AnalysisCompletionClientCapabilities, CompletionQuery,
    CompletionSource,
};
use rg_ir_model::{Path, Span};
use rg_lsp_proto::{
    CodeActionRequestContext, CodeActionRequestTrigger, CompletionClientCapabilities,
    CompletionImport, CompletionImportCandidate, CompletionImportSite, CompletionResult,
    DocumentPositionSnapshot, DocumentRangeSnapshot, DocumentSnapshot, EditorDocumentSnapshot,
    EditorSnapshot, FoldingClientCapabilities, SignatureHelpClientCapabilities,
};
use rg_parse::{LineIndex, parse_source_file};
use rg_project::{FileContext, ProjectSnapshot};
use rg_source::CapturedSource;
use rg_std::UniqueVec;

pub(super) use self::lifecycle::{QueryCancellation, QueryContext};
use crate::{
    engine::project::ProjectCoordinator,
    formatting,
    memory::MemoryControl,
    proto::{
        code_action, completion, folding as folding_proto, formatting as formatting_proto, hover,
        inlay_hint, navigation::SourceLocations, position, signature_help, symbols,
    },
};

/// Borrows the engine state used by one dispatched analysis request.
///
/// The runner does not own project generations. It prepares the query's analysis surface, borrows
/// the active input view through `ProjectCoordinator`, and lets the shared lifecycle policy release
/// request-scoped loads before the dispatcher accepts the next command.
pub(super) struct QueryRunner<'a> {
    project: &'a mut ProjectCoordinator,
    memory_control: Arc<dyn MemoryControl>,
}

impl<'a> QueryRunner<'a> {
    pub(super) fn new(
        project: &'a mut ProjectCoordinator,
        memory_control: Arc<dyn MemoryControl>,
    ) -> Self {
        Self {
            project,
            memory_control,
        }
    }

    /// Install the capture before selecting file owners: an edit can add a `mod` declaration
    /// and change which crate owns a file.
    fn with_editor_snapshot<R>(
        &self,
        editor: &EditorSnapshot,
        cancellation: &QueryCancellation<'_>,
        work: impl FnOnce(ProjectSnapshot<'_>) -> anyhow::Result<R>,
    ) -> anyhow::Result<R> {
        let sources = editor
            .documents()
            .iter()
            .map(|document| {
                CapturedSource::from_source_path(
                    document.source_path().to_path_buf(),
                    document.shared_text(),
                )
            })
            .collect::<Vec<_>>();
        self.project
            .saved_project()?
            .with_editor_snapshot(&sources, cancellation.token(), work)
    }

    fn with_document_analysis<R>(
        &self,
        query: &'static str,
        input: &DocumentSnapshot,
        selection: DocumentSelection,
        cancellation: &QueryCancellation<'_>,
        work: impl FnOnce(Option<DocumentAnalysis<'_>>) -> anyhow::Result<R>,
    ) -> anyhow::Result<R> {
        self.with_editor_snapshot(input.editor(), cancellation, |snapshot| {
            let Some(target) = Self::file_context(snapshot, input.document().source_path())? else {
                return work(None);
            };
            let line_index = snapshot.sources().line_index(target.file)?;
            let Some(selection) = selection.to_span(line_index) else {
                return work(None);
            };
            tracing::trace!(query, ?selection, crate_ref = ?target.crate_ref, "document analysis prepared");
            work(Some(DocumentAnalysis { snapshot, analysis: snapshot.analysis(), target, line_index, selection }))
        })
    }

    /// Compute completions using captured semantics and feature-specific syntax recovery.
    pub(super) fn completion(
        &mut self,
        input: DocumentPositionSnapshot,
        client_capabilities: CompletionClientCapabilities,
        cancellation: &QueryCancellation<'_>,
    ) -> anyhow::Result<CompletionResult> {
        let document = input.document();
        let position = input.position();
        let path = document.source_path().to_path_buf();
        let source_text = document.text();
        let started = Instant::now();
        let document_analysis_started = Instant::now();
        self.with_document_analysis(
            "completion",
            input.snapshot(),
            DocumentSelection::Position(position),
            cancellation,
            |current| {
                let Some(current) = current else {
                    return Ok(CompletionResult::default());
                };
                let document_analysis_us = document_analysis_started.elapsed().as_micros();
                let offset = current.selection.start;
                let completion_source_started = Instant::now();
                let completion_source = CompletionSource::new(source_text, offset);
                let completion_source_us = completion_source_started.elapsed().as_micros();

                let target = current.target;
                rg_std::check_cancel!(cancellation, "before semantic completion");
                let mut query = CompletionQuery::new(target.crate_ref, target.file, offset)
                    .with_client_capabilities(AnalysisCompletionClientCapabilities {
                        snippet_support: client_capabilities.snippet_support,
                        auto_import: client_capabilities.resolve_additional_text_edits,
                    });
                query = match completion_source.as_ref() {
                    Some(source) => query.with_completion_source(source),
                    None => query.with_source_text(source_text),
                };
                // Engine tests assert which imports are returned. Give them enough time
                // for the same search to finish on slower CI runners.
                #[cfg(test)]
                {
                    query.auto_import_budget = std::time::Duration::from_secs(30);
                }
                let analysis_compute_started = Instant::now();
                let result = current
                    .analysis
                    .completions_at(query)
                    .context("compute completions")?;
                let analysis_compute_us = analysis_compute_started.elapsed().as_micros();

                // Discard cancelled work before converting its result.
                rg_std::check_cancel!(cancellation, "after semantic completion");
                let protocol_conversion_started = Instant::now();
                // The import rows share a cursor and replacement span. Keep their source anchor
                // on the list until the server hands each row to the editor.
                let import_site = result
                    .items
                    .iter()
                    .find_map(|item| item.edit.filter(|_| item.import.is_some()))
                    .and_then(|edit| {
                        CompletionImportSite::capture(
                            source_text,
                            offset as usize,
                            edit.replace.start as usize..edit.replace.end as usize,
                            completion_source.as_ref()?.inline_module_path(),
                        )
                    });
                let completions: Vec<_> = result
                    .items
                    .into_iter()
                    .enumerate()
                    .map(|(index, item)| {
                        completion::completion_candidate(
                            item,
                            current.line_index,
                            index,
                            client_capabilities.label_details_support,
                        )
                    })
                    .collect();
                let protocol_conversion_us = protocol_conversion_started.elapsed().as_micros();

                tracing::trace!(
                    result_count = completions.len(),
                    document_analysis_us,
                    completion_source_us,
                    analysis_compute_us,
                    protocol_conversion_us,
                    "completion query phases finished"
                );

                tracing::trace!(
                    path = %path.display(),
                    line = position.line,
                    character = position.character,
                    result_count = completions.len(),
                    elapsed_ms = started.elapsed().as_millis(),
                    "completion query finished"
                );

                Ok(CompletionResult {
                    items: completions,
                    is_incomplete: result.is_incomplete,
                    import_site,
                })
            },
        )
        .context("prepare completion analysis")
    }

    /// Plan a selected import using one captured editor snapshot.
    ///
    /// A sibling edit can change the receiver type even if the cursor has not moved.
    pub(super) fn resolve_completion_import(
        &self,
        input: DocumentPositionSnapshot,
        import: &CompletionImport,
        candidate: Option<&CompletionImportCandidate>,
        cancellation: &QueryCancellation<'_>,
    ) -> anyhow::Result<Option<TextEdit>> {
        let Some(path) = Path::from_text(&import.path, None) else {
            return Ok(None);
        };
        self.with_document_analysis(
            "resolve_completion_import",
            input.snapshot(),
            DocumentSelection::Position(input.position()),
            cancellation,
            |current| {
                let Some(current) = current else {
                    return Ok(None);
                };
                let query = CompletionQuery::new(
                    current.target.crate_ref,
                    current.target.file,
                    current.selection.start,
                )
                .with_source_text(input.document().text());
                let selected = if let Some(candidate) = candidate {
                    let Some(kind) = completion::import_kind(candidate.kind) else {
                        return Ok(None);
                    };
                    Some((candidate.label.as_str(), kind))
                } else {
                    None
                };
                Ok(current
                    .analysis
                    .resolve_completion_import(
                        query,
                        &path,
                        import.anonymous,
                        selected,
                        candidate.and_then(|row| row.inline_module_path.as_deref()),
                    )?
                    .map(|edit| completion::import_edit(edit, current.line_index)))
            },
        )
    }

    /// Return source actions with complete edits for the captured document and selected range.
    pub(super) fn code_action(
        &mut self,
        input: DocumentRangeSnapshot,
        request_context: CodeActionRequestContext,
        cancellation: &QueryCancellation<'_>,
    ) -> anyhow::Result<Vec<CodeAction>> {
        let (input, range) = input.into_parts();
        let document = input.document();
        let path = document.source_path().to_path_buf();
        let started = Instant::now();

        // 1. Capture analysis for the request's editor revision and translate both ends of the LSP
        // range into byte offsets in that exact source text.
        self.with_document_analysis(
            "code_action",
            &input,
            DocumentSelection::Position(range.start),
            cancellation,
            |current| {
                let Some(current) = current else {
                    return Ok(Vec::new());
                };
                let Some(end) = current
                    .line_index
                    .offset_from_utf16_position(position::parse_position(range.end))
                else {
                    return Ok(Vec::new());
                };
                let start = current.selection.start;
                if start > end {
                    return Ok(Vec::new());
                }

                // 2. Translate protocol filters and query the selected crate interpretation.
                let request_kinds = request_context.kinds();
                let kinds = CodeActionKinds::none()
                    .with_quick_fix(request_kinds.quick_fix())
                    .with_refactor_rewrite(request_kinds.refactor_rewrite());
                let trigger = match request_context.trigger() {
                    CodeActionRequestTrigger::Invoked => CodeActionTrigger::Invoked,
                    CodeActionRequestTrigger::Automatic => CodeActionTrigger::Automatic,
                    CodeActionRequestTrigger::Unspecified => CodeActionTrigger::Unspecified,
                };
                let target = current.target;
                rg_std::check_cancel!(cancellation, "before code action analysis");
                let query = CodeActionQuery::new(
                    target.crate_ref,
                    target.file,
                    Span { start, end },
                    document.text(),
                )
                .with_kinds(kinds)
                .with_trigger(trigger);
                let actions = current
                    .analysis
                    .code_actions(query)
                    .context("compute code actions")?;
                rg_std::check_cancel!(cancellation, "after code action analysis");

                // 3. Convert UTF-8 edits only after analysis is finished, attaching the URI and captured
                // document version to every action.
                let mut lsp_actions = Vec::new();
                for action in actions {
                    lsp_actions.push(
                        code_action::code_action(
                            document.path(),
                            document.client_version(),
                            current.line_index,
                            action,
                        )
                        .context("convert code action")?,
                    );
                }
                tracing::trace!(
                    path = %path.display(),
                    result_count = lsp_actions.len(),
                    elapsed_ms = started.elapsed().as_millis(),
                    "code action query finished"
                );
                Ok(lsp_actions)
            },
        )
        .context("prepare code action analysis")
    }

    /// Return hover information from the selected crate context.
    ///
    /// A hover is read from one document, but its item links can point into other open files.
    /// The input includes those documents so links can follow declarations moved by unsaved edits.
    pub(super) fn hover(
        &mut self,
        input: DocumentPositionSnapshot,
        cancellation: &QueryCancellation<'_>,
    ) -> anyhow::Result<Option<Hover>> {
        let document = input.document();
        let position = input.position();
        let path = document.source_path().to_path_buf();
        let started = Instant::now();
        self.with_document_analysis(
            "hover",
            input.snapshot(),
            DocumentSelection::Position(position),
            cancellation,
            |current| {
                let Some(current) = current else {
                    return Ok(None);
                };

                let target = current.target;
                let destinations =
                    SourceLocations::new(current.snapshot, input.snapshot().editor());
                let hover = if let Some(info) = current
                    .analysis
                    .hover(target.crate_ref, target.file, current.selection.start)
                    .context("compute hover")?
                {
                    hover::hover(info, current.line_index, |destination| {
                        destinations.for_target(destination)
                    })
                    .context("render hover documentation")?
                } else {
                    None
                };

                tracing::trace!(
                    path = %path.display(),
                    line = position.line,
                    character = position.character,
                    has_hover = hover.is_some(),
                    elapsed_ms = started.elapsed().as_millis(),
                    "hover query finished"
                );
                Ok(hover)
            },
        )
        .context("prepare hover analysis")
    }

    pub(super) fn signature_help(
        &mut self,
        input: DocumentPositionSnapshot,
        client_capabilities: SignatureHelpClientCapabilities,
        cancellation: &QueryCancellation<'_>,
    ) -> anyhow::Result<Option<SignatureHelp>> {
        self.with_document_analysis(
            "signature_help",
            input.snapshot(),
            DocumentSelection::Position(input.position()),
            cancellation,
            |current| {
                let Some(current) = current else {
                    return Ok(None);
                };
                let help = current.analysis.signature_help(
                    current.target.crate_ref,
                    current.target.file,
                    current.selection.start,
                )?;
                Ok(help.map(|help| signature_help::signature_help(help, client_capabilities)))
            },
        )
        .context("prepare signature help analysis")
    }

    /// Build the document outline directly from the syntax shown by the editor.
    pub(super) fn document_symbol(
        &mut self,
        input: DocumentSnapshot,
    ) -> anyhow::Result<Vec<DocumentSymbol>> {
        let document = input.document();
        let edition = self.rust_edition_for_path(document.source_path())?;
        let syntax = parse_source_file(document.text(), edition).tree();
        let line_index = LineIndex::new(document.text());
        Ok(Analysis::document_symbols_from_syntax(&syntax)
            .into_iter()
            .map(|symbol| symbols::document_symbol(&line_index, symbol))
            .collect())
    }

    /// Build folding ranges directly from the syntax shown by the editor.
    pub(super) fn folding_range(
        &mut self,
        document: EditorDocumentSnapshot,
        client_capabilities: FoldingClientCapabilities,
    ) -> anyhow::Result<Vec<FoldingRange>> {
        let path = document.source_path().to_path_buf();
        let started = Instant::now();
        let edition = self
            .rust_edition_for_path(&path)
            .context("resolve folding-range edition")?;
        let syntax = parse_source_file(document.text(), edition).tree();
        let line_index = LineIndex::new(document.text());
        let lsp_ranges = Analysis::folding_ranges_from_syntax(&syntax)
            .into_iter()
            .filter_map(|fold| {
                folding_proto::folding_range(
                    document.text(),
                    &line_index,
                    client_capabilities.line_folding_only,
                    fold,
                )
            })
            .collect::<Vec<_>>();

        tracing::trace!(
            path = %path.display(),
            result_count = lsp_ranges.len(),
            elapsed_ms = started.elapsed().as_millis(),
            "folding range query finished"
        );

        Ok(lsp_ranges)
    }

    /// Format the live editor text using the owning package's Rust edition.
    ///
    /// Formatting does not need semantic materialization. The saved project is consulted only for
    /// edition metadata, and documents outside known packages use the newest supported edition.
    pub(super) fn formatting(
        &mut self,
        document: EditorDocumentSnapshot,
    ) -> anyhow::Result<Option<Vec<TextEdit>>> {
        let path = document.source_path().to_path_buf();
        let text = document.text();
        let started = Instant::now();
        let edition = self
            .rust_edition_for_path(&path)
            .context("resolve formatting edition")?;
        let line_index = LineIndex::new(text);
        let formatted_text = formatting::rustfmt(text, edition, line_index.line_endings())
            .context("format Rust source")?;
        let edits = formatting_proto::document_edits(text, formatted_text, &line_index)
            .context("build formatting edits")?;

        tracing::trace!(
            path = %path.display(),
            edition = %edition,
            edit_count = edits.len(),
            elapsed_ms = started.elapsed().as_millis(),
            "formatting query finished"
        );

        Ok(Some(edits))
    }

    /// Compute inlay hints in the selected crate context for the requested source range.
    pub(super) fn inlay_hint(
        &mut self,
        input: DocumentRangeSnapshot,
        max_length: Option<usize>,
        cancellation: &QueryCancellation<'_>,
    ) -> anyhow::Result<Vec<InlayHint>> {
        let (input, range) = input.into_parts();
        let document = input.document();
        let path = document.source_path().to_path_buf();
        let started = Instant::now();
        self.with_document_analysis(
            "inlay_hint",
            &input,
            DocumentSelection::Range(range),
            cancellation,
            |current| {
                let Some(current) = current else {
                    return Ok(Vec::new());
                };
                let text_range = current.selection;
                let target = current.target;
                rg_std::check_cancel!(cancellation, "before inlay hint analysis");
                let hints = current
                    .analysis
                    .inlay_hints(target.crate_ref, target.file, text_range, max_length)
                    .context("compute inlay hints")?;

                let mut lsp_hints = hints
                    .into_iter()
                    .map(|hint| inlay_hint::inlay_hint_with_line_index(current.line_index, hint))
                    .collect::<Vec<_>>();

                // Clamping a start beyond the document back to EOF can select text before the requested
                // coverage. The response still belongs to the client's original numeric range.
                lsp_hints.retain(|hint| range.start <= hint.position && hint.position <= range.end);
                tracing::trace!(
                    path = %path.display(),
                    result_count = lsp_hints.len(),
                    elapsed_ms = started.elapsed().as_millis(),
                    "inlay hint query finished"
                );

                Ok(lsp_hints)
            },
        )
        .context("prepare inlay hint analysis")
    }

    /// Search declarations in the complete editor view without selecting a target document.
    pub(super) fn workspace_symbol(
        &mut self,
        editor: &EditorSnapshot,
        query: &str,
        cancellation: &QueryCancellation<'_>,
    ) -> anyhow::Result<Vec<WorkspaceSymbol>> {
        let started = Instant::now();
        let lsp_symbols = self
            .with_editor_snapshot(editor, cancellation, |snapshot| {
                let analysis = snapshot.analysis();
                let mut lsp_symbols = UniqueVec::new();

                for symbol in analysis
                    .workspace_symbols(query)
                    .context("search workspace symbols")?
                {
                    rg_std::check_cancel!(cancellation, "workspace symbol conversion");
                    let Some(symbol) = symbols::workspace_symbol(snapshot, editor, symbol)
                        .context("convert workspace symbol")?
                    else {
                        continue;
                    };
                    lsp_symbols.push(symbol);
                }
                Ok(lsp_symbols.into_vec())
            })
            .context("run workspace-symbol query")?;

        tracing::trace!(
            query,
            result_count = lsp_symbols.len(),
            elapsed_ms = started.elapsed().as_millis(),
            "workspace symbol query finished"
        );

        Ok(lsp_symbols)
    }
}

/// Editor coordinates interpreted in the installed input view.
enum DocumentSelection {
    Position(Position),
    /// Inlay coverage may extend past the text after an edit; intersect it with the document.
    Range(Range),
}

impl DocumentSelection {
    fn to_span(&self, line_index: &LineIndex) -> Option<Span> {
        match self {
            Self::Position(position) => line_index
                .offset_from_utf16_position(position::parse_position(*position))
                .map(|offset| Span {
                    start: offset,
                    end: offset,
                }),
            Self::Range(range) => {
                let start = line_index
                    .offset_from_utf16_position_clamped(position::parse_position(range.start));
                let end = line_index
                    .offset_from_utf16_position_clamped(position::parse_position(range.end));
                Some(Span { start, end })
            }
        }
    }
}

/// Analysis and source selection prepared for one document query.
struct DocumentAnalysis<'project> {
    snapshot: ProjectSnapshot<'project>,
    analysis: Analysis<'project>,
    target: FileContext,
    line_index: &'project LineIndex,
    selection: Span,
}
