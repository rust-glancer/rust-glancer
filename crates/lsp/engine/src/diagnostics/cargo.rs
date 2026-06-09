use std::{
    collections::{BTreeMap, BTreeSet},
    io::Cursor,
    path::{Path, PathBuf},
};

use cargo_metadata::{
    Message,
    diagnostic::{Diagnostic as CargoDiagnostic, DiagnosticLevel, DiagnosticSpan},
};
use ls_types::{
    Diagnostic, DiagnosticRelatedInformation, DiagnosticSeverity, Location, NumberOrString,
    Position, Range, Uri,
};
use rg_std::UniqueVec;

#[derive(Debug, Default)]
pub(crate) struct CargoDiagnostics {
    by_path: BTreeMap<PathBuf, UniqueVec<Diagnostic>>,
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

    pub(crate) fn paths(&self) -> BTreeSet<PathBuf> {
        self.by_path.keys().cloned().collect()
    }

    pub(crate) fn into_inner(self) -> BTreeMap<PathBuf, Vec<Diagnostic>> {
        self.by_path
            .into_iter()
            .map(|(path, diagnostics)| (path, diagnostics.into_vec()))
            .collect()
    }

    #[cfg(test)]
    pub(super) fn from_map(by_path: BTreeMap<PathBuf, Vec<Diagnostic>>) -> Self {
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
    path: PathBuf,
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
            let path = self.resolve_span_path(span);
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

        let path = self.resolve_span_path(span);
        let uri = Uri::from_file_path(path)?;
        Some(DiagnosticRelatedInformation {
            location: Location {
                uri,
                range: Self::range(span),
            },
            message,
        })
    }

    fn resolve_span_path(&self, span: &DiagnosticSpan) -> PathBuf {
        let raw = PathBuf::from(&span.file_name);
        if raw.is_absolute() {
            return canonicalized(raw);
        }

        // Cargo usually reports span files relative to the command working directory. If a
        // toolchain reports package-relative paths instead, the compiler message target root is
        // enough to derive the package root without running cargo metadata twice.
        let workspace_candidate = self.workspace_root.join(&raw);
        if workspace_candidate.exists() {
            return canonicalized(workspace_candidate);
        }

        let package_candidate = self.package_root.as_ref().map(|root| root.join(&raw));
        if let Some(candidate) = package_candidate.as_ref()
            && candidate.exists()
        {
            return canonicalized(candidate.clone());
        }

        package_candidate.unwrap_or(workspace_candidate)
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

fn canonicalized(path: PathBuf) -> PathBuf {
    path.canonicalize().unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use ls_types::{Diagnostic, Position, Range};
    use rg_std::UniqueVec;

    use super::CargoDiagnosticMapper;

    #[test]
    fn deduplicates_identical_cargo_diagnostics() {
        let diagnostic = Diagnostic {
            range: Range::new(Position::new(0, 0), Position::new(0, 3)),
            severity: None,
            code: None,
            code_description: None,
            source: Some("cargo check".to_string()),
            message: "unused variable".to_string(),
            related_information: None,
            tags: None,
            data: None,
        };
        let mut diagnostics = UniqueVec::new();

        diagnostics.push(diagnostic.clone());
        diagnostics.push(diagnostic);

        assert_eq!(diagnostics.len(), 1);
    }

    #[test]
    fn derives_package_root_from_target_src_path() {
        let temp_root = std::env::temp_dir().join(format!(
            "rust-glancer-check-diagnostics-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time should be after Unix epoch")
                .as_nanos()
        ));
        let package_root = temp_root.join("crates/member");
        let src_dir = package_root.join("src/bin");
        fs::create_dir_all(&src_dir).expect("test package directory should be created");
        fs::write(
            package_root.join("Cargo.toml"),
            "[package]\nname = \"member\"\n",
        )
        .expect("test manifest should be written");

        let target_src_path = src_dir.join("tool.rs");
        assert_eq!(
            CargoDiagnosticMapper::package_root_from_target_src_path(&target_src_path),
            Some(package_root)
        );

        fs::remove_dir_all(temp_root).expect("test package directory should be removed");
    }
}
