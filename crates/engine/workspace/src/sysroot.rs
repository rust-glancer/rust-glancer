//! Discovers and validates source roots for compiler-provided Rust crates.
//!
//! The LSP can continue without rust-src, so discovery returns `None` when the selected toolchain
//! has no complete source tree. A usable tree must contain every compiler-provided crate that
//! rust-glancer models.

use rg_std::MemorySize;
use std::{
    fmt,
    path::{Path, PathBuf},
    process::Command,
};

/// Sysroot crates that rust-glancer can model as ordinary library roots.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, MemorySize)]
#[memsize(leaf)]
pub enum SysrootCrate {
    Core,
    Alloc,
    Std,
    ProcMacro,
}

impl SysrootCrate {
    /// Every sysroot crate that rust-glancer models from an installed rust-src component.
    pub(crate) const ALL: [Self; 4] = [Self::Core, Self::Alloc, Self::Std, Self::ProcMacro];

    pub fn name(self) -> &'static str {
        match self {
            Self::Core => "core",
            Self::Alloc => "alloc",
            Self::Std => "std",
            Self::ProcMacro => "proc_macro",
        }
    }
}

impl fmt::Display for SysrootCrate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Validated Rust source tree discovered from `rustc --print sysroot`.
///
/// Construction requires source roots for every crate represented by [`SysrootCrate`]. This
/// matches the complete `library` tree installed by the standard rust-src component.
#[derive(Debug, Clone, PartialEq, Eq, MemorySize)]
pub struct SysrootSources {
    pub(crate) library_root: PathBuf,
}

impl SysrootSources {
    /// Discovers rust-src using the toolchain selected for `workspace_root`.
    ///
    /// Missing `rustc`, a failing command, or missing rust-src all disable sysroot support for the
    /// caller. The LSP remains useful without standard-library sources.
    pub fn discover(workspace_root: &Path) -> Option<Self> {
        let output = Command::new("rustc")
            .arg("--print")
            .arg("sysroot")
            .current_dir(workspace_root)
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }

        let sysroot = String::from_utf8(output.stdout).ok()?;
        let sysroot_root = PathBuf::from(sysroot.trim());
        if sysroot_root.as_os_str().is_empty() {
            return None;
        }

        let library_root = sysroot_root
            .join("lib")
            .join("rustlib")
            .join("src")
            .join("rust")
            .join("library");
        Self::from_library_root(library_root)
    }

    /// Builds a sysroot source model from an explicit `.../rust/library` path.
    ///
    /// This is mostly used by tests, where a tiny fake sysroot is easier and more deterministic
    /// than relying on the developer's installed toolchain.
    pub fn from_library_root(library_root: impl Into<PathBuf>) -> Option<Self> {
        let library_root = rg_std::path::canonicalize(library_root.into()).ok()?;
        let sources = Self { library_root };

        SysrootCrate::ALL
            .iter()
            .all(|krate| sources.crate_root(*krate).is_file())
            .then_some(sources)
    }

    pub fn library_root(&self) -> &Path {
        &self.library_root
    }

    pub(crate) fn crate_root(&self, krate: SysrootCrate) -> PathBuf {
        self.library_root
            .join(krate.name())
            .join("src")
            .join("lib.rs")
    }
}
