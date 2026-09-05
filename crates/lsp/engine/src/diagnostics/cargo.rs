use std::{
    collections::BTreeMap,
    io::Cursor,
    path::{Path, PathBuf},
};

use cargo_metadata::{
    Message,
    diagnostic::{Diagnostic as CargoDiagnostic, DiagnosticLevel, DiagnosticSpan},
};
use ls_types::{
    Diagnostic, DiagnosticRelatedInformation, DiagnosticSeverity, Location, NumberOrString,
    Position, Range,
};
use rg_lsp_proto::path_to_file_uri;
use rg_std::{NormalizedPathBuf, UniqueVec};

#[derive(Debug, Default)]
pub(crate) struct CargoDiagnostics {
    by_path: BTreeMap<NormalizedPathBuf, UniqueVec<Diagnostic>>,
}

impl CargoDiagnostics {
    pub(crate) fn parse(workspace_root: &Path, source: &str, stdout: &[u8], stderr: &[u8]) -> Self {
        let mut diagnostics = Self::default();
        diagnostics.parse_stream(workspace_root, source, stdout);
        diagnostics.parse_stream(workspace_root, source, stderr);
        diagnostics
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.by_path.is_empty()
    }

    pub(crate) fn path_count(&self) -> usize {
        self.by_path.len()
    }

    pub(crate) fn into_inner(self) -> BTreeMap<NormalizedPathBuf, Vec<Diagnostic>> {
        self.by_path
            .into_iter()
            .map(|(path, diagnostics)| (path, diagnostics.into_vec()))
            .collect()
    }

    #[cfg(test)]
    pub(super) fn from_map(by_path: BTreeMap<NormalizedPathBuf, Vec<Diagnostic>>) -> Self {
        Self {
            by_path: by_path
                .into_iter()
                .map(|(path, diagnostics)| (path, diagnostics.into_iter().collect()))
                .collect(),
        }
    }

    fn parse_stream(&mut self, workspace_root: &Path, source: &str, bytes: &[u8]) {
        for message in Message::parse_stream(Cursor::new(bytes)) {
            let message = match message {
                Ok(message) => message,
                Err(error) => {
                    tracing::trace!(
                        error = %error,
                        "ignored non-cargo-json diagnostics output line"
                    );
                    continue;
                }
            };

            let Message::CompilerMessage(message) = message else {
                continue;
            };

            let target_src_path = message.target.src_path.into_std_path_buf();
            for diagnostic in CargoDiagnosticMapper::new(
                workspace_root,
                &target_src_path,
                source,
                &message.message,
            )
            .map()
            {
                self.by_path
                    .entry(diagnostic.path)
                    .or_default()
                    .push(diagnostic.diagnostic);
            }
        }
    }
}

#[derive(Debug)]
struct MappedDiagnostic {
    path: NormalizedPathBuf,
    diagnostic: Diagnostic,
}

struct CargoDiagnosticMapper<'a> {
    workspace_root: &'a Path,
    package_root: Option<PathBuf>,
    source: &'a str,
    diagnostic: &'a CargoDiagnostic,
}

impl<'a> CargoDiagnosticMapper<'a> {
    fn new(
        workspace_root: &'a Path,
        target_src_path: &'a Path,
        source: &'a str,
        diagnostic: &'a CargoDiagnostic,
    ) -> Self {
        Self {
            workspace_root,
            package_root: Self::package_root_from_target_src_path(target_src_path),
            source,
            diagnostic,
        }
    }

    fn map(&self) -> Vec<MappedDiagnostic> {
        let mut mapped = Vec::new();

        // Rustc can mark several spans as primary. Publishing one LSP diagnostic per primary span
        // keeps the important locations visible without trying to recreate rustc's rendered text.
        for span in self.diagnostic.spans.iter().filter(|span| span.is_primary) {
            let Some(path) = self.resolve_span_path(span) else {
                continue;
            };
            let related_information = self.related_information();
            let diagnostic = Diagnostic {
                range: Self::range(span),
                severity: Self::severity(self.diagnostic.level),
                code: self
                    .diagnostic
                    .code
                    .as_ref()
                    .map(|code| NumberOrString::String(code.code.clone())),
                code_description: None,
                source: Some(self.source.to_string()),
                message: self.diagnostic.message.clone(),
                related_information,
                tags: None,
                data: None,
            };
            mapped.push(MappedDiagnostic { path, diagnostic });
        }

        mapped
    }

    fn related_information(&self) -> Option<Vec<DiagnosticRelatedInformation>> {
        let related = self
            .diagnostic
            .children
            .iter()
            .flat_map(|child| {
                child
                    .spans
                    .iter()
                    .filter_map(|span| self.related_information_for_span(child, span))
            })
            .collect::<Vec<_>>();

        (!related.is_empty()).then_some(related)
    }

    fn related_information_for_span(
        &self,
        child: &CargoDiagnostic,
        span: &DiagnosticSpan,
    ) -> Option<DiagnosticRelatedInformation> {
        let message = span
            .label
            .clone()
            .filter(|label| !label.is_empty())
            .unwrap_or_else(|| child.message.clone());
        if message.is_empty() {
            return None;
        }

        let path = self.resolve_span_path(span)?;
        let uri = path_to_file_uri(path).ok()?;
        Some(DiagnosticRelatedInformation {
            location: Location {
                uri,
                range: Self::range(span),
            },
            message,
        })
    }

    fn resolve_span_path(&self, span: &DiagnosticSpan) -> Option<NormalizedPathBuf> {
        let raw = PathBuf::from(&span.file_name);
        let candidate = if raw.is_absolute() {
            raw
        } else {
            // Cargo usually reports span files relative to the command working directory. If a
            // toolchain reports package-relative paths instead, the compiler message target root
            // is enough to derive the package root without running cargo metadata twice.
            let workspace_candidate = self.workspace_root.join(&raw);
            if workspace_candidate.exists() {
                workspace_candidate
            } else {
                self.package_root
                    .as_ref()
                    .map(|root| root.join(&raw))
                    .unwrap_or(workspace_candidate)
            }
        };

        match NormalizedPathBuf::from_absolute(&candidate) {
            Ok(path) => Some(path),
            Err(error) => {
                tracing::debug!(
                    span_path = %span.file_name,
                    candidate = %candidate.display(),
                    error = %error,
                    "ignored diagnostic span with an invalid filesystem path"
                );
                None
            }
        }
    }

    fn package_root_from_target_src_path(target_src_path: &Path) -> Option<PathBuf> {
        let mut dir = target_src_path.parent();
        while let Some(candidate) = dir {
            if candidate.join("Cargo.toml").is_file() {
                return Some(candidate.to_path_buf());
            }
            dir = candidate.parent();
        }
        None
    }

    fn severity(level: DiagnosticLevel) -> Option<DiagnosticSeverity> {
        match level {
            DiagnosticLevel::Ice | DiagnosticLevel::Error => Some(DiagnosticSeverity::ERROR),
            DiagnosticLevel::Warning => Some(DiagnosticSeverity::WARNING),
            DiagnosticLevel::Note | DiagnosticLevel::FailureNote => {
                Some(DiagnosticSeverity::INFORMATION)
            }
            DiagnosticLevel::Help => Some(DiagnosticSeverity::HINT),
            _ => None,
        }
    }

    fn range(span: &DiagnosticSpan) -> Range {
        Range {
            start: Self::position(span, span.line_start, span.column_start),
            end: Self::position(span, span.line_end, span.column_end),
        }
    }

    fn position(span: &DiagnosticSpan, line: usize, column: usize) -> Position {
        let line_index = line.saturating_sub(span.line_start);
        let column = column.saturating_sub(1);
        let character = span
            .text
            .get(line_index)
            .map(|line| line.text.chars().take(column).map(char::len_utf16).sum())
            .unwrap_or(column);

        Position {
            line: line.saturating_sub(1) as u32,
            character: character as u32,
        }
    }
}

#[cfg(test)]
mod tests {
    use test_fixture::fixture_crate;

    use super::{CargoDiagnosticMapper, CargoDiagnostics};

    #[test]
    fn deduplicates_identical_cargo_diagnostics() {
        let fixture = fixture_crate(
            r#"
            //- /Cargo.toml
            [package]
            name = "diagnostics"
            version = "0.1.0"
            edition = "2024"

            //- /src/lib.rs
            pub fn demo() {}
            "#,
        );
        let message = serde_json::json!({
            "reason": "compiler-message",
            "package_id": "path+file:///diagnostics#0.1.0",
            "target": {
                "kind": ["lib"],
                "crate_types": ["lib"],
                "name": "diagnostics",
                "src_path": fixture.path("src/lib.rs"),
                "edition": "2024",
                "doc": true,
                "doctest": true,
                "test": true
            },
            "message": {
                "rendered": null,
                "children": [],
                "code": null,
                "level": "warning",
                "message": "unused variable",
                "spans": [{
                    "file_name": "src/lib.rs",
                    "byte_start": 7,
                    "byte_end": 11,
                    "line_start": 1,
                    "line_end": 1,
                    "column_start": 8,
                    "column_end": 12,
                    "is_primary": true,
                    "text": [{
                        "text": "pub fn demo() {}",
                        "highlight_start": 8,
                        "highlight_end": 12
                    }],
                    "label": null,
                    "suggested_replacement": null,
                    "suggestion_applicability": null,
                    "expansion": null
                }]
            }
        });
        let message = serde_json::to_vec(&message).expect("compiler message should serialize");

        // Cargo and wrappers can mirror one compiler message to both output streams.
        let diagnostics =
            CargoDiagnostics::parse(&fixture.path(""), "cargo check", &message, &message)
                .into_inner();

        assert_eq!(
            diagnostics.len(),
            1,
            "one source should receive diagnostics"
        );
        let published = diagnostics
            .values()
            .next()
            .expect("source should receive diagnostics");
        assert_eq!(published.len(), 1);
        assert_eq!(published[0].message, "unused variable");
    }

    #[test]
    fn derives_package_root_from_target_src_path() {
        let fixture = fixture_crate(
            r#"
            //- /Cargo.toml
            [workspace]
            members = ["crates/member"]

            //- /crates/member/Cargo.toml
            [package]
            name = "member"
            version = "0.1.0"
            edition = "2024"

            //- /crates/member/src/bin/tool.rs
            fn main() {}
            "#,
        );

        let target_src_path = fixture.path("crates/member/src/bin/tool.rs");
        assert_eq!(
            CargoDiagnosticMapper::package_root_from_target_src_path(&target_src_path),
            Some(fixture.path("crates/member"))
        );
    }
}
