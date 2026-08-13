//! Source-boundary failures that preserve enough meaning for project recovery.
//!
//! Ordinary I/O and UTF-8 failures explain why source could not be captured. `Stale`, `Missing`,
//! and `ExistenceChanged` mean something stronger: a generation was internally coherent, but the
//! filesystem no longer matches it. Project hosts use that distinction to retry from the newer
//! disk state instead of treating a write that landed during construction as a permanent failure.

use std::{
    error::Error,
    fmt,
    path::{Path, PathBuf},
    str,
};

use crate::SourceRevision;

/// Failure to capture, access, or verify source for a generation.
#[derive(Debug)]
pub enum SourceError {
    /// The source path could not be read from the filesystem.
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// Source bytes were read, but they cannot be parsed as UTF-8 Rust source.
    InvalidUtf8 {
        path: PathBuf,
        source: str::Utf8Error,
    },
    /// Disk now contains a different revision than the generation captured.
    Stale {
        path: PathBuf,
        expected: SourceRevision,
        actual: SourceRevision,
    },
    /// A known saved source disappeared after the generation captured it.
    Missing {
        path: PathBuf,
        expected: SourceRevision,
    },
    /// A module candidate appeared or disappeared after file discovery.
    ExistenceChanged {
        path: PathBuf,
        expected: bool,
        actual: bool,
    },
    /// A source override attempted to introduce a path outside its saved source universe.
    Unknown { path: PathBuf },
    /// A sealed generation was asked to discover or replace a source path.
    Sealed { path: PathBuf },
}

impl SourceError {
    /// Returns the path that invalidated an otherwise usable project generation.
    pub fn stale_path(&self) -> Option<&Path> {
        match self {
            Self::Stale { path, .. }
            | Self::Missing { path, .. }
            | Self::ExistenceChanged { path, .. } => Some(path),
            _ => None,
        }
    }

    /// Preserves the underlying I/O kind for watcher races such as a file disappearing mid-save.
    pub fn io_kind(&self) -> Option<std::io::ErrorKind> {
        match self {
            Self::Io { source, .. } => Some(source.kind()),
            Self::Missing { .. } => Some(std::io::ErrorKind::NotFound),
            _ => None,
        }
    }
}

impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, .. } => write!(f, "failed to read source {}", path.display()),
            Self::InvalidUtf8 { path, .. } => {
                write!(f, "source {} is not valid UTF-8", path.display())
            }
            Self::Stale {
                path,
                expected,
                actual,
            } => write!(
                f,
                "source {} changed from revision {expected} to {actual}",
                path.display()
            ),
            Self::Missing { path, expected } => write!(
                f,
                "source {} at revision {expected} is missing",
                path.display()
            ),
            Self::ExistenceChanged {
                path,
                expected,
                actual,
            } => write!(
                f,
                "source existence for {} changed from {expected} to {actual}",
                path.display()
            ),
            Self::Sealed { path } => write!(
                f,
                "sealed source inventory cannot discover {}",
                path.display()
            ),
            Self::Unknown { path } => write!(
                f,
                "source {} is not part of the selected project generation",
                path.display()
            ),
        }
    }
}

impl Error for SourceError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::InvalidUtf8 { source, .. } => Some(source),
            Self::Stale { .. }
            | Self::Missing { .. }
            | Self::ExistenceChanged { .. }
            | Self::Unknown { .. }
            | Self::Sealed { .. } => None,
        }
    }
}
