//! Normalized query results for comparison and reporting.
//!
//! The execution layer keeps raw JSON because the two servers may legally choose different LSP
//! response shapes. This layer turns the location-producing shapes into one fixture-relative set
//! so later scoring can compare meaning instead of protocol spelling.

use std::{
    collections::BTreeSet,
    path::{Component, Path},
    time::Duration,
};

use anyhow::Context as _;
use ls_types::{
    AnnotatedTextEdit, DocumentChangeOperation, DocumentChanges, DocumentHighlight, DocumentSymbol,
    DocumentSymbolResponse, InlayHint, InlayHintKind, InlayHintLabel, Location, LocationLink,
    OneOf, PrepareRenameResponse, Range, ResourceOp, SymbolInformation, SymbolKind, TextEdit, Uri,
    WorkspaceEdit, WorkspaceSymbol, WorkspaceSymbolResponse,
};
use serde_json::Value;

use crate::compare_lsp::{
    execution::{ExecutionSummary, QueryExecution, RawOutcome, RawServerOutcome, ServerUnderTest},
    fixture::Fixture,
    query::QueryKind,
};

/// Query outcomes after protocol shapes and file URIs have been made comparable.
#[derive(Debug)]
pub(crate) struct NormalizedSummary {
    results: Vec<NormalizedQueryExecution>,
}

impl NormalizedSummary {
    pub(crate) fn from_execution(
        fixture: &Fixture,
        execution: &ExecutionSummary,
    ) -> anyhow::Result<Self> {
        let fixture_root = fixture.root().canonicalize().with_context(|| {
            format!(
                "Canonicalizing LSP comparison fixture root {} failed",
                fixture.root().display(),
            )
        })?;
        let results = execution
            .results()
            .iter()
            .map(|query| NormalizedQueryExecution::from_raw(&fixture_root, query))
            .collect();

        Ok(Self { results })
    }

    pub(crate) fn results(&self) -> &[NormalizedQueryExecution] {
        &self.results
    }

    #[cfg(test)]
    pub(crate) fn test_from_results(results: Vec<NormalizedQueryExecution>) -> Self {
        Self { results }
    }
}

#[derive(Debug)]
pub(crate) struct NormalizedQueryExecution {
    label: &'static str,
    kind: QueryKind,
    rust_glancer: NormalizedServerOutcome,
    rust_analyzer: NormalizedServerOutcome,
}

impl NormalizedQueryExecution {
    fn from_raw(fixture_root: &Path, query: &QueryExecution) -> Self {
        Self {
            label: query.label(),
            kind: query.kind(),
            rust_glancer: NormalizedServerOutcome::from_raw(
                fixture_root,
                query.kind(),
                query.outcome(ServerUnderTest::RustGlancer),
            ),
            rust_analyzer: NormalizedServerOutcome::from_raw(
                fixture_root,
                query.kind(),
                query.outcome(ServerUnderTest::RustAnalyzer),
            ),
        }
    }

    pub(crate) fn label(&self) -> &'static str {
        self.label
    }

    pub(crate) fn kind(&self) -> QueryKind {
        self.kind
    }

    pub(crate) fn outcome(&self, server: ServerUnderTest) -> &NormalizedServerOutcome {
        match server {
            ServerUnderTest::RustGlancer => &self.rust_glancer,
            ServerUnderTest::RustAnalyzer => &self.rust_analyzer,
        }
    }

    #[cfg(test)]
    pub(crate) fn test_new(
        label: &'static str,
        kind: QueryKind,
        rust_glancer: NormalizedOutcome,
        rust_analyzer: NormalizedOutcome,
    ) -> Self {
        Self {
            label,
            kind,
            rust_glancer: NormalizedServerOutcome::test_new(rust_glancer),
            rust_analyzer: NormalizedServerOutcome::test_new(rust_analyzer),
        }
    }
}

#[derive(Debug)]
pub(crate) struct NormalizedServerOutcome {
    latency: Duration,
    value: NormalizedOutcome,
}

impl NormalizedServerOutcome {
    fn from_raw(fixture_root: &Path, kind: QueryKind, outcome: &RawServerOutcome) -> Self {
        Self {
            latency: outcome.latency(),
            value: NormalizedOutcome::from_raw(fixture_root, kind, outcome.value()),
        }
    }

    pub(crate) fn value(&self) -> &NormalizedOutcome {
        &self.value
    }

    pub(crate) fn latency(&self) -> Duration {
        self.latency
    }

    #[cfg(test)]
    pub(crate) fn test_new(value: NormalizedOutcome) -> Self {
        Self {
            latency: Duration::ZERO,
            value,
        }
    }
}

#[derive(Debug)]
pub(crate) enum NormalizedOutcome {
    Locations(NormalizedLocationSet),
    Ranges(NormalizedRangeSet),
    Symbols(NormalizedSymbolSet),
    InlayHints(NormalizedInlayHintSet),
    PrepareRenames(NormalizedPrepareRenameSet),
    RenameEdits(NormalizedTextEditSet),
    Hover { present: bool },
    MalformedSuccess { message: String },
    Error { code: i64, message: String },
    Timeout,
    TransportFailure { message: String },
}

impl NormalizedOutcome {
    fn from_raw(fixture_root: &Path, kind: QueryKind, raw_outcome: &RawOutcome) -> Self {
        match raw_outcome {
            RawOutcome::Success { raw, .. } => match kind {
                QueryKind::References { .. }
                | QueryKind::GotoDefinition
                | QueryKind::TypeDefinition
                | QueryKind::Implementation => {
                    match NormalizedLocationSet::from_json(fixture_root, raw) {
                        Ok(locations) => Self::Locations(locations),
                        Err(message) => Self::MalformedSuccess { message },
                    }
                }
                QueryKind::PrepareRename => match NormalizedPrepareRenameSet::from_json(raw) {
                    Ok(targets) => Self::PrepareRenames(targets),
                    Err(message) => Self::MalformedSuccess { message },
                },
                QueryKind::Rename => match NormalizedTextEditSet::from_json(fixture_root, raw) {
                    Ok(edits) => Self::RenameEdits(edits),
                    Err(message) => Self::MalformedSuccess { message },
                },
                QueryKind::DocumentHighlight => match NormalizedRangeSet::from_json(raw) {
                    Ok(ranges) => Self::Ranges(ranges),
                    Err(message) => Self::MalformedSuccess { message },
                },
                QueryKind::DocumentSymbol => {
                    match NormalizedSymbolSet::from_document_json(fixture_root, raw) {
                        Ok(symbols) => Self::Symbols(symbols),
                        Err(message) => Self::MalformedSuccess { message },
                    }
                }
                QueryKind::WorkspaceSymbol => {
                    match NormalizedSymbolSet::from_workspace_json(fixture_root, raw) {
                        Ok(symbols) => Self::Symbols(symbols),
                        Err(message) => Self::MalformedSuccess { message },
                    }
                }
                QueryKind::InlayHint => match NormalizedInlayHintSet::from_json(raw) {
                    Ok(hints) => Self::InlayHints(hints),
                    Err(message) => Self::MalformedSuccess { message },
                },
                QueryKind::Hover => Self::Hover {
                    present: !raw.is_null(),
                },
            },
            RawOutcome::Error { code, message } => Self::Error {
                code: *code,
                message: message.clone(),
            },
            RawOutcome::Timeout => Self::Timeout,
            RawOutcome::TransportFailure { message } => Self::TransportFailure {
                message: message.clone(),
            },
        }
    }
}

#[derive(Debug)]
pub(crate) struct NormalizedPrepareRenameSet {
    targets: Vec<NormalizedPrepareRenameTarget>,
}

impl NormalizedPrepareRenameSet {
    fn from_json(raw: &Value) -> Result<Self, String> {
        if raw.is_null() {
            return Ok(Self {
                targets: Vec::new(),
            });
        }

        let response =
            serde_json::from_value::<PrepareRenameResponse>(raw.clone()).map_err(|_| {
                format!(
                    "unsupported prepare-rename response shape {}",
                    json_shape(raw)
                )
            })?;
        let target = match response {
            PrepareRenameResponse::Range(range) => Some(NormalizedPrepareRenameTarget {
                range: Some(NormalizedRange::from_lsp(range)),
                default_behavior: false,
            }),
            PrepareRenameResponse::RangeWithPlaceholder { range, .. } => {
                Some(NormalizedPrepareRenameTarget {
                    range: Some(NormalizedRange::from_lsp(range)),
                    default_behavior: false,
                })
            }
            PrepareRenameResponse::DefaultBehavior { default_behavior } => default_behavior
                .then_some(NormalizedPrepareRenameTarget {
                    range: None,
                    default_behavior: true,
                }),
        };

        Ok(Self {
            targets: target.into_iter().collect(),
        })
    }

    pub(crate) fn targets(&self) -> &[NormalizedPrepareRenameTarget] {
        &self.targets
    }
}

#[derive(Debug)]
pub(crate) struct NormalizedTextEditSet {
    edits: Vec<NormalizedTextEdit>,
    unmapped: Vec<UnmappedLocation>,
}

impl NormalizedTextEditSet {
    fn from_json(fixture_root: &Path, raw: &Value) -> Result<Self, String> {
        if raw.is_null() {
            return Ok(Self {
                edits: Vec::new(),
                unmapped: Vec::new(),
            });
        }

        let workspace_edit = serde_json::from_value::<WorkspaceEdit>(raw.clone())
            .map_err(|_| format!("unsupported rename response shape {}", json_shape(raw)))?;
        let mut edits = BTreeSet::new();
        let mut unmapped = Vec::new();

        if let Some(document_changes) = workspace_edit.document_changes {
            Self::push_document_changes(fixture_root, document_changes, &mut edits, &mut unmapped);
        } else if let Some(changes) = workspace_edit.changes {
            for (uri, text_edits) in changes {
                for edit in text_edits {
                    Self::push_text_edit(fixture_root, &uri, edit, &mut edits, &mut unmapped);
                }
            }
        }

        Ok(Self {
            edits: edits.into_iter().collect(),
            unmapped,
        })
    }

    fn push_document_changes(
        fixture_root: &Path,
        document_changes: DocumentChanges,
        edits: &mut BTreeSet<NormalizedTextEdit>,
        unmapped: &mut Vec<UnmappedLocation>,
    ) {
        match document_changes {
            DocumentChanges::Edits(document_edits) => {
                for document_edit in document_edits {
                    let uri = document_edit.text_document.uri;
                    for edit in document_edit.edits {
                        Self::push_one_of_text_edit(fixture_root, &uri, edit, edits, unmapped);
                    }
                }
            }
            DocumentChanges::Operations(operations) => {
                for operation in operations {
                    match operation {
                        DocumentChangeOperation::Edit(document_edit) => {
                            let uri = document_edit.text_document.uri;
                            for edit in document_edit.edits {
                                Self::push_one_of_text_edit(
                                    fixture_root,
                                    &uri,
                                    edit,
                                    edits,
                                    unmapped,
                                );
                            }
                        }
                        DocumentChangeOperation::Op(resource_op) => {
                            unmapped.push(Self::resource_operation_location(resource_op));
                        }
                    }
                }
            }
        }
    }

    fn push_one_of_text_edit(
        fixture_root: &Path,
        uri: &Uri,
        edit: OneOf<TextEdit, AnnotatedTextEdit>,
        edits: &mut BTreeSet<NormalizedTextEdit>,
        unmapped: &mut Vec<UnmappedLocation>,
    ) {
        let edit = match edit {
            OneOf::Left(edit) => edit,
            OneOf::Right(edit) => edit.text_edit,
        };
        Self::push_text_edit(fixture_root, uri, edit, edits, unmapped);
    }

    fn push_text_edit(
        fixture_root: &Path,
        uri: &Uri,
        edit: TextEdit,
        edits: &mut BTreeSet<NormalizedTextEdit>,
        unmapped: &mut Vec<UnmappedLocation>,
    ) {
        match NormalizedTextEdit::from_lsp(fixture_root, uri, edit) {
            Ok(edit) => {
                edits.insert(edit);
            }
            Err(location) => unmapped.push(location),
        }
    }

    fn resource_operation_location(resource_op: ResourceOp) -> UnmappedLocation {
        match resource_op {
            ResourceOp::Create(file) => UnmappedLocation {
                uri: file.uri.as_str().to_string(),
                reason: "rename returned a create-file operation".to_string(),
            },
            ResourceOp::Rename(file) => UnmappedLocation {
                uri: format!("{} -> {}", file.old_uri.as_str(), file.new_uri.as_str()),
                reason: "rename returned a rename-file operation".to_string(),
            },
            ResourceOp::Delete(file) => UnmappedLocation {
                uri: file.uri.as_str().to_string(),
                reason: "rename returned a delete-file operation".to_string(),
            },
        }
    }

    pub(crate) fn edits(&self) -> &[NormalizedTextEdit] {
        &self.edits
    }

    pub(crate) fn unmapped_count(&self) -> usize {
        self.unmapped.len()
    }

    pub(crate) fn unmapped_summaries(&self) -> Vec<String> {
        self.unmapped
            .iter()
            .map(UnmappedLocation::summary)
            .collect()
    }
}

#[derive(Debug)]
pub(crate) struct NormalizedInlayHintSet {
    hints: Vec<NormalizedInlayHint>,
}

impl NormalizedInlayHintSet {
    fn from_json(raw: &Value) -> Result<Self, String> {
        if raw.is_null() {
            return Ok(Self { hints: Vec::new() });
        }

        let hints = serde_json::from_value::<Vec<InlayHint>>(raw.clone())
            .map_err(|_| format!("unsupported inlay-hint response shape {}", json_shape(raw)))?;
        let hints = hints
            .into_iter()
            .map(NormalizedInlayHint::from_lsp)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();

        Ok(Self { hints })
    }

    pub(crate) fn hints(&self) -> &[NormalizedInlayHint] {
        &self.hints
    }
}

#[derive(Debug)]
pub(crate) struct NormalizedRangeSet {
    ranges: Vec<NormalizedRange>,
}

impl NormalizedRangeSet {
    fn from_json(raw: &Value) -> Result<Self, String> {
        if raw.is_null() {
            return Ok(Self { ranges: Vec::new() });
        }

        let highlights =
            serde_json::from_value::<Vec<DocumentHighlight>>(raw.clone()).map_err(|_| {
                format!(
                    "unsupported document-highlight response shape {}",
                    json_shape(raw)
                )
            })?;
        let ranges = highlights
            .into_iter()
            .map(|highlight| NormalizedRange::from_lsp(highlight.range))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();

        Ok(Self { ranges })
    }

    pub(crate) fn ranges(&self) -> &[NormalizedRange] {
        &self.ranges
    }
}

#[derive(Debug)]
pub(crate) struct NormalizedSymbolSet {
    symbols: Vec<NormalizedSymbol>,
    unmapped: Vec<UnmappedLocation>,
}

impl NormalizedSymbolSet {
    fn from_document_json(fixture_root: &Path, raw: &Value) -> Result<Self, String> {
        if raw.is_null() {
            return Ok(Self {
                symbols: Vec::new(),
                unmapped: Vec::new(),
            });
        }

        let response =
            serde_json::from_value::<DocumentSymbolResponse>(raw.clone()).map_err(|_| {
                format!(
                    "unsupported document-symbol response shape {}",
                    json_shape(raw)
                )
            })?;
        let mut symbols = BTreeSet::new();
        let mut unmapped = Vec::new();

        match response {
            DocumentSymbolResponse::Nested(document_symbols) => {
                for symbol in document_symbols {
                    NormalizedSymbol::push_document_symbol(symbol, &mut symbols);
                }
            }
            DocumentSymbolResponse::Flat(symbol_infos) => {
                for symbol in symbol_infos {
                    match NormalizedSymbol::from_document_symbol_information(fixture_root, symbol) {
                        Ok(symbol) => {
                            symbols.insert(symbol);
                        }
                        Err(location) => unmapped.push(location),
                    }
                }
            }
        }

        Ok(Self {
            symbols: symbols.into_iter().collect(),
            unmapped,
        })
    }

    fn from_workspace_json(fixture_root: &Path, raw: &Value) -> Result<Self, String> {
        if raw.is_null() {
            return Ok(Self {
                symbols: Vec::new(),
                unmapped: Vec::new(),
            });
        }

        let response =
            serde_json::from_value::<WorkspaceSymbolResponse>(raw.clone()).map_err(|_| {
                format!(
                    "unsupported workspace-symbol response shape {}",
                    json_shape(raw)
                )
            })?;
        let mut symbols = BTreeSet::new();
        let mut unmapped = Vec::new();

        match response {
            WorkspaceSymbolResponse::Nested(workspace_symbols) => {
                for symbol in workspace_symbols {
                    match NormalizedSymbol::from_workspace_symbol(fixture_root, symbol) {
                        Ok(symbol) => {
                            symbols.insert(symbol);
                        }
                        Err(location) => unmapped.push(location),
                    }
                }
            }
            WorkspaceSymbolResponse::Flat(symbol_infos) => {
                for symbol in symbol_infos {
                    match NormalizedSymbol::from_symbol_information(fixture_root, symbol) {
                        Ok(symbol) => {
                            symbols.insert(symbol);
                        }
                        Err(location) => unmapped.push(location),
                    }
                }
            }
        }

        Ok(Self {
            symbols: symbols.into_iter().collect(),
            unmapped,
        })
    }

    pub(crate) fn symbols(&self) -> &[NormalizedSymbol] {
        &self.symbols
    }

    pub(crate) fn unmapped_count(&self) -> usize {
        self.unmapped.len()
    }

    pub(crate) fn unmapped_summaries(&self) -> Vec<String> {
        self.unmapped
            .iter()
            .map(UnmappedLocation::summary)
            .collect()
    }
}

#[derive(Debug)]
pub(crate) struct NormalizedLocationSet {
    locations: Vec<NormalizedLocation>,
    unmapped: Vec<UnmappedLocation>,
}

impl NormalizedLocationSet {
    fn from_json(fixture_root: &Path, raw: &Value) -> Result<Self, String> {
        let protocol_locations = ProtocolLocations::from_json(raw)?;
        let mut locations = BTreeSet::new();
        let mut unmapped = Vec::new();

        for protocol_location in protocol_locations.locations {
            match NormalizedLocation::from_protocol(fixture_root, protocol_location) {
                Ok(location) => {
                    locations.insert(location);
                }
                Err(location) => unmapped.push(location),
            }
        }

        Ok(Self {
            locations: locations.into_iter().collect(),
            unmapped,
        })
    }

    pub(crate) fn locations(&self) -> &[NormalizedLocation] {
        &self.locations
    }

    pub(crate) fn unmapped_count(&self) -> usize {
        self.unmapped.len()
    }

    pub(crate) fn unmapped_summaries(&self) -> Vec<String> {
        self.unmapped
            .iter()
            .map(UnmappedLocation::summary)
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn test_from_locations(locations: Vec<NormalizedLocation>) -> Self {
        let locations = locations
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();

        Self {
            locations,
            unmapped: Vec::new(),
        }
    }
}

struct ProtocolLocations {
    locations: Vec<ProtocolLocation>,
}

impl ProtocolLocations {
    fn from_json(raw: &Value) -> Result<Self, String> {
        if raw.is_null() {
            return Ok(Self {
                locations: Vec::new(),
            });
        }

        // LSP definition responses can be either Location or LocationLink based, while references
        // are expected to be plain Locations. Probe both families so the fixture vector can share
        // one normalization path across request kinds.
        if let Ok(locations) = serde_json::from_value::<Vec<Location>>(raw.clone()) {
            return Ok(Self {
                locations: locations
                    .into_iter()
                    .map(ProtocolLocation::Location)
                    .collect(),
            });
        }
        if let Ok(location) = serde_json::from_value::<Location>(raw.clone()) {
            return Ok(Self {
                locations: vec![ProtocolLocation::Location(location)],
            });
        }
        if let Ok(links) = serde_json::from_value::<Vec<LocationLink>>(raw.clone()) {
            return Ok(Self {
                locations: links
                    .into_iter()
                    .map(ProtocolLocation::LocationLink)
                    .collect(),
            });
        }
        if let Ok(link) = serde_json::from_value::<LocationLink>(raw.clone()) {
            return Ok(Self {
                locations: vec![ProtocolLocation::LocationLink(link)],
            });
        }

        Err(format!(
            "unsupported location response shape {}",
            json_shape(raw),
        ))
    }
}

enum ProtocolLocation {
    Location(Location),
    LocationLink(LocationLink),
}

impl ProtocolLocation {
    fn uri(&self) -> &Uri {
        match self {
            Self::Location(location) => &location.uri,
            Self::LocationLink(location) => &location.target_uri,
        }
    }

    fn range(&self) -> Range {
        match self {
            Self::Location(location) => location.range,
            Self::LocationLink(location) => location.target_range,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct NormalizedPrepareRenameTarget {
    range: Option<NormalizedRange>,
    default_behavior: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct NormalizedInlayHint {
    line: u32,
    character: u32,
    kind: Option<i64>,
    label: String,
}

impl NormalizedInlayHint {
    fn from_lsp(hint: InlayHint) -> Self {
        Self {
            line: hint.position.line,
            character: hint.position.character,
            kind: hint.kind.map(inlay_hint_kind_code),
            label: inlay_hint_label(hint.label),
        }
    }

    pub(crate) const fn line(&self) -> u32 {
        self.line
    }

    pub(crate) fn label(&self) -> &str {
        &self.label
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct NormalizedTextEdit {
    path: String,
    range: NormalizedRange,
    new_text: String,
}

impl NormalizedTextEdit {
    fn from_lsp(fixture_root: &Path, uri: &Uri, edit: TextEdit) -> Result<Self, UnmappedLocation> {
        Ok(Self {
            path: fixture_relative_file_uri(fixture_root, uri)?,
            range: NormalizedRange::from_lsp(edit.range),
            new_text: edit.new_text,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct NormalizedLocation {
    path: String,
    range: NormalizedRange,
}

impl NormalizedLocation {
    #[cfg(test)]
    pub(crate) fn test_new(path: &'static str, range: NormalizedRange) -> Self {
        Self {
            path: path.to_string(),
            range,
        }
    }

    fn from_protocol(
        fixture_root: &Path,
        location: ProtocolLocation,
    ) -> Result<Self, UnmappedLocation> {
        let uri = location.uri();
        let range = location.range();
        let uri_text = uri.as_str().to_string();

        if !uri.scheme().as_str().eq_ignore_ascii_case("file") {
            return Err(UnmappedLocation {
                uri: uri_text,
                reason: "URI is not a file URI".to_string(),
            });
        }

        let Some(path) = uri.to_file_path() else {
            return Err(UnmappedLocation {
                uri: uri_text,
                reason: "file URI has no path".to_string(),
            });
        };
        let path = path.into_owned();
        let relative = path
            .strip_prefix(fixture_root)
            .map_err(|_| UnmappedLocation {
                uri: uri_text,
                reason: format!(
                    "file path is outside fixture root {}",
                    fixture_root.display(),
                ),
            })?;

        Ok(Self {
            path: fixture_relative_path(relative),
            range: NormalizedRange::from_lsp(range),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct NormalizedSymbol {
    name: String,
    kind: i64,
    path: Option<String>,
    range: Option<NormalizedRange>,
}

impl NormalizedSymbol {
    fn push_document_symbol(symbol: DocumentSymbol, symbols: &mut BTreeSet<Self>) {
        let children = symbol.children.unwrap_or_default();
        symbols.insert(Self {
            name: symbol.name,
            kind: symbol_kind_code(symbol.kind),
            path: None,
            range: Some(NormalizedRange::from_lsp(symbol.selection_range)),
        });

        for child in children {
            Self::push_document_symbol(child, symbols);
        }
    }

    fn from_symbol_information(
        fixture_root: &Path,
        symbol: SymbolInformation,
    ) -> Result<Self, UnmappedLocation> {
        let location = NormalizedLocation::from_protocol(
            fixture_root,
            ProtocolLocation::Location(symbol.location),
        )?;
        Ok(Self {
            name: symbol.name,
            kind: symbol_kind_code(symbol.kind),
            path: Some(location.path),
            range: Some(location.range),
        })
    }

    fn from_document_symbol_information(
        fixture_root: &Path,
        symbol: SymbolInformation,
    ) -> Result<Self, UnmappedLocation> {
        let location = NormalizedLocation::from_protocol(
            fixture_root,
            ProtocolLocation::Location(symbol.location),
        )?;
        Ok(Self {
            name: symbol.name,
            kind: symbol_kind_code(symbol.kind),
            path: None,
            range: Some(location.range),
        })
    }

    fn from_workspace_symbol(
        fixture_root: &Path,
        symbol: WorkspaceSymbol,
    ) -> Result<Self, UnmappedLocation> {
        let (path, range) = match symbol.location {
            OneOf::Left(location) => {
                let location = NormalizedLocation::from_protocol(
                    fixture_root,
                    ProtocolLocation::Location(location),
                )?;
                (Some(location.path), Some(location.range))
            }
            OneOf::Right(location) => {
                let uri_text = location.uri.as_str().to_string();
                if !location.uri.scheme().as_str().eq_ignore_ascii_case("file") {
                    return Err(UnmappedLocation {
                        uri: uri_text,
                        reason: "URI is not a file URI".to_string(),
                    });
                }

                let Some(path) = location.uri.to_file_path() else {
                    return Err(UnmappedLocation {
                        uri: uri_text,
                        reason: "file URI has no path".to_string(),
                    });
                };
                let path = path.into_owned();
                let relative = path
                    .strip_prefix(fixture_root)
                    .map_err(|_| UnmappedLocation {
                        uri: uri_text,
                        reason: format!(
                            "file path is outside fixture root {}",
                            fixture_root.display(),
                        ),
                    })?;

                (Some(fixture_relative_path(relative)), None)
            }
        };

        Ok(Self {
            name: symbol.name,
            kind: symbol_kind_code(symbol.kind),
            path,
            range,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct NormalizedRange {
    start_line: u32,
    start_character: u32,
    end_line: u32,
    end_character: u32,
}

impl NormalizedRange {
    #[cfg(test)]
    pub(crate) const fn test_new(
        start_line: u32,
        start_character: u32,
        end_line: u32,
        end_character: u32,
    ) -> Self {
        Self {
            start_line,
            start_character,
            end_line,
            end_character,
        }
    }

    fn from_lsp(range: Range) -> Self {
        Self {
            start_line: range.start.line,
            start_character: range.start.character,
            end_line: range.end.line,
            end_character: range.end.character,
        }
    }
}

#[derive(Debug)]
struct UnmappedLocation {
    uri: String,
    reason: String,
}

impl UnmappedLocation {
    fn summary(&self) -> String {
        format!("{}: {}", self.reason, self.uri)
    }
}

fn fixture_relative_path(path: &Path) -> String {
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(segment) => components.push(segment.to_string_lossy().into_owned()),
            Component::CurDir => {}
            Component::ParentDir => components.push("..".to_string()),
            Component::RootDir | Component::Prefix(_) => {
                components.push(component.as_os_str().to_string_lossy().into_owned());
            }
        }
    }

    components.join("/")
}

fn fixture_relative_file_uri(fixture_root: &Path, uri: &Uri) -> Result<String, UnmappedLocation> {
    let uri_text = uri.as_str().to_string();
    if !uri.scheme().as_str().eq_ignore_ascii_case("file") {
        return Err(UnmappedLocation {
            uri: uri_text,
            reason: "URI is not a file URI".to_string(),
        });
    }

    let Some(path) = uri.to_file_path() else {
        return Err(UnmappedLocation {
            uri: uri_text,
            reason: "file URI has no path".to_string(),
        });
    };
    let path = path.into_owned();
    let relative = path
        .strip_prefix(fixture_root)
        .map_err(|_| UnmappedLocation {
            uri: uri_text,
            reason: format!(
                "file path is outside fixture root {}",
                fixture_root.display(),
            ),
        })?;

    Ok(fixture_relative_path(relative))
}

fn symbol_kind_code(kind: SymbolKind) -> i64 {
    serde_json::to_value(kind)
        .expect("SymbolKind should serialize as an integer")
        .as_i64()
        .expect("SymbolKind should serialize as an integer")
}

fn inlay_hint_kind_code(kind: InlayHintKind) -> i64 {
    serde_json::to_value(kind)
        .expect("InlayHintKind should serialize as an integer")
        .as_i64()
        .expect("InlayHintKind should serialize as an integer")
}

fn inlay_hint_label(label: InlayHintLabel) -> String {
    match label {
        InlayHintLabel::String(label) => label,
        InlayHintLabel::LabelParts(parts) => parts.into_iter().map(|part| part.value).collect(),
    }
}

fn json_shape(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(_) => "boolean".to_string(),
        Value::Number(_) => "number".to_string(),
        Value::String(_) => "string".to_string(),
        Value::Array(values) => format!("array(len={})", values.len()),
        Value::Object(fields) => {
            let mut keys = fields.keys().map(String::as_str).collect::<Vec<_>>();
            keys.sort_unstable();
            format!("object(keys={})", keys.join("|"))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
    };

    use serde_json::{Value, json};

    use super::{NormalizedLocationSet, NormalizedRange};

    #[test]
    fn normalizes_location_arrays_to_fixture_relative_sets() {
        let root = fixture_root("location-array");
        let a_uri = file_uri(&root, "src/a.rs");
        let b_uri = file_uri(&root, "src/b.rs");
        let raw = json!([
            location_json(&b_uri, range_json(3, 1, 3, 2)),
            location_json(&a_uri, range_json(1, 2, 1, 4)),
            location_json(&a_uri, range_json(1, 2, 1, 4)),
        ]);

        let locations =
            NormalizedLocationSet::from_json(&root, &raw).expect("location array should normalize");

        assert_eq!(locations.unmapped.len(), 0, "all file URIs should map");
        assert_eq!(locations.locations.len(), 2, "duplicates should collapse");
        assert_eq!(locations.locations[0].path, "src/a.rs");
        assert_eq!(
            locations.locations[0].range,
            NormalizedRange {
                start_line: 1,
                start_character: 2,
                end_line: 1,
                end_character: 4,
            },
        );
        assert_eq!(locations.locations[1].path, "src/b.rs");
    }

    #[test]
    fn normalizes_single_location_link_targets() {
        let root = fixture_root("location-link");
        let uri = file_uri(&root, "crates/core/src/lib.rs");
        let raw = json!({
            "targetUri": uri,
            "targetRange": range_json(10, 4, 10, 16),
            "targetSelectionRange": range_json(10, 4, 10, 16),
        });

        let locations = NormalizedLocationSet::from_json(&root, &raw)
            .expect("single LocationLink should normalize");

        assert_eq!(locations.unmapped.len(), 0, "link target should map");
        assert_eq!(locations.locations.len(), 1);
        assert_eq!(locations.locations[0].path, "crates/core/src/lib.rs");
        assert_eq!(
            locations.locations[0].range,
            NormalizedRange {
                start_line: 10,
                start_character: 4,
                end_line: 10,
                end_character: 16,
            },
        );
    }

    #[test]
    fn treats_null_as_an_empty_location_set() {
        let root = fixture_root("null");
        let locations = NormalizedLocationSet::from_json(&root, &Value::Null)
            .expect("null should be a valid empty response");

        assert_eq!(locations.locations.len(), 0);
        assert_eq!(locations.unmapped.len(), 0);
    }

    #[test]
    fn keeps_non_file_locations_as_unmapped_data() {
        let root = fixture_root("non-file");
        let raw = location_json("untitled:///scratch.rs", range_json(0, 0, 0, 3));

        let locations = NormalizedLocationSet::from_json(&root, &raw)
            .expect("non-file URI should still be a valid location payload");

        assert_eq!(locations.locations.len(), 0);
        assert_eq!(locations.unmapped.len(), 1);
        assert!(
            locations.unmapped_summaries()[0].contains("URI is not a file URI"),
            "unmapped details should explain why the location was not mapped",
        );
    }

    fn fixture_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir()
            .join("rust-glancer-compare-lsp-normalization")
            .join(format!("{}-{}", name, std::process::id()));
        fs::create_dir_all(&root).expect("test fixture root should be created");
        root.canonicalize()
            .expect("test fixture root should canonicalize")
    }

    fn file_uri(root: &Path, relative_path: &str) -> String {
        let path = root.join(relative_path);
        let parent = path.parent().expect("test path should have a parent");
        fs::create_dir_all(parent).expect("test file parent should be created");
        fs::write(&path, "").expect("test file should be written");
        ls_types::Uri::from_file_path(&path)
            .expect("test path should convert to a file URI")
            .as_str()
            .to_string()
    }

    fn location_json(uri: &str, range: Value) -> Value {
        json!({
            "uri": uri,
            "range": range,
        })
    }

    fn range_json(
        start_line: u32,
        start_character: u32,
        end_line: u32,
        end_character: u32,
    ) -> Value {
        json!({
            "start": {
                "line": start_line,
                "character": start_character,
            },
            "end": {
                "line": end_line,
                "character": end_character,
            },
        })
    }
}
