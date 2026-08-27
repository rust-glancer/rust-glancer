//! Reading one immutable package artifact revision.
//!
//! Opening the artifact reads only its fixed directory and probe. Later phase loaders share the
//! same file handle, parsed directory, and package-local name interner. The phase modules add their
//! nested directories; [`open`] binds a filesystem path to this reader; [`error`] translates
//! storage failures into the package-store boundary.
//!
//! After opening, an ordinary phase read has three steps:
//!
//! 1. Use the phase directory to select one validated nested byte range.
//! 2. Decode those bytes while reusing the package-local `NameInterner`.
//! 3. Validate the decoded phase against the already-decoded probe.
//!
//! The file and interner use mutexes because DefMap, Semantic IR, and Body IR loaders may share
//! cloned reader handles. Holding one open file handle also pins the request to one artifact
//! revision even if an atomic cache update later replaces the path.

mod body;
mod def_map;
mod error;
mod open;
mod semantic_ir;

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
    PackageCacheProbe,
    codec::{
        PackageBodyCacheIndex, PackageCacheLayout, PackageCacheSectionRange,
        PackageDefMapCacheIndex, PackageSemanticIrCacheIndex,
    },
};

/// One open package artifact revision shared by phase loaders in a read transaction.
///
/// Clones share the file handle, parsed layout, lazy phase indexes, probe, and name interner. They do
/// not reopen the path and do not retain decoded DefMap or Semantic IR values after returning them;
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
    /// Nested DefMap directory, decoded only when a DefMap query needs it.
    def_map_index: OnceLock<PackageDefMapCacheIndex>,
    /// Nested Semantic IR directory, decoded only when a declaration query needs it.
    semantic_ir_index: OnceLock<PackageSemanticIrCacheIndex>,
    /// Nested Body IR directory, decoded only when a Body IR query needs it.
    body_index: OnceLock<PackageBodyCacheIndex>,
    /// Package-local names shared by all independently decoded sections in this request.
    names: Mutex<NameInterner>,
}

impl PackageArtifactReader {
    pub(crate) fn probe(&self) -> &PackageCacheProbe {
        &self.inner.probe
    }

    /// Reads the complete encoded DefMap section for a Body-only artifact rewrite.
    ///
    /// The outer range was validated when this reader opened. Nested manifests and crate payloads
    /// remain encoded here and are validated normally when a later exact read opens them.
    pub(crate) fn read_encoded_def_map_section(&self) -> Result<Vec<u8>, PackageCacheReadError> {
        self.read_section("def_map.copy", self.inner.layout.def_map)
    }

    /// Reads the complete encoded Semantic IR section for a Body-only artifact rewrite.
    ///
    /// Copying preserves all nested crate boundaries without decoding declaration data that the
    /// rebuild did not change.
    pub(crate) fn read_encoded_semantic_ir_section(
        &self,
    ) -> Result<Vec<u8>, PackageCacheReadError> {
        self.read_section("semantic_ir.copy", self.inner.layout.semantic_ir)
    }

    /// Validate a nested section-relative range and translate it into outer-file coordinates.
    fn read_nested_range(
        &self,
        label: &'static str,
        section: PackageCacheSectionRange,
        range: PackageCacheSectionRange,
    ) -> Result<Vec<u8>, PackageCacheReadError> {
        let end = range
            .offset
            .checked_add(range.len)
            .ok_or_else(|| self.decode_error(anyhow::anyhow!("{label} range overflows u64")))?;
        if end > section.len {
            return Err(self.decode_error(anyhow::anyhow!(
                "{label} range ends at byte {end}, section has {} bytes",
                section.len,
            )));
        }
        let offset = section.offset.checked_add(range.offset).ok_or_else(|| {
            self.decode_error(anyhow::anyhow!("{label} file offset overflows u64"))
        })?;
        self.read_section(
            label,
            PackageCacheSectionRange {
                offset,
                len: range.len,
            },
        )
    }

    /// Read and decode a prefix-framed nested directory without touching its payload bytes.
    ///
    /// DefMap, Semantic IR, and Body IR have different logical indexes, but their artifact readers
    /// all discover those indexes through the same fixed-prefix and variable-manifest sequence.
    /// Keeping the bounds checks, diagnostics, and metrics here prevents the lazy phase paths from
    /// drifting apart.
    fn read_nested_index<T>(
        &self,
        section_name: &'static str,
        label: &'static str,
        section: PackageCacheSectionRange,
        prefix_len: usize,
        decode_prefix: fn(&[u8]) -> anyhow::Result<usize>,
        decode_index: fn(&[u8], u64, &PackageCacheProbe) -> anyhow::Result<T>,
    ) -> Result<T, PackageCacheReadError> {
        // The outer layout has already been validated, but the nested section still needs enough
        // bytes for its own magic and manifest-length field.
        let prefix_len = u64::try_from(prefix_len).map_err(|error| {
            self.decode_error(anyhow::anyhow!(
                "{section_name} prefix length does not fit u64: {error}"
            ))
        })?;
        if section.len < prefix_len {
            return Err(self.decode_error(anyhow::anyhow!(
                "{section_name} section is shorter than its {prefix_len}-byte prefix"
            )));
        }

        // Read the fixed prefix first so a corrupt length cannot make the following allocation
        // escape the phase section.
        let prefix = self.read_nested_range(
            label,
            section,
            PackageCacheSectionRange {
                offset: 0,
                len: prefix_len,
            },
        )?;
        let manifest_len = decode_prefix(&prefix).map_err(|error| self.decode_error(error))?;
        let manifest_len = u64::try_from(manifest_len)
            .map_err(|error| self.decode_error(anyhow::anyhow!(error)))?;
        let manifest_end = prefix_len.checked_add(manifest_len).ok_or_else(|| {
            self.decode_error(anyhow::anyhow!("{section_name} manifest overflows u64"))
        })?;
        if manifest_end > section.len {
            return Err(self.decode_error(anyhow::anyhow!(
                "{section_name} manifest ends at byte {manifest_end}, section has {} bytes",
                section.len,
            )));
        }

        // Decode only the bounded manifest. The returned phase index owns all payload ranges, so
        // later reads no longer need to know about this framing.
        let bytes = self.read_nested_range(
            label,
            section,
            PackageCacheSectionRange {
                offset: prefix_len,
                len: manifest_len,
            },
        )?;
        let started = Instant::now();
        let decoded = decode_index(&bytes, section.len, &self.inner.probe)
            .map_err(|error| self.decode_error(error));
        metric::CACHE_SECTION_DECODE.record(label, started.elapsed());
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
