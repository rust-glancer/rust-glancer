use std::{
    fmt,
    path::{Path, PathBuf},
    process::Command,
};

/// Sysroot crates that rust-glancer can model as ordinary library roots.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SysrootCrate {
    Core,
    Alloc,
    Std,
}

impl SysrootCrate {
    pub const ALL: [Self; 3] = [Self::Core, Self::Alloc, Self::Std];

    pub fn name(self) -> &'static str {
        match self {
            Self::Core => "core",
            Self::Alloc => "alloc",
            Self::Std => "std",
        }
    }
}

impl fmt::Display for SysrootCrate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Rust source tree discovered from `rustc --print sysroot`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SysrootSources {
    pub(crate) sysroot_root: PathBuf,
    pub(crate) library_root: PathBuf,
}

impl SysrootSources {
    /// Discovers rust-src using the toolchain selected for `workspace_root`.
    ///
    /// Missing `rustc`, a failing command, or missing rust-src all simply disable sysroot support.
    /// The analysis pipeline is still useful without sysroot crates, and the user can install
    /// `rust-src` when they want standard-library navigation.
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

        Self::from_sysroot_root(sysroot_root)
    }

    /// Builds a sysroot source model from an explicit `.../rust/library` path.
    ///
    /// This is mostly used by tests, where a tiny fake sysroot is easier and more deterministic
    /// than relying on the developer's installed toolchain.
    pub fn from_library_root(library_root: impl Into<PathBuf>) -> Option<Self> {
        let library_root = library_root.into();
        Self::from_roots(library_root.clone(), library_root)
    }

    pub fn library_root(&self) -> &Path {
        &self.library_root
    }

    pub fn crate_root(&self, krate: SysrootCrate) -> PathBuf {
        self.library_root
            .join(krate.name())
            .join("src")
            .join("lib.rs")
    }

    fn from_sysroot_root(sysroot_root: PathBuf) -> Option<Self> {
        let library_root = sysroot_root
            .join("lib")
            .join("rustlib")
            .join("src")
            .join("rust")
            .join("library");
        Self::from_roots(sysroot_root, library_root)
    }

    fn from_roots(sysroot_root: PathBuf, library_root: PathBuf) -> Option<Self> {
        let sysroot_root = sysroot_root.canonicalize().ok()?;
        let library_root = library_root.canonicalize().ok()?;
        let sources = Self {
            sysroot_root,
            library_root,
        };

        if SysrootCrate::ALL
            .iter()
            .all(|krate| sources.crate_root(*krate).is_file())
        {
            Some(sources)
        } else {
            None
        }
    }
}
