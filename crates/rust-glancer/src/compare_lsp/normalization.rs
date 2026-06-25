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
use ls_types::{Location, LocationLink, Range, Uri};
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

    pub(crate) fn summary_line(&self) -> String {
        format!("{} query cases normalized", self.results.len())
    }

    pub(crate) fn server_summary_line(&self, server: ServerUnderTest) -> String {
        let mut total_locations = 0;
        let mut total_unmapped_locations = 0;
        let mut hover_present_count = 0;
        let mut hover_absent_count = 0;
        let mut malformed_success_count = 0;
        let mut error_count = 0;
        let mut timeout_count = 0;
        let mut transport_failure_count = 0;
        let mut normalized_results = Vec::new();

        for query in &self.results {
            let outcome = query.outcome(server);
            match &outcome.value {
                NormalizedOutcome::Locations(locations) => {
                    total_locations += locations.locations.len();
                    total_unmapped_locations += locations.unmapped.len();
                }
                NormalizedOutcome::Hover { present: true } => hover_present_count += 1,
                NormalizedOutcome::Hover { present: false } => hover_absent_count += 1,
                NormalizedOutcome::MalformedSuccess { .. } => malformed_success_count += 1,
                NormalizedOutcome::Error { .. } => error_count += 1,
                NormalizedOutcome::Timeout => timeout_count += 1,
                NormalizedOutcome::TransportFailure { .. } => transport_failure_count += 1,
            }

            normalized_results.push(format!(
                "{}:{}={} in {}",
                query.kind().label(),
                compact(query.label()),
                outcome.value.summary(),
                format_duration(outcome.latency),
            ));
        }

        format!(
            "{} normalized_locations={total_locations}, unmapped_locations={total_unmapped_locations}, \
             hover_present={hover_present_count}, hover_absent={hover_absent_count}, \
             malformed_successes={malformed_success_count}, errors={error_count}, \
             timeouts={timeout_count}, transport_failures={transport_failure_count}, normalized=[{}]",
            server.label(),
            normalized_results.join(", "),
        )
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
                QueryKind::References { .. } | QueryKind::GotoDefinition => {
                    match NormalizedLocationSet::from_json(fixture_root, raw) {
                        Ok(locations) => Self::Locations(locations),
                        Err(message) => Self::MalformedSuccess { message },
                    }
                }
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

    pub(crate) fn summary(&self) -> String {
        match self {
            Self::Locations(locations) => locations.summary(),
            Self::Hover { present: true } => "hover present".to_string(),
            Self::Hover { present: false } => "hover absent".to_string(),
            Self::MalformedSuccess { message } => format!("malformed({})", compact(message)),
            Self::Error { code, message } => format!("error({code}: {})", compact(message)),
            Self::Timeout => "timeout".to_string(),
            Self::TransportFailure { message } => format!("transport({})", compact(message)),
        }
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

    fn summary(&self) -> String {
        if self.unmapped.is_empty() {
            return format!("{} locations", self.locations.len());
        }

        format!(
            "{} locations, {} unmapped (first: {})",
            self.locations.len(),
            self.unmapped.len(),
            self.unmapped
                .first()
                .map(UnmappedLocation::summary)
                .unwrap_or_else(|| "none".to_string()),
        )
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
        format!("{}: {}", self.reason, compact(&self.uri))
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

fn compact(message: &str) -> String {
    let mut message = message.split_whitespace().collect::<Vec<_>>().join(" ");
    const MAX_LEN: usize = 80;
    if message.len() > MAX_LEN {
        message.truncate(MAX_LEN);
        message.push_str("...");
    }
    message
}

fn format_duration(duration: Duration) -> String {
    format!("{:.1}ms", duration.as_secs_f64() * 1_000.0)
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
            locations.summary().contains("URI is not a file URI"),
            "summary should explain why the location was not mapped",
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
