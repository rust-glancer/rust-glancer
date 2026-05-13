use std::{
    cell::RefCell,
    collections::HashSet,
    fmt,
    path::{Path, PathBuf},
    process::Command,
    sync::{Mutex, OnceLock},
};

use rg_body_ir::{BodyIrBuildPolicy, BodyIrDb};
use rg_def_map::DefMapDb;
use rg_item_tree::ItemTreeDb;
use rg_parse::ParseDb;
use rg_semantic_ir::SemanticIrDb;
use rg_text::PackageNameInterners;
use rg_workspace::WorkspaceMetadata;

// Shared support for benchmark binaries.
//
// Each file in `benches/` is compiled as its own tiny crate, so common fixture discovery and
// expensive setup live here. The benchmark entrypoints decide what to time; this module only
// prepares stable workspaces and reusable baseline artifacts.
pub mod query;

// Several benchmark functions can request the same target in one process. `cargo fetch` is slow
// setup work, so run it once per target and keep it outside all measured closures.
static CARGO_FETCH_LOCK: OnceLock<Mutex<HashSet<BenchTarget>>> = OnceLock::new();

/// One workspace fixture that can be used by build-pipeline or query benchmarks.
///
/// Synthetic targets are checked in and generated for specific pipeline stress shapes. The
/// rust-analyzer fixture is intentionally optional because it is large and fetched separately.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BenchTarget {
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

    pub fn manifest_path(self) -> PathBuf {
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

    pub fn project_root(self) -> PathBuf {
        match self {
            Self::SmallApp => workspace_root().join("test_targets/bench_fixtures/small_app"),
            Self::SyntheticParseHeavy => {
                workspace_root().join("test_targets/bench_fixtures/synthetic_parse_heavy")
            }
            Self::SyntheticItemTreeHeavy => {
                workspace_root().join("test_targets/bench_fixtures/synthetic_item_tree_heavy")
            }
            Self::SyntheticDefMapHeavy => {
                workspace_root().join("test_targets/bench_fixtures/synthetic_def_map_heavy")
            }
            Self::SyntheticBodyHeavy => {
                workspace_root().join("test_targets/bench_fixtures/synthetic_body_heavy")
            }
            Self::RustAnalyzer => {
                workspace_root().join("test_targets/bench_fixtures/rust-analyzer")
            }
        }
    }

    pub fn prepare(self) {
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
}

impl fmt::Display for BenchTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Prebuilt inputs and counters for phase-by-phase pipeline benchmarks.
///
/// `analysis_pipeline.rs` measures rebuilding individual phase databases. Building every prior
/// phase inside each measurement would swamp the phase being measured, so this fixture constructs
/// one reusable baseline and stores both the inputs and the counters Divan displays.
pub struct BenchFixture {
    pub workspace: WorkspaceMetadata,
    pub parse: ParseDb,
    pub item_tree: ItemTreeDb,
    pub names_after_item_tree: PackageNameInterners,
    pub names_after_semantic_ir: PackageNameInterners,
    pub def_map: DefMapDb,
    pub semantic_ir: SemanticIrDb,
    pub source_files: usize,
    pub source_bytes: u64,
    pub item_tree_items: usize,
    pub def_map_imports: usize,
    pub semantic_items: usize,
    pub body_expressions: usize,
}

impl BenchFixture {
    pub fn get(target: BenchTarget) -> &'static Self {
        thread_local! {
            static FIXTURES: RefCell<Vec<(BenchTarget, &'static BenchFixture)>> = const {
                RefCell::new(Vec::new())
            };
        }

        FIXTURES.with(|fixtures| {
            if let Some((_, cached)) = fixtures
                .borrow()
                .iter()
                .find(|(cached_target, _)| *cached_target == target)
            {
                return *cached;
            }

            let loaded = Box::leak(Box::new(BenchFixture::load(target)));
            fixtures.borrow_mut().push((target, loaded));

            loaded
        })
    }

    fn load(target: BenchTarget) -> Self {
        target.prepare();

        // The pipeline benchmark starts from the real Cargo workspace metadata for each target,
        // matching the way the CLI/LSP build a project rather than using synthetic in-memory
        // fixtures.
        let workspace = WorkspaceMetadata::from_manifest_path(target.manifest_path())
            .unwrap_or_else(|error| panic!("{target} Cargo metadata should load: {error}"));
        let mut parse = ParseDb::build(&workspace)
            .unwrap_or_else(|error| panic!("{target} parse db should build: {error}"));
        let source_files = count_source_files(&parse);
        let source_bytes = count_source_bytes(&parse);

        // Name interning is shared across phases during a normal build. Keep snapshots of the
        // interner state at phase boundaries so each benchmark can start from the same inputs the
        // real pipeline would have at that point.
        let mut names = PackageNameInterners::new(parse.package_count());
        let item_tree = ItemTreeDb::build_with_interners(&mut parse, &mut names)
            .unwrap_or_else(|error| panic!("{target} item tree should build: {error}"));
        let item_tree_items = count_item_tree_items(&workspace, &item_tree);
        let names_after_item_tree = names.clone();

        let def_map = DefMapDb::builder(&workspace, &parse, &item_tree)
            .name_interners(&mut names)
            .build()
            .unwrap_or_else(|error| panic!("{target} def map should build: {error}"));
        let def_map_imports = def_map.stats().import_count;

        let semantic_ir = SemanticIrDb::builder(&item_tree, &def_map)
            .build()
            .unwrap_or_else(|error| panic!("{target} semantic IR should build: {error}"));
        let semantic_stats = semantic_ir.stats();
        let semantic_items = semantic_stats.struct_count
            + semantic_stats.union_count
            + semantic_stats.enum_count
            + semantic_stats.trait_count
            + semantic_stats.impl_count
            + semantic_stats.function_count
            + semantic_stats.type_alias_count
            + semantic_stats.const_count
            + semantic_stats.static_count;
        let names_after_semantic_ir = names.clone();

        let body_ir = BodyIrDb::builder(&parse, &def_map, &semantic_ir)
            .name_interners(&mut names)
            .policy(BodyIrBuildPolicy::workspace_packages())
            .build()
            .unwrap_or_else(|error| panic!("{target} body IR should build: {error}"));
        let body_expressions = body_ir.stats().expression_count;

        Self {
            workspace,
            parse,
            item_tree,
            names_after_item_tree,
            names_after_semantic_ir,
            def_map,
            semantic_ir,
            source_files,
            source_bytes,
            item_tree_items,
            def_map_imports,
            semantic_items,
            body_expressions,
        }
    }
}

pub fn bench_targets() -> Vec<BenchTarget> {
    match std::env::var("RUST_GLANCER_BENCH_TARGETS") {
        Ok(value) => BenchTarget::parse_list(&value),
        Err(std::env::VarError::NotPresent) => BenchTarget::ALL.to_vec(),
        Err(error) => panic!("failed to read RUST_GLANCER_BENCH_TARGETS: {error}"),
    }
}

fn workspace_root() -> PathBuf {
    // `CARGO_MANIFEST_DIR` points at `crates/engine/project`; benchmark fixtures live at the
    // workspace root under `test_targets/bench_fixtures`.
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn count_source_files(parse: &ParseDb) -> usize {
    parse
        .packages()
        .iter()
        .map(|package| package.parsed_files().count())
        .sum()
}

fn count_source_bytes(parse: &ParseDb) -> u64 {
    parse
        .packages()
        .iter()
        .flat_map(|package| package.parsed_files())
        .filter_map(|file| std::fs::metadata(file.path()).ok())
        .map(|metadata| metadata.len())
        .sum()
}

fn count_item_tree_items(workspace: &WorkspaceMetadata, item_tree: &ItemTreeDb) -> usize {
    (0..workspace.packages().len())
        .filter_map(|package| item_tree.package(package))
        .flat_map(|package| package.files())
        .map(|file| file.items.len())
        .sum()
}
