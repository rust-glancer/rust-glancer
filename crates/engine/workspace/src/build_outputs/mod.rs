//! Passive recovery of generated Rust sources from builds the user already ran.
//!
//! Build-script output is not part of Cargo metadata, and rust-glancer deliberately does not run
//! Cargo or project code to obtain it. Instead, this module looks for evidence left by a completed
//! rustc unit. A dep-info rule can contain both the package target root and a generated input:
//!
//! ```text
//! target/debug/deps/demo-abc.d:
//!     /workspace/demo/src/lib.rs
//!     /workspace/target/debug/build/demo-def/out/bindings.rs
//! ```
//!
//! The exact target root attributes that historical unit to a package. A dependency path below a
//! Cargo `build/<unit>/out` directory proves that rustc actually read the generated file. Retained
//! build-script stdout beside that directory then supplies useful `cargo::rustc-cfg` and
//! `cargo::rustc-env` values. Together they are enough to index common source such as
//! `include!(concat!(env!("OUT_DIR"), "/bindings.rs"))` without executing anything.
//!
//! This is intentionally approximate, not a reimplementation of Cargo freshness or feature
//! selection. Cargo's reported build directory wins when it contains a usable unit; otherwise the
//! target-directory fallbacks participate. Within one root, one deterministic recent candidate is
//! chosen. Missing files and unfamiliar layouts are ignored; if no usable candidate can be
//! established, the package remains on ordinary source-only analysis.

mod dep_info;
mod instructions;

use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant, UNIX_EPOCH},
};

use rg_cfg_eval::CfgOptions;
use rg_std::{ExpectedUnique, MemoryRecorder, MemorySize, UniqueVec};

use crate::Package;

const MAX_DEPS_DIRECTORIES: usize = 128;
const MAX_DIRECTORY_ENTRIES: usize = 100_000;
const MAX_BUILD_OUTPUT_CANDIDATES_PER_TARGET_NAME: usize = 64;
const MAX_GENERATED_FILES_PER_BUILD_OUTPUT: usize = 1_024;

/// Bounded discovery totals retained for startup diagnostics and profiling.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, MemorySize)]
#[memsize(leaf)]
pub struct CargoBuildOutputScanStats {
    target_directories: usize,
    deps_directories: usize,
    dep_info_files: usize,
    build_script_packages: usize,
    matched_rustc_units: usize,
    build_output_candidates: usize,
    selected_packages: usize,
    generated_files: usize,
    generated_bytes: u64,
    scan_duration: Duration,
}

impl CargoBuildOutputScanStats {
    pub fn target_directories(self) -> usize {
        self.target_directories
    }

    pub fn deps_directories(self) -> usize {
        self.deps_directories
    }

    pub fn dep_info_files(self) -> usize {
        self.dep_info_files
    }

    pub fn matched_rustc_units(self) -> usize {
        self.matched_rustc_units
    }

    pub fn build_script_packages(self) -> usize {
        self.build_script_packages
    }

    pub fn build_output_candidates(self) -> usize {
        self.build_output_candidates
    }

    pub fn selected_packages(self) -> usize {
        self.selected_packages
    }

    pub fn generated_files(self) -> usize {
        self.generated_files
    }

    pub fn generated_bytes(self) -> u64 {
        self.generated_bytes
    }

    pub fn scan_duration(self) -> Duration {
        self.scan_duration
    }
}

/// Generated source paths and compile-time environment recovered from one concrete rustc unit.
///
/// This is evidence, not a promise that Cargo would select the same unit for another invocation.
/// The files remain useful for indexing as long as they exist and form a consistent source
/// snapshot. For example, the retained `OUT_DIR` value can render an `include!` path, while the
/// generated-file list proves that the selected unit really consumed the rendered file.
///
/// The recovered sources belong to one [`crate::WorkspaceMetadata`] snapshot. Ordinary saved-file
/// rebuilds reuse them without rescanning Cargo directories; loading a new workspace snapshot
/// performs a new passive scan and may select a different unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CargoGeneratedSources(Arc<CargoGeneratedSourcesData>);

#[derive(Debug, PartialEq, Eq, MemorySize)]
struct CargoGeneratedSourcesData {
    out_dir: PathBuf,
    compile_env: Vec<CargoCompileEnvVar>,
    generated_files: Vec<PathBuf>,
}

impl CargoGeneratedSources {
    fn new(
        out_dir: PathBuf,
        compile_env: Vec<CargoCompileEnvVar>,
        generated_files: Vec<PathBuf>,
    ) -> Self {
        Self(Arc::new(CargoGeneratedSourcesData {
            out_dir,
            compile_env,
            generated_files,
        }))
    }

    /// Returns the concrete output directory used by the selected compilation.
    pub fn out_dir(&self) -> &Path {
        &self.0.out_dir
    }

    /// Returns a captured compile-time environment value without consulting the LSP process.
    pub fn compile_env_value(&self, name: &str) -> Option<&str> {
        self.0
            .compile_env
            .iter()
            .find(|entry| entry.name == name)
            .map(|entry| entry.value.as_str())
    }

    /// Returns generated files rustc recorded for the selected compilation.
    pub fn generated_files(&self) -> &[PathBuf] {
        &self.0.generated_files
    }

    /// Resolves one path relative to the selected `OUT_DIR` only when dep-info named it.
    ///
    /// Requiring a unique concrete file prevents an approximate feature selection from silently
    /// choosing between several historical outputs with the same suffix.
    pub fn generated_file_for_out_dir_suffix(&self, suffix: &Path) -> Option<&Path> {
        let suffix = suffix.strip_prefix(Path::new("/")).unwrap_or(suffix);
        if suffix
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            return None;
        }

        let mut matches = ExpectedUnique::new();
        for path in &self.0.generated_files {
            if path
                .strip_prefix(&self.0.out_dir)
                .is_ok_and(|relative| relative == suffix)
            {
                matches.push(path.as_path());
            }
        }
        matches.into_option()
    }
}

impl MemorySize for CargoGeneratedSources {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        if recorder.visit_shared_allocation(Arc::as_ptr(&self.0).cast::<()>()) {
            self.0.record_memory_children(recorder);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, MemorySize)]
struct CargoCompileEnvVar {
    name: String,
    value: String,
}

/// Finds and applies one best passive build-output candidate per normalized package.
///
/// Discovery is bounded and best-effort. It scans likely `deps` directories, attributes dep-info
/// files by exact target root, extracts generated inputs under Cargo build output directories, and
/// enriches each candidate with retained build-script cfg/environment instructions. Applying the
/// selected candidate extends the package cfg facts and retains its generated files and compile
/// environment for later `include!` resolution.
pub(crate) struct CargoBuildOutputDiscovery;

impl CargoBuildOutputDiscovery {
    pub(crate) fn apply(
        workspace_root: &Path,
        cargo_build_dir: Option<&Path>,
        cargo_target_dir: &Path,
        packages: &mut [Package],
    ) -> CargoBuildOutputScanStats {
        let started = Instant::now();
        let mut stats = CargoBuildOutputScanStats {
            build_script_packages: packages
                .iter()
                .filter(|package| {
                    package
                        .targets
                        .iter()
                        .any(|target| target.kind.is_custom_build())
                })
                .count(),
            ..Default::default()
        };
        let target_directories =
            Self::target_directories(workspace_root, cargo_build_dir, cargo_target_dir);
        stats.target_directories = target_directories.len();
        let deps_directories = target_directories
            .iter()
            .flat_map(|root| Self::deps_directories(root))
            .map(|path| fs::canonicalize(&path).unwrap_or(path))
            .collect::<UniqueVec<_>>()
            .into_vec()
            .into_iter()
            .take(MAX_DEPS_DIRECTORIES)
            .map(|path| {
                let target_directory_rank = target_directories
                    .iter()
                    .position(|root| path.starts_with(root))
                    .expect("discovered deps directory should belong to one scan root");
                DepsDirectory {
                    path,
                    target_directory_rank,
                }
            })
            .collect::<Vec<_>>();
        stats.deps_directories = deps_directories.len();
        if deps_directories.is_empty() {
            stats.scan_duration = started.elapsed();
            return stats;
        }

        // Rustc output filenames identify a Cargo target only by normalized name. Keep every
        // possible owner here; the exact target root inside dep-info makes later attribution
        // reliable.
        let package_target_roots = Self::package_target_roots(packages);
        let dep_info_files =
            Self::matching_dep_info_files(&deps_directories, &package_target_roots);
        stats.dep_info_files = dep_info_files.values().map(Vec::len).sum();
        let mut build_output_candidates = vec![Vec::new(); packages.len()];

        for (artifact_name, files) in dep_info_files {
            let Some(target_roots) = package_target_roots.get(&artifact_name) else {
                continue;
            };

            for file in files
                .into_iter()
                .take(MAX_BUILD_OUTPUT_CANDIDATES_PER_TARGET_NAME)
            {
                let Some(rustc_input_paths) = dep_info::rustc_input_paths(&file.path) else {
                    continue;
                };
                let rustc_input_paths = rustc_input_paths
                    .into_iter()
                    // Rustc normally writes absolute dependency paths. Resolving an unfamiliar
                    // relative spelling against the LSP process directory could attribute an
                    // unrelated file by accident, so leave it unsupported until Cargo supplies a
                    // trustworthy base for it.
                    .filter(|path| path.is_absolute())
                    .filter_map(|path| fs::canonicalize(path).ok())
                    .collect::<Vec<_>>();
                // A name match is only a cheap scan filter. The rustc unit belongs to a package
                // only when its dependency list contains that package's exact target entrypoint.
                let matching_packages = target_roots
                    .iter()
                    .filter(|target| {
                        rustc_input_paths
                            .iter()
                            .any(|path| path == &target.target_root)
                    })
                    .map(|target| target.package_slot)
                    .collect::<UniqueVec<_>>();
                if matching_packages.is_empty() {
                    continue;
                }
                stats.matched_rustc_units += 1;

                // One rustc unit should consume one build-script output directory. Grouping keeps
                // an unfamiliar mixed record deterministic instead of combining unrelated roots.
                let mut generated_by_out_dir = BTreeMap::<PathBuf, Vec<PathBuf>>::new();
                for input_path in rustc_input_paths {
                    let Some(out_dir) =
                        Self::build_script_out_dir(&input_path, target_directories.as_slice())
                    else {
                        continue;
                    };
                    let out_dir = fs::canonicalize(&out_dir).unwrap_or(out_dir);
                    let generated_files = generated_by_out_dir.entry(out_dir).or_default();
                    if generated_files.len() < MAX_GENERATED_FILES_PER_BUILD_OUTPUT {
                        generated_files.push(input_path);
                    }
                }

                for (out_dir, mut generated_files) in generated_by_out_dir {
                    generated_files.sort();
                    generated_files.dedup();
                    if generated_files.is_empty() {
                        continue;
                    }

                    // Dep-info is the source allow-list. Retained build-script output adds cfg and
                    // environment, but it is never trusted to invent a generated file by itself.
                    let instructions = instructions::BuildScriptInstructions::read(&out_dir);
                    let cfg_options = instructions.cfg_options.clone();
                    let generated_sources = CargoGeneratedSources::new(
                        out_dir.clone(),
                        instructions.compile_env_with_out_dir(&out_dir),
                        generated_files,
                    );
                    let candidate = BuildOutputCandidate {
                        generated_sources,
                        cfg_options,
                        modified: file.modified,
                        dep_info_path: file.path.clone(),
                        target_directory_rank: file.target_directory_rank,
                    };
                    stats.build_output_candidates += 1;
                    for &package_slot in &matching_packages {
                        build_output_candidates[package_slot].push(candidate.clone());
                    }
                }
            }
        }

        // Cargo's reported build directory is authoritative when it contains a usable candidate;
        // the target-directory roots are historical fallbacks. Within one root, prefer the newest
        // matching rustc unit and use the path as a stable tie-breaker. This does not recreate
        // Cargo's active feature set; it chooses one useful, internally consistent source snapshot.
        for (package, mut candidates) in packages.iter_mut().zip(build_output_candidates) {
            candidates.sort_by(|left, right| {
                left.target_directory_rank
                    .cmp(&right.target_directory_rank)
                    .then_with(|| right.modified.cmp(&left.modified))
                    .then_with(|| left.dep_info_path.cmp(&right.dep_info_path))
            });
            let Some(selected) = candidates.into_iter().next() else {
                continue;
            };

            stats.selected_packages += 1;
            stats.generated_files += selected.generated_sources.generated_files().len();
            stats.generated_bytes += selected
                .generated_sources
                .generated_files()
                .iter()
                .filter_map(|path| fs::metadata(path).ok())
                .map(|metadata| metadata.len())
                .sum::<u64>();

            for atom in selected.cfg_options.atoms() {
                package.cfg_options.insert_atom(atom);
            }
            for key_value in selected.cfg_options.key_values() {
                package
                    .cfg_options
                    .insert_key_value(key_value.key(), key_value.value());
            }
            package.cargo_generated_sources = Some(selected.generated_sources);
        }

        stats.scan_duration = started.elapsed();
        stats
    }

    fn target_directories(
        workspace_root: &Path,
        cargo_build_dir: Option<&Path>,
        cargo_target_dir: &Path,
    ) -> UniqueVec<PathBuf> {
        let mut target_directories = UniqueVec::with_capacity(3);
        for root in cargo_build_dir.into_iter().map(Path::to_path_buf).chain([
            cargo_target_dir.to_path_buf(),
            workspace_root.join("target"),
        ]) {
            let root = fs::canonicalize(&root).unwrap_or(root);
            if root.is_dir() {
                target_directories.push(root);
            }
        }
        target_directories
    }

    /// Finds `profile/deps` and `target-triple/profile/deps` without descending into artifact
    /// trees. Cargo's build-directory layout is private, so unfamiliar shapes are simply ignored.
    fn deps_directories(target_directory: &Path) -> Vec<PathBuf> {
        let mut directories = Vec::new();
        let Ok(first_level) = fs::read_dir(target_directory) else {
            return directories;
        };

        for first in first_level.flatten().take(MAX_DIRECTORY_ENTRIES) {
            let Ok(file_type) = first.file_type() else {
                continue;
            };
            if !file_type.is_dir() {
                continue;
            }
            let first_path = first.path();
            let deps = first_path.join("deps");
            if deps.is_dir() {
                directories.push(deps);
            }

            let Ok(second_level) = fs::read_dir(&first_path) else {
                continue;
            };
            for second in second_level.flatten().take(MAX_DIRECTORY_ENTRIES) {
                let Ok(file_type) = second.file_type() else {
                    continue;
                };
                if !file_type.is_dir() {
                    continue;
                }
                let deps = second.path().join("deps");
                if deps.is_dir() {
                    directories.push(deps);
                }
                if directories.len() >= MAX_DEPS_DIRECTORIES {
                    return directories;
                }
            }
        }
        directories.sort();
        directories.dedup();
        directories
    }

    fn package_target_roots(packages: &[Package]) -> HashMap<String, Vec<PackageTargetRoot>> {
        let mut target_roots = HashMap::<String, Vec<PackageTargetRoot>>::new();
        for (package_slot, package) in packages.iter().enumerate() {
            if !package
                .targets
                .iter()
                .any(|target| target.kind.is_custom_build())
            {
                continue;
            }
            for target in &package.targets {
                if target.kind.is_custom_build() {
                    continue;
                }
                target_roots
                    .entry(Self::rustc_artifact_name(&target.name))
                    .or_default()
                    .push(PackageTargetRoot {
                        package_slot,
                        target_root: target.src_path.clone(),
                    });
            }
        }
        target_roots
    }

    fn matching_dep_info_files(
        deps_directories: &[DepsDirectory],
        target_roots: &HashMap<String, Vec<PackageTargetRoot>>,
    ) -> HashMap<String, Vec<DepInfoFile>> {
        let mut files = HashMap::<String, Vec<DepInfoFile>>::new();
        for deps_directory in deps_directories {
            let Ok(entries) = fs::read_dir(&deps_directory.path) else {
                continue;
            };
            for entry in entries.flatten().take(MAX_DIRECTORY_ENTRIES) {
                let path = entry.path();
                if path.extension().and_then(|extension| extension.to_str()) != Some("d") {
                    continue;
                }
                let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
                    continue;
                };
                let artifact_name = stem
                    .rsplit_once('-')
                    .filter(|(_, suffix)| {
                        !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
                    })
                    .map(|(name, _)| name)
                    .unwrap_or(stem);
                if !target_roots.contains_key(artifact_name) {
                    continue;
                }
                let modified = entry
                    .metadata()
                    .and_then(|metadata| metadata.modified())
                    .ok()
                    .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
                    .map(|duration| duration.as_nanos())
                    .unwrap_or_default();
                files
                    .entry(artifact_name.to_string())
                    .or_default()
                    .push(DepInfoFile {
                        path,
                        modified,
                        target_directory_rank: deps_directory.target_directory_rank,
                    });
            }
        }

        for files in files.values_mut() {
            files.sort_by(|left, right| {
                left.target_directory_rank
                    .cmp(&right.target_directory_rank)
                    .then_with(|| right.modified.cmp(&left.modified))
                    .then_with(|| left.path.cmp(&right.path))
            });
            files.truncate(MAX_BUILD_OUTPUT_CANDIDATES_PER_TARGET_NAME);
        }
        files
    }

    fn rustc_artifact_name(name: &str) -> String {
        name.replace('-', "_")
    }

    fn build_script_out_dir(source_path: &Path, target_directories: &[PathBuf]) -> Option<PathBuf> {
        if !target_directories
            .iter()
            .any(|root| source_path.starts_with(root))
        {
            return None;
        }

        source_path.ancestors().find_map(|ancestor| {
            (ancestor.file_name()?.to_str()? == "out"
                && ancestor.parent()?.parent()?.file_name()?.to_str()? == "build")
                .then(|| ancestor.to_path_buf())
        })
    }
}

#[derive(Debug, Clone)]
struct BuildOutputCandidate {
    generated_sources: CargoGeneratedSources,
    cfg_options: CfgOptions,
    modified: u128,
    dep_info_path: PathBuf,
    target_directory_rank: usize,
}

struct DepsDirectory {
    path: PathBuf,
    target_directory_rank: usize,
}

struct PackageTargetRoot {
    package_slot: usize,
    target_root: PathBuf,
}

struct DepInfoFile {
    path: PathBuf,
    modified: u128,
    target_directory_rank: usize,
}

#[cfg(test)]
mod tests;
