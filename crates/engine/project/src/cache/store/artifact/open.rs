//! Opens the fixed outer directory and probe, then pins that file revision in a reader.
//!
//! Opening is intentionally shallow. It proves that the file has a valid outer layout and that its
//! probe belongs to the requested package, but it leaves DefMap, Semantic IR, and Body IR payloads
//! untouched. The returned reader keeps this exact file handle so later phase loads cannot observe
//! a different revision at the same path.

use std::{
    fs::File,
    io::Read as _,
    sync::{Arc, Mutex, OnceLock},
    time::Instant,
};

use rg_package_store::MalformedCacheError;
use rg_text::NameInterner;

use super::{PackageArtifactReader, PackageArtifactReaderInner, PackageCacheReadError};
use crate::{
    cache::{
        CachedPackage, PackageCacheCodec, PackageCacheHeader, PackageCacheProbe, PackageCacheStore,
        codec::{PACKAGE_CACHE_CONTAINER_PREFIX_BYTES, PackageCacheLayout},
    },
    profile::metric,
};

impl PackageCacheStore {
    /// Open an artifact expected to match this complete header.
    ///
    /// `open_artifact_for_package` first checks the package identity stored in the file. This second
    /// comparison also checks source fingerprint and schema facts carried by the expected header.
    pub(crate) fn open_artifact(
        &self,
        header: &PackageCacheHeader,
    ) -> Result<Option<PackageArtifactReader>, PackageCacheReadError> {
        let reader = self.open_artifact_for_package(&header.package)?;
        let Some(reader) = reader else {
            return Ok(None);
        };
        if reader.probe().header != *header {
            return Err(self.header_mismatch(&header.package, &reader.probe().header.package));
        }
        Ok(Some(reader))
    }

    /// Read only the outer directory and probe for startup cache validation.
    ///
    /// Dropping the temporary reader closes the file without decoding any retained IR section.
    pub(crate) fn read_probe_for_package(
        &self,
        package: &CachedPackage,
    ) -> Result<Option<PackageCacheProbe>, PackageCacheReadError> {
        Ok(self
            .open_artifact_for_package(package)?
            .map(|reader| reader.probe().clone()))
    }

    /// Open one package path, validate its shallow framing, and construct a pinned reader.
    fn open_artifact_for_package(
        &self,
        package: &CachedPackage,
    ) -> Result<Option<PackageArtifactReader>, PackageCacheReadError> {
        // 1. A missing file is an ordinary cache miss. Other open failures retain their I/O source
        // so callers can distinguish unavailable storage from malformed bytes.
        let read_started = Instant::now();
        let path = self.package_artifact_path(package);
        let mut file = match File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(PackageCacheReadError::Io { path, source }),
        };
        // 2. Read and validate the fixed outer directory against the complete file length. This
        // establishes trusted ranges before any variable-size section is allocated.
        let file_len = file
            .metadata()
            .map_err(|source| PackageCacheReadError::Io {
                path: path.clone(),
                source,
            })?
            .len();
        let mut prefix = [0_u8; PACKAGE_CACHE_CONTAINER_PREFIX_BYTES];
        if let Err(source) = file.read_exact(&mut prefix) {
            if source.kind() == std::io::ErrorKind::UnexpectedEof {
                return Err(PackageArtifactReader::truncated_artifact(
                    &path,
                    format!(
                        "package cache artifact is shorter than its {PACKAGE_CACHE_CONTAINER_PREFIX_BYTES}-byte prefix"
                    ),
                ));
            }
            return Err(PackageCacheReadError::Io { path, source });
        }
        let layout = PackageCacheLayout::decode_prefix(&prefix, file_len).map_err(|error| {
            PackageCacheReadError::Malformed {
                source: MalformedCacheError::Decode {
                    path: path.clone(),
                    reason: format!("{error:#}"),
                },
            }
        })?;
        // 3. Read and decode only the probe. It contains the package identity used to reject a file
        // found at the right path but containing bytes for another package.
        let probe_bytes =
            PackageArtifactReader::read_section_bytes(&path, &mut file, layout.probe)?;
        metric::CACHE_SECTION_READ.record("probe", read_started.elapsed());
        metric::CACHE_SECTION_BYTES.add(
            "probe",
            layout.probe.len + PACKAGE_CACHE_CONTAINER_PREFIX_BYTES as u64,
        );
        let decode_started = Instant::now();
        let probe = PackageCacheCodec::decode_probe(&probe_bytes).map_err(|error| {
            PackageCacheReadError::Malformed {
                source: MalformedCacheError::Decode {
                    path: path.clone(),
                    reason: format!("{error:#}"),
                },
            }
        })?;
        metric::CACHE_SECTION_DECODE.record("probe", decode_started.elapsed());
        if probe.header.package != *package {
            return Err(self.header_mismatch(package, &probe.header.package));
        }

        // 4. Keep the open file, validated ranges, and probe together. Body IR's nested index and
        // the shared name table start empty and grow only when later phase queries need them.
        Ok(Some(PackageArtifactReader {
            inner: Arc::new(PackageArtifactReaderInner {
                path,
                file: Mutex::new(file),
                layout,
                probe,
                body_index: OnceLock::new(),
                names: Mutex::new(NameInterner::new()),
            }),
        }))
    }

    /// Report both package identities when a path and decoded probe disagree.
    fn header_mismatch(
        &self,
        expected: &CachedPackage,
        actual: &CachedPackage,
    ) -> PackageCacheReadError {
        PackageCacheReadError::Malformed {
            source: MalformedCacheError::HeaderMismatch {
                path: self.package_artifact_path(expected),
                actual_slot: actual.package.0,
                actual_name: actual.name.clone(),
                expected_slot: expected.package.0,
                expected_name: expected.name.clone(),
            },
        }
    }
}
