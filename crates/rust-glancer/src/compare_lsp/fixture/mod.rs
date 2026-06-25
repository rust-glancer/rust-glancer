//! Fixture roots and query-vector selection.
//!
//! A fixture is more than a directory: it fixes the project root, the query cases that make sense
//! for that project, and the setup errors users should see before any LSP process is spawned.
//! Keeping this validation ahead of the client layer makes benchmark failures about protocol
//! or server behavior, not missing checkout files.

use std::path::{Path, PathBuf};

use anyhow::Context as _;

use crate::compare_lsp::{
    CliFixture,
    query::{self, QueryCase, QueryTarget},
};

/// Prepared benchmark fixture used by the LSP comparison pipeline.
///
/// Example: `rust_analyzer` resolves to the pinned checkout under `test_targets/bench_fixtures`
/// unless the user passes `--path`, then carries the static query vector for that checkout.
#[derive(Debug)]
pub(crate) struct Fixture {
    kind: CliFixture,
    root: PathBuf,
    query_cases: &'static [QueryCase],
}

impl Fixture {
    /// Resolve user input into a validated fixture before any server process starts.
    pub(crate) fn resolve(
        kind: CliFixture,
        path_override: Option<PathBuf>,
    ) -> anyhow::Result<Self> {
        let uses_default_root = path_override.is_none();
        let root = path_override.unwrap_or_else(|| Self::default_root(kind));

        Self::validate_root(kind, &root, uses_default_root)?;
        let query_cases = Self::query_cases_for(kind);
        Self::validate_query_files(&root, query_cases)?;

        Ok(Self {
            kind,
            root,
            query_cases,
        })
    }

    pub(crate) fn kind(&self) -> CliFixture {
        self.kind
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn query_cases(&self) -> &'static [QueryCase] {
        self.query_cases
    }

    fn default_root(kind: CliFixture) -> PathBuf {
        match kind {
            CliFixture::RustAnalyzer => {
                workspace_root().join("test_targets/bench_fixtures/rust-analyzer")
            }
        }
    }

    fn query_cases_for(kind: CliFixture) -> &'static [QueryCase] {
        match kind {
            CliFixture::RustAnalyzer => query::rust_analyzer_cases(),
        }
    }

    fn validate_root(kind: CliFixture, root: &Path, uses_default_root: bool) -> anyhow::Result<()> {
        if !root.exists() {
            if uses_default_root && kind == CliFixture::RustAnalyzer {
                anyhow::bail!(
                    "rust-analyzer LSP comparison fixture is missing at {}.\n\
                     Run ./test_targets/bench_fixtures/fetch-rust-analyzer.sh, \
                     or pass --path <root> to use another checkout.",
                    root.display(),
                );
            }

            anyhow::bail!(
                "LSP comparison fixture root {} does not exist",
                root.display()
            );
        }
        if !root.is_dir() {
            anyhow::bail!(
                "LSP comparison fixture root {} is not a directory",
                root.display()
            );
        }

        let manifest = root.join("Cargo.toml");
        if !manifest.is_file() {
            anyhow::bail!(
                "LSP comparison fixture root {} does not contain Cargo.toml",
                root.display(),
            );
        }

        Ok(())
    }

    /// Check that hardcoded vector entries still point at real source positions.
    fn validate_query_files(root: &Path, query_cases: &[QueryCase]) -> anyhow::Result<()> {
        for query in query_cases {
            let Some(source_path) = query.source_path() else {
                continue;
            };
            let path = root.join(source_path);
            if !path.is_file() {
                anyhow::bail!(
                    "LSP comparison query `{}` points to missing file {}",
                    query.label(),
                    path.display(),
                );
            }

            let source = std::fs::read_to_string(&path).with_context(|| {
                format!(
                    "Reading LSP comparison query file {} failed",
                    path.display()
                )
            })?;
            let position = match query.target() {
                QueryTarget::Position { position, .. } | QueryTarget::Rename { position, .. } => {
                    position
                }
                QueryTarget::File { .. } | QueryTarget::Workspace { .. } => continue,
            };

            // LSP speaks UTF-16 positions, so validate the same coordinate space the eventual
            // request payload will use.
            let Some(line) = source.lines().nth(position.line() as usize) else {
                anyhow::bail!(
                    "LSP comparison query `{}` points past the end of {} at line {}",
                    query.label(),
                    path.display(),
                    position.line(),
                );
            };
            if line.encode_utf16().count() < position.character() as usize {
                anyhow::bail!(
                    "LSP comparison query `{}` points past line {} in {} at character {}",
                    query.label(),
                    position.line(),
                    path.display(),
                    position.character(),
                );
            }
        }

        Ok(())
    }
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}
