//! Passive recovery of generated Rust sources from builds the user already ran.
//!
//! Build-script output is not part of Cargo metadata, and rust-glancer deliberately does not run
//! Cargo or project code to obtain it. Instead, this module looks for evidence left by a completed
//! rustc unit. A dep-info rule can contain both the package target root and a generated input:
//!
//! ```text
//! target/debug/build/demo/<compile-hash>/out/demo.d:
//!     demo/src/lib.rs
//!     /workspace/target/debug/build/demo/<run-hash>/out/bindings.rs
//! ```
//!
//! The exact target root attributes that historical unit to a package. A dependency path below a
//! build script's output directory proves that rustc actually read the generated file. Retained
//! build-script stdout for that run then supplies useful `cargo::rustc-cfg` and
//! `cargo::rustc-env` values. Together they are enough to index common source such as
//! `include!(concat!(env!("OUT_DIR"), "/bindings.rs"))` without executing anything.
//!
//! This is intentionally approximate, not a reimplementation of Cargo freshness or feature
//! selection. Cargo's reported build directory wins when it contains a usable unit; otherwise the
//! target-directory fallbacks participate. Within one root, one deterministic recent output
//! directory is chosen, and generated inputs from package targets that used that same directory
//! are combined. Missing files and unfamiliar layouts are ignored; if no usable candidate can be
//! established, the package remains on ordinary source-only analysis.

mod dep_info;
mod instructions;
mod layout;

use std::{
    cell::Cell,
    cmp::Reverse,
    collections::{BTreeMap, HashMap, HashSet},
    fs, iter,
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant, UNIX_EPOCH},
};

use rg_cfg_eval::CfgOptions;
use rg_std::{ExpectedUnique, MemoryRecorder, MemorySize, NativeOsString, UniqueVec};

use self::{instructions::BuildScriptInstructions, layout::BuildScriptRun};
use crate::Package;

const MAX_DIRECTORY_ENTRIES: usize = 100_000;
const MAX_BUILD_OUTPUT_CANDIDATES_PER_TARGET_NAME: usize = 64;
const MAX_GENERATED_FILES_PER_BUILD_OUTPUT: usize = 1_024;

/// Bounded discovery totals retained for startup diagnostics and profiling.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, MemorySize)]
#[memsize(leaf)]
pub struct CargoBuildOutputScanStats {
    target_directories: usize,
    dep_info_directories: usize,
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
    /// Counts candidate roots, including a separate Cargo build directory when configured.
    pub fn target_directories(self) -> usize {
        self.target_directories
    }

    pub fn dep_info_directories(self) -> usize {
        self.dep_info_directories
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

/// Generated source paths and compile-time environment recovered from one Cargo output directory.
///
/// This is evidence, not a promise that Cargo would select the same unit for another invocation.
/// The files remain useful for indexing as long as they exist and form a consistent source
/// snapshot. For example, the retained `OUT_DIR` value can render an `include!` path, while the
/// generated-file list proves that at least one attributed package target consumed the rendered
/// file.
///
/// The recovered sources belong to one [`crate::WorkspaceMetadata`] snapshot. Ordinary saved-file
/// rebuilds reuse them without rescanning Cargo directories; loading a new workspace snapshot
/// performs a new passive scan and may select a different unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CargoGeneratedSources(Arc<CargoGeneratedSourcesData>);

impl CargoGeneratedSources {
    pub(crate) fn stamp(&self) -> ajika::Result<ajika::Stamp> {
        let native = |path: &Path| NativeOsString::from_os_str(path.as_os_str());
        let out_dir = native(&self.0.out_dir);
        let compile_env: Vec<_> = self
            .0
            .compile_env
            .iter()
            .map(|entry| (entry.name.clone(), entry.value.clone()))
            .collect();
        let generated_files: Vec<_> = self
            .0
            .generated_files
            .iter()
            .map(|path| native(path))
            .collect();
        ajika::Stamp::of_value(&(out_dir, compile_env, generated_files))
    }

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
            .any(|component| matches!(component, Component::ParentDir))
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
            // Delegate through `Arc`, not its dereferenced data, so the allocation payload and
            // overhead are counted together with the data's owned buffers.
            MemorySize::record_memory_children(&self.0, recorder);
        }
    }
}

#[derive(Debug, PartialEq, Eq, MemorySize)]
struct CargoGeneratedSourcesData {
    out_dir: PathBuf,
    compile_env: Vec<CargoCompileEnvVar>,
    generated_files: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, MemorySize)]
struct CargoCompileEnvVar {
    name: String,
    value: String,
}

/// Finds and applies one best passive build-output candidate per normalized package.
///
/// Discovery uses rustc dep-info to match a target to its generated inputs, then reads the
/// build-script instructions for those inputs. The selected run supplies cfg values and the
/// compile-time environment for `include!` resolution.
pub(crate) struct CargoBuildOutputDiscovery;

impl CargoBuildOutputDiscovery {
    pub(crate) fn apply(
        workspace_root: &Path,
        cargo_build_dir: Option<&Path>,
        cargo_target_dir: &Path,
        packages: &mut [Package],
    ) -> CargoBuildOutputScanStats {
        let started = Instant::now();
        let mut stats = CargoBuildOutputScanStats::default();
        let scan_roots = Self::scan_roots(workspace_root, cargo_build_dir, cargo_target_dir);
        stats.target_directories = scan_roots.len();

        // Rustc output filenames identify a Cargo target only by normalized name. Keep every
        // possible owner here; the exact target root inside dep-info makes later attribution
        // reliable.
        let mut package_names = HashSet::new();
        let mut package_target_roots = HashMap::<String, Vec<PackageTargetRoot<'_>>>::new();
        for (package_slot, package) in packages.iter().enumerate() {
            if !package
                .targets
                .iter()
                .any(|target| target.kind.is_custom_build())
            {
                continue;
            }
            stats.build_script_packages += 1;
            package_names.insert(package.name.as_str());
            for target in package
                .targets
                .iter()
                .filter(|target| !target.kind.is_custom_build())
            {
                package_target_roots
                    .entry(target.name.replace('-', "_"))
                    .or_default()
                    .push(PackageTargetRoot {
                        package_slot,
                        target_root: &target.src_path,
                    });
            }
        }
        if package_target_roots.is_empty() || scan_roots.is_empty() {
            stats.scan_duration = started.elapsed();
            return stats;
        }

        let dep_info_files = Self::matching_dep_info_files(
            &scan_roots,
            &package_names,
            &package_target_roots,
            &mut stats,
        );
        stats.dep_info_files = dep_info_files.values().map(Vec::len).sum();
        let mut build_output_candidates = vec![Vec::new(); packages.len()];

        for (artifact_name, files) in &dep_info_files {
            let Some(target_roots) = package_target_roots.get(artifact_name) else {
                continue;
            };

            for file in files {
                let Some(rustc_input_paths) = dep_info::rustc_input_paths(&file.path) else {
                    continue;
                };
                // Cargo spells local target roots relative to the workspace. An external build
                // directory can be shared with a checkout that has the same relative paths.
                // TODO: Recognize external build directories dedicated to this workspace.
                let workspace_local = scan_roots[file.root_rank].starts_with(workspace_root);
                let rustc_input_paths = rustc_input_paths
                    .into_iter()
                    .filter_map(|path| {
                        if path.is_absolute() {
                            fs::canonicalize(path).ok()
                        } else if workspace_local {
                            fs::canonicalize(workspace_root.join(path)).ok()
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>();
                // A name match is only a cheap scan filter. The rustc unit belongs to a package
                // only when its dependency list contains that package's exact target entrypoint.
                let matching_packages = target_roots
                    .iter()
                    .filter(|target| {
                        rustc_input_paths
                            .iter()
                            .any(|path| path == target.target_root)
                    })
                    .map(|target| target.package_slot)
                    .collect::<UniqueVec<_>>();
                if matching_packages.is_empty() {
                    continue;
                }
                stats.matched_rustc_units += 1;

                // One rustc unit should consume one build-script output directory. Grouping keeps
                // an unfamiliar mixed record deterministic instead of combining unrelated roots.
                let mut generated_by_run = BTreeMap::<BuildScriptRun, Vec<PathBuf>>::new();
                for input_path in rustc_input_paths {
                    let Some(run) = BuildScriptRun::for_input(&input_path, &scan_roots) else {
                        continue;
                    };
                    let generated_files = generated_by_run.entry(run).or_default();
                    if generated_files.len() < MAX_GENERATED_FILES_PER_BUILD_OUTPUT {
                        generated_files.push(input_path);
                    }
                }

                for (run, mut generated_files) in generated_by_run {
                    generated_files.sort();
                    generated_files.dedup();

                    // Dep-info is the source allow-list. Retained build-script output adds cfg and
                    // environment, but it is never trusted to invent a generated file by itself.
                    let (cfg_options, compile_env) =
                        BuildScriptInstructions::read(&run.stdout_path)
                            .into_cfg_and_env(&run.out_dir);
                    let generated_sources =
                        CargoGeneratedSources::new(run.out_dir, compile_env, generated_files);
                    let candidate = BuildOutputCandidate {
                        generated_sources,
                        cfg_options,
                        dep_info: file,
                    };
                    stats.build_output_candidates += 1;
                    for &package_slot in &matching_packages {
                        build_output_candidates[package_slot].push(candidate.clone());
                    }
                }
            }
        }

        // Cargo's reported build directory is authoritative when it contains a usable candidate;
        // the target-directory roots are historical fallbacks. Within one root, prefer the output
        // directory used by the newest matching rustc unit and use the path as a stable
        // tie-breaker. This does not recreate Cargo's active feature set; it chooses one useful,
        // internally consistent source snapshot.
        for (package, mut candidates) in packages.iter_mut().zip(build_output_candidates) {
            candidates
                .sort_by(|left, right| left.dep_info.priority().cmp(&right.dep_info.priority()));
            let Some(selected) = candidates.first() else {
                continue;
            };
            let selected_out_dir = selected.generated_sources.out_dir();

            // A library and its binaries are separate rustc units, but they reuse the package's
            // build-script output. Combine their dep-info allow-lists when they point at the
            // selected output directory so each target's `include!` can see the file it consumed.
            let mut generated_files = UniqueVec::new();
            'candidates: for candidate in &candidates {
                if candidate.generated_sources.out_dir() != selected_out_dir {
                    continue;
                }
                for path in candidate.generated_sources.generated_files() {
                    generated_files.push(path.clone());
                    if generated_files.len() == MAX_GENERATED_FILES_PER_BUILD_OUTPUT {
                        break 'candidates;
                    }
                }
            }

            let generated_sources = CargoGeneratedSources::new(
                selected_out_dir.to_path_buf(),
                selected.generated_sources.0.compile_env.clone(),
                generated_files.into_vec(),
            );

            stats.selected_packages += 1;
            stats.generated_files += generated_sources.generated_files().len();
            stats.generated_bytes += generated_sources
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
            package.cargo_generated_sources = Some(generated_sources);
        }

        stats.scan_duration = started.elapsed();
        stats
    }

    fn scan_roots(
        workspace_root: &Path,
        cargo_build_dir: Option<&Path>,
        cargo_target_dir: &Path,
    ) -> Vec<PathBuf> {
        let mut roots = UniqueVec::with_capacity(3);
        for root in cargo_build_dir.into_iter().map(Path::to_path_buf).chain([
            cargo_target_dir.to_path_buf(),
            workspace_root.join("target"),
        ]) {
            let root = fs::canonicalize(&root).unwrap_or(root);
            if root.is_dir() {
                roots.push(root);
            }
        }
        roots.into_vec()
    }

    fn matching_dep_info_files(
        scan_roots: &[PathBuf],
        package_names: &HashSet<&str>,
        target_roots: &HashMap<String, Vec<PackageTargetRoot<'_>>>,
        stats: &mut CargoBuildOutputScanStats,
    ) -> HashMap<String, Vec<DepInfoFile>> {
        let mut files = HashMap::<String, Vec<DepInfoFile>>::new();
        let scan = DirectoryScan {
            remaining_entries: Cell::new(MAX_DIRECTORY_ENTRIES),
        };
        let mut seen_directories = HashSet::new();
        for (root_rank, root) in scan_roots.iter().enumerate() {
            layout::visit_dep_info_directories(root, package_names, &scan, |directory| {
                let Ok(directory) = directory.canonicalize() else {
                    return;
                };
                // The metadata build directory can also be nested under a fallback root.
                if !directory.starts_with(root) || !seen_directories.insert(directory.clone()) {
                    return;
                }
                stats.dep_info_directories += 1;

                for entry in scan.entries(&directory) {
                    let path = entry.path();
                    if path.extension().is_none_or(|extension| extension != "d") {
                        continue;
                    }
                    let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
                        continue;
                    };
                    let artifact_name = stem
                        .rsplit_once('-')
                        .filter(|(_, suffix)| {
                            !suffix.is_empty()
                                && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
                        })
                        .map(|(name, _)| name)
                        .unwrap_or(stem);
                    if !target_roots.contains_key(artifact_name)
                        || !entry.file_type().is_ok_and(|kind| kind.is_file())
                    {
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
                            root_rank,
                        });
                }
            });
        }

        for files in files.values_mut() {
            files.sort_by(|left, right| left.priority().cmp(&right.priority()));
            files.truncate(MAX_BUILD_OUTPUT_CANDIDATES_PER_TARGET_NAME);
        }
        files
    }
}

#[derive(Clone)]
struct BuildOutputCandidate<'a> {
    generated_sources: CargoGeneratedSources,
    cfg_options: CfgOptions,
    dep_info: &'a DepInfoFile,
}

/// Nested directory walks share one entry budget across both layouts and all scan roots.
struct DirectoryScan {
    remaining_entries: Cell<usize>,
}

impl DirectoryScan {
    fn entries(&self, path: &Path) -> impl Iterator<Item = fs::DirEntry> {
        let mut entries = if self.remaining_entries.get() > 0 {
            fs::read_dir(path).ok()
        } else {
            None
        };
        iter::from_fn(move || {
            let remaining = self.remaining_entries.get().checked_sub(1)?;
            let entry = entries.as_mut()?.next()?;
            self.remaining_entries.set(remaining);
            Some(entry)
        })
        .flatten()
    }

    fn directories(&self, path: &Path) -> impl Iterator<Item = PathBuf> {
        self.entries(path)
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .map(|entry| entry.path())
    }
}

struct PackageTargetRoot<'a> {
    package_slot: usize,
    target_root: &'a Path,
}

struct DepInfoFile {
    path: PathBuf,
    modified: u128,
    root_rank: usize,
}

impl DepInfoFile {
    fn priority(&self) -> (usize, Reverse<u128>, &Path) {
        (self.root_rank, Reverse(self.modified), &self.path)
    }
}

#[cfg(test)]
mod tests;
