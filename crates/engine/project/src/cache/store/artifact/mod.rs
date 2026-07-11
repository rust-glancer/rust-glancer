//! Reading one immutable package artifact revision.
//!
//! Opening the artifact reads only its fixed directory and probe. Later phase loaders share the
//! same file handle, parsed directory, and package-local name interner. [`body`] adds the nested
//! Body IR directory; [`open`] binds a filesystem path to this reader; [`error`] translates storage
//! failures into the package-store boundary.
//!
//! After opening, an ordinary phase read has three steps:
//!
//! 1. Take the validated byte range from `layout` and read exactly those bytes.
//! 2. Decode them while reusing the package-local `NameInterner`.
//! 3. Validate the decoded phase against the already-decoded probe.
//!
//! The file and interner use mutexes because DefMap, Semantic IR, and Body IR loaders may share
//! cloned reader handles. Holding one open file handle also pins the request to one artifact
//! revision even if an atomic cache update later replaces the path.

mod body;
mod error;
mod open;

use std::{
    fs::File,
    io::{Read as _, Seek as _, SeekFrom},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::Instant,
};

use rg_package_store::MalformedCacheError;
use rg_text::NameInterner;

use crate::profile::metric;

pub(crate) use self::error::PackageCacheReadError;
use super::super::{
    PackageCacheCodec, PackageCacheProbe,
    codec::{PackageBodyCacheIndex, PackageCacheLayout, PackageCacheSectionRange},
};

/// One open package artifact revision shared by phase loaders in a read transaction.
///
/// Clones share the file handle, parsed layout, probe, lazy Body IR index, and name interner. They do
/// not reopen the path and do not keep decoded DefMap or Semantic IR values after returning them;
/// those values are owned by the phase-specific read transaction.
#[derive(Debug, Clone)]
pub(crate) struct PackageArtifactReader {
    inner: Arc<PackageArtifactReaderInner>,
}

#[derive(Debug)]
struct PackageArtifactReaderInner {
    /// Stable path used in diagnostics for this open revision.
    path: PathBuf,
    /// Shared seek cursor for exact section reads.
    file: Mutex<File>,
    /// Outer ranges validated against the complete file length during open.
    layout: PackageCacheLayout,
    /// Small package identity and parse snapshot loaded during open.
    probe: PackageCacheProbe,
    /// Nested Body IR directory, decoded only when a Body IR query needs it.
    body_index: OnceLock<PackageBodyCacheIndex>,
    /// Package-local names shared by all independently decoded sections in this request.
    names: Mutex<NameInterner>,
}

impl PackageArtifactReader {
    pub(crate) fn probe(&self) -> &PackageCacheProbe {
        &self.inner.probe
    }

    /// Read and decode only the package DefMap section.
    pub(crate) fn read_def_map(
        &self,
    ) -> Result<rg_ir_storage::PackageDefMaps, PackageCacheReadError> {
        let bytes = self.read_section("def_map", self.inner.layout.def_map)?;
        let started = Instant::now();
        let decoded = self
            .decode_with_names(|| PackageCacheCodec::decode_def_map(&bytes, &self.inner.probe))
            .map_err(|error| self.decode_error(error));
        metric::CACHE_SECTION_DECODE.record("def_map", started.elapsed());
        decoded
    }

    /// Read and decode only the package Semantic IR section.
    pub(crate) fn read_semantic_ir(
        &self,
    ) -> Result<rg_semantic_ir::PackageIr, PackageCacheReadError> {
        let bytes = self.read_section("semantic_ir", self.inner.layout.semantic_ir)?;
        let started = Instant::now();
        let decoded = self
            .decode_with_names(|| PackageCacheCodec::decode_semantic_ir(&bytes, &self.inner.probe))
            .map_err(|error| self.decode_error(error));
        metric::CACHE_SECTION_DECODE.record("semantic_ir", started.elapsed());
        decoded
    }

    /// Read one labeled outer-file range and record its I/O cost.
    ///
    /// The file mutex protects the shared seek cursor. It is released before decoding, so an
    /// expensive decoder does not block another loader from reading its own section bytes.
    fn read_section(
        &self,
        label: &'static str,
        range: PackageCacheSectionRange,
    ) -> Result<Vec<u8>, PackageCacheReadError> {
        let started = Instant::now();
        let mut file = self
            .inner
            .file
            .lock()
            .expect("package artifact file mutex should not be poisoned");
        let bytes = Self::read_section_bytes(&self.inner.path, &mut file, range);
        metric::CACHE_SECTION_READ.record(label, started.elapsed());
        metric::CACHE_SECTION_BYTES.add(label, range.len);
        bytes
    }

    /// Seek and read exactly one already-validated range from an open artifact.
    ///
    /// This helper is also used during open, before a `PackageArtifactReader` exists. An unexpected
    /// EOF is classified as malformed cache data rather than a generic I/O failure because the
    /// declared file layout promised these bytes were present.
    fn read_section_bytes(
        path: &Path,
        file: &mut File,
        range: PackageCacheSectionRange,
    ) -> Result<Vec<u8>, PackageCacheReadError> {
        let len = usize::try_from(range.len).map_err(|error| PackageCacheReadError::Malformed {
            source: MalformedCacheError::Decode {
                path: path.to_path_buf(),
                reason: format!("package cache section length does not fit usize: {error}"),
            },
        })?;
        file.seek(SeekFrom::Start(range.offset))
            .map_err(|source| PackageCacheReadError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        let mut bytes = vec![0_u8; len];
        if let Err(source) = file.read_exact(&mut bytes) {
            if source.kind() == std::io::ErrorKind::UnexpectedEof {
                return Err(Self::truncated_artifact(
                    path,
                    format!(
                        "package cache section at byte {} is shorter than its declared {} bytes",
                        range.offset, range.len,
                    ),
                ));
            }
            return Err(PackageCacheReadError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
        Ok(bytes)
    }

    /// Decode one independent section using the names accumulated by earlier sections.
    ///
    /// Wincode sections are framed independently, but their decoded engine values should reuse one
    /// package-local name table. Temporarily installing this interner gives each decoder the same
    /// context and then stores the expanded table back for the next section.
    fn decode_with_names<T>(
        &self,
        decode: impl FnOnce() -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let mut names = self
            .inner
            .names
            .lock()
            .expect("package decode name interner mutex should not be poisoned");
        let interner = std::mem::take(&mut *names);
        let (interner, decoded) = rg_text::with_decode_name_interner(interner, decode);
        *names = interner;
        decoded
    }

    /// Attach the artifact path and preserve the decoder's complete context chain.
    fn decode_error(&self, error: anyhow::Error) -> PackageCacheReadError {
        PackageCacheReadError::Malformed {
            source: MalformedCacheError::Decode {
                path: self.inner.path.clone(),
                reason: format!("{error:#}"),
            },
        }
    }

    /// Build the malformed-cache form used when declared bytes are missing.
    fn truncated_artifact(path: &Path, reason: String) -> PackageCacheReadError {
        PackageCacheReadError::Malformed {
            source: MalformedCacheError::Decode {
                path: path.to_path_buf(),
                reason,
            },
        }
    }
}
