//! Reads just enough of rustc's Makefile-style dep-info to recover source provenance.
//!
//! The first complete rule lists every input used by one rustc unit. Build-output discovery needs
//! that dependency side: an exact package target root identifies the owner, and paths below a Cargo
//! build-script output directory identify generated Rust files. `depinfo` supplies the rustc/Cargo
//! format knowledge; this module only owns rust-glancer's input bound and non-empty-file policy.

use std::{fs::File, io::Read as _, path::PathBuf};

const MAX_DEP_INFO_BYTES: u64 = 8 * 1024 * 1024;

/// Reads the dependency/source side of the first complete Makefile rule emitted by rustc.
pub(super) fn rustc_input_paths(path: &std::path::Path) -> Option<Vec<PathBuf>> {
    let file = File::open(path).ok()?;
    let metadata = file.metadata().ok()?;
    if metadata.len() > MAX_DEP_INFO_BYTES {
        return None;
    }

    // `depinfo::RustcDepInfo::from_file` reads the complete file. Keep rust-glancer's hard input
    // bound even if the file grows after the metadata check by exposing at most one extra byte.
    let mut contents = String::with_capacity(metadata.len().try_into().ok()?);
    file.take(MAX_DEP_INFO_BYTES + 1)
        .read_to_string(&mut contents)
        .ok()?;
    if contents.len() as u64 > MAX_DEP_INFO_BYTES {
        return None;
    }
    let paths = depinfo::RustcDepInfo::new(&contents).ok()?.files;
    (!paths.is_empty()).then_some(paths)
}
