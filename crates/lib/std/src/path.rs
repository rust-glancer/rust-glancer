//! Filesystem canonicalization that keeps paths editor-friendly.
//!
//! On Windows, [`std::fs::canonicalize`] returns *verbatim* paths such as
//! `\\?\D:\src\lib.rs`. Verbatim paths break two things this project relies on:
//!
//! - URI round-trips. LSP URIs are built by percent-encoding the plain path, and verbatim
//!   input produces URIs that editors cannot map back to a real file.
//! - Path comparisons. A verbatim prefix makes otherwise equal paths unequal
//!   (`\\?\D:\a` != `D:\a`), silently breaking workspace membership checks.
//!
//! [`canonicalize`] behaves like `std::fs::canonicalize`, but returns the plain
//! form on Windows:
//!
//! - `\\?\D:\src\lib.rs` becomes `D:\src\lib.rs`
//! - `\\?\UNC\server\share\lib.rs` becomes `\\server\share\lib.rs`

use std::{
    io,
    path::{Path, PathBuf},
};

/// Canonicalizes a path, stripping the verbatim prefix on Windows.
pub fn canonicalize(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    #[cfg(not(windows))]
    {
        path.as_ref().canonicalize()
    }

    #[cfg(windows)]
    {
        path.as_ref().canonicalize().map(strip_verbatim_prefix)
    }
}

/// Rewrites `\\?\`-prefixed components back into their plain form.
///
/// The drive letter is preserved as-is, matching the casing of the input.
#[cfg(windows)]
fn strip_verbatim_prefix(path: PathBuf) -> PathBuf {
    use std::path::{Component, Prefix};

    let mut plain = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => match prefix.kind() {
                Prefix::VerbatimDisk(disk) => plain.push(format!("{}:", disk as char)),
                Prefix::VerbatimUNC(server, share) => plain.push(format!(
                    r"\\{}\{}",
                    server.to_string_lossy(),
                    share.to_string_lossy()
                )),
                // Device-bar and already-plain prefixes are left untouched.
                _ => plain.push(component.as_os_str()),
            },
            _ => plain.push(component.as_os_str()),
        }
    }
    plain
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::atomic::{AtomicU32, Ordering},
    };

    use super::*;

    /// Creates an empty unique file inside the temp directory and returns its path.
    fn temp_file() -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let id = (std::process::id(), COUNTER.fetch_add(1, Ordering::Relaxed));
        let path = std::env::temp_dir().join(format!("rg-std-path-test-{}-{}", id.0, id.1));
        fs::write(&path, b"").expect("temp file should be writable");
        path
    }

    #[cfg(not(windows))]
    #[test]
    fn matches_std_canonicalize() {
        let path = temp_file();
        assert_eq!(canonicalize(&path).unwrap(), path.canonicalize().unwrap());
    }

    #[cfg(windows)]
    #[test]
    fn canonicalized_temp_path_has_no_verbatim_prefix() {
        let path = temp_file();
        let canonical = canonicalize(&path).unwrap();

        assert!(!canonical.as_os_str().to_string_lossy().starts_with(r"\\?\"));
        assert_eq!(
            canonical,
            strip_verbatim_prefix(path.canonicalize().unwrap()),
        );
    }

    #[cfg(windows)]
    #[test]
    fn strips_verbatim_disk_prefix() {
        let plain = strip_verbatim_prefix(PathBuf::from(r"\\?\D:\dir\file.rs"));

        assert_eq!(plain, PathBuf::from(r"D:\dir\file.rs"));
    }

    #[cfg(windows)]
    #[test]
    fn rewrites_verbatim_unc_prefix_to_plain_unc() {
        let plain = strip_verbatim_prefix(PathBuf::from(r"\\?\UNC\server\share\file.rs"));

        assert_eq!(plain, PathBuf::from(r"\\server\share\file.rs"));
    }

    #[cfg(windows)]
    #[test]
    fn leaves_already_plain_paths_unchanged() {
        let plain = strip_verbatim_prefix(PathBuf::from(r"D:\dir\file.rs"));

        assert_eq!(plain, PathBuf::from(r"D:\dir\file.rs"));
    }
}
