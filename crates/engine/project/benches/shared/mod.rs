mod pipeline;

use std::{
    collections::HashSet,
    fmt,
    path::{Path, PathBuf},
    process::Command,
    sync::{Mutex, OnceLock},
};

use rg_workspace::{CargoMetadataConfig, WorkspaceLoweringConfig, WorkspaceMetadata};

pub(crate) use self::pipeline::BenchWorkspace;

static CARGO_FETCH_LOCK: OnceLock<Mutex<HashSet<BenchTarget>>> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum BenchTarget {
    SmallApp,
    SyntheticParseHeavy,
    SyntheticItemTreeHeavy,
    SyntheticDefMapHeavy,
    SyntheticBodyHeavy,
    RustAnalyzer,
}

impl BenchTarget {
    const ALL: [Self; 6] = [
        Self::SmallApp,
        Self::SyntheticParseHeavy,
        Self::SyntheticItemTreeHeavy,
        Self::SyntheticDefMapHeavy,
        Self::SyntheticBodyHeavy,
        Self::RustAnalyzer,
    ];

    pub(crate) fn workspace(self) -> WorkspaceMetadata {
        self.prepare();
        let loaded = CargoMetadataConfig::default()
            .load_metadata_with_target_cfg(self.manifest_path())
            .unwrap_or_else(|error| panic!("{self} Cargo metadata should load: {error}"));
        WorkspaceMetadata::lower(
            loaded.metadata,
            loaded.target_cfg,
            WorkspaceLoweringConfig::default(),
        )
        .unwrap_or_else(|error| panic!("{self} workspace metadata should lower: {error}"))
    }

    fn manifest_path(self) -> PathBuf {
        self.project_root().join("Cargo.toml")
    }

    fn name(self) -> &'static str {
        match self {
            Self::SmallApp => "small_app",
            Self::SyntheticParseHeavy => "synthetic_parse_heavy",
            Self::SyntheticItemTreeHeavy => "synthetic_item_tree_heavy",
            Self::SyntheticDefMapHeavy => "synthetic_def_map_heavy",
            Self::SyntheticBodyHeavy => "synthetic_body_heavy",
            Self::RustAnalyzer => "rust_analyzer",
        }
    }

    fn project_root(self) -> PathBuf {
        match self {
            Self::SmallApp => Self::workspace_root().join("test_targets/bench_fixtures/small_app"),
            Self::SyntheticParseHeavy => {
                Self::workspace_root().join("test_targets/bench_fixtures/synthetic_parse_heavy")
            }
            Self::SyntheticItemTreeHeavy => {
                Self::workspace_root().join("test_targets/bench_fixtures/synthetic_item_tree_heavy")
            }
            Self::SyntheticDefMapHeavy => {
                Self::workspace_root().join("test_targets/bench_fixtures/synthetic_def_map_heavy")
            }
            Self::SyntheticBodyHeavy => {
                Self::workspace_root().join("test_targets/bench_fixtures/synthetic_body_heavy")
            }
            Self::RustAnalyzer => {
                Self::workspace_root().join("test_targets/bench_fixtures/rust-analyzer")
            }
        }
    }

    fn prepare(self) {
        self.ensure_project_exists();
        self.fetch_dependencies();
    }

    fn parse_list(value: &str) -> Vec<Self> {
        let mut targets = Vec::new();

        for raw_target in value.split(',') {
            let target = raw_target.trim();
            if target.is_empty() {
                continue;
            }

            let target = match target {
                "small_app" => Self::SmallApp,
                "synthetic_parse_heavy" => Self::SyntheticParseHeavy,
                "synthetic_item_tree_heavy" => Self::SyntheticItemTreeHeavy,
                "synthetic_def_map_heavy" => Self::SyntheticDefMapHeavy,
                "synthetic_body_heavy" => Self::SyntheticBodyHeavy,
                "rust_analyzer" => Self::RustAnalyzer,
                _ => panic!(
                    "unknown benchmark target '{target}'; expected one of: small_app, synthetic_parse_heavy, synthetic_item_tree_heavy, synthetic_def_map_heavy, synthetic_body_heavy, rust_analyzer"
                ),
            };

            if !targets.contains(&target) {
                targets.push(target);
            }
        }

        assert!(
            !targets.is_empty(),
            "RUST_GLANCER_BENCH_TARGETS must select at least one benchmark target"
        );

        targets
    }

    fn ensure_project_exists(self) {
        let manifest_path = self.manifest_path();

        match self {
            Self::SmallApp
            | Self::SyntheticParseHeavy
            | Self::SyntheticItemTreeHeavy
            | Self::SyntheticDefMapHeavy
            | Self::SyntheticBodyHeavy => assert!(
                manifest_path.exists(),
                "benchmark target {} is missing at {}",
                self,
                manifest_path.display(),
            ),
            Self::RustAnalyzer => assert!(
                manifest_path.exists(),
                "rust-analyzer benchmark fixture is missing at {}.\n\
                 Run ./test_targets/bench_fixtures/fetch-rust-analyzer.sh, or set \
                 RUST_GLANCER_BENCH_TARGETS=synthetic_body_heavy to run only a checked-in synthetic target.",
                self.project_root().display(),
            ),
        }
    }

    fn fetch_dependencies(self) {
        let lock = CARGO_FETCH_LOCK.get_or_init(|| Mutex::new(HashSet::new()));
        let mut fetched_targets = lock
            .lock()
            .expect("cargo fetch target set should not be poisoned");

        if fetched_targets.contains(&self) {
            return;
        }

        // Analysis reads dependency source files from Cargo's local checkout. Fetching is setup,
        // not benchmarked work, so keep it outside Divan timing and quiet in Divan output.
        let project_root = self.project_root();
        let status = Command::new("cargo")
            .arg("fetch")
            .arg("--locked")
            .arg("--quiet")
            .current_dir(&project_root)
            .status()
            .unwrap_or_else(|error| {
                panic!(
                    "failed to run cargo fetch for {} fixture at {}: {error}",
                    self,
                    project_root.display(),
                )
            });

        assert!(
            status.success(),
            "cargo fetch failed for {} fixture at {} with status {status}",
            self,
            project_root.display(),
        );

        fetched_targets.insert(self);
    }

    fn workspace_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
    }
}

impl fmt::Display for BenchTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

pub(crate) fn bench_targets() -> Vec<BenchTarget> {
    match std::env::var("RUST_GLANCER_BENCH_TARGETS") {
        Ok(value) => BenchTarget::parse_list(&value),
        Err(std::env::VarError::NotPresent) => BenchTarget::ALL.to_vec(),
        Err(error) => panic!("failed to read RUST_GLANCER_BENCH_TARGETS: {error}"),
    }
}
