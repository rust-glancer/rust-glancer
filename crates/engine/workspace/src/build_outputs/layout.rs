//! Filesystem paths for Cargo's private intermediate build layouts.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use super::DirectoryScan;

#[derive(PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct BuildScriptRun {
    pub(super) out_dir: PathBuf,
    pub(super) stdout_path: PathBuf,
}

impl BuildScriptRun {
    /// Locates the run that produced an already canonicalized dep-info input.
    pub(super) fn for_input(source_path: &Path, scan_roots: &[PathBuf]) -> Option<Self> {
        if !scan_roots.iter().any(|root| source_path.starts_with(root)) {
            return None;
        }

        source_path.ancestors().skip(1).find_map(|out_dir| {
            if out_dir.file_name()? != "out" {
                return None;
            }
            let unit = out_dir.parent()?;
            let parent = unit.parent()?;

            let stdout_path = if has_unit_hash(unit) {
                // The dep-info input gives us the run hash. The compiler unit that consumed it
                // has a different hash and its own out directory.
                let run = unit.join("run");
                if !is_build_directory(parent.parent()?, scan_roots) || !run.is_dir() {
                    return None;
                }
                run.join("stdout")
            } else {
                let unit_name = unit.file_name()?.to_str()?;
                if !unit_name.contains('-') || !is_build_directory(parent, scan_roots) {
                    return None;
                }
                unit.join("output")
            };

            Some(Self {
                out_dir: out_dir.to_path_buf(),
                stdout_path,
            })
        })
    }
}

/// Looks for both layouts under `<profile>` and `<target-triple>/<profile>`. Both can be
/// present in the same profile directory after switching toolchains.
pub(super) fn visit_dep_info_directories(
    root: &Path,
    package_names: &HashSet<&str>,
    scan: &DirectoryScan,
    mut visit: impl FnMut(&Path),
) {
    let mut visit_profile = |profile: &Path| {
        // Before Cargo 1.100, rustc units shared the profile's deps directory.
        let deps = profile.join("deps");
        if deps.is_dir() {
            visit(&deps);
        }

        // Cargo 1.100 puts rustc output in build/<package>/<unit-hash>/out.
        // A shared build directory can contain many packages this workspace never uses.
        for package in scan.directories(&profile.join("build")) {
            if !package
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| package_names.contains(name))
            {
                continue;
            }

            for unit in scan.directories(&package) {
                // A build-script run also has an out directory, but its files are generated
                // inputs, not compiler records.
                if has_unit_hash(&unit) && !unit.join("run").is_dir() {
                    let out = unit.join("out");
                    if out.is_dir() {
                        visit(&out);
                    }
                }
            }
        }
    };

    for first in scan.directories(root) {
        visit_profile(&first);
        for second in scan.directories(&first) {
            visit_profile(&second);
        }
    }
}

fn has_unit_hash(unit: &Path) -> bool {
    unit.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.len() == 16 && name.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

fn is_build_directory(directory: &Path, roots: &[PathBuf]) -> bool {
    directory.file_name().is_some_and(|name| name == "build")
        && directory.parent().is_some_and(|profile| {
            roots.iter().any(|root| {
                profile
                    .strip_prefix(root)
                    .is_ok_and(|path| matches!(path.components().count(), 1 | 2))
            })
        })
}
