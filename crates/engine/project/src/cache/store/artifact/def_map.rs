//! Reads the nested DefMap directory and exact crate payloads.
//!
//! The directory is initialized on the first DefMap access and then shared by reader clones. Package
//! metadata and routing need no crate bytes; module scopes are read only when a query names a crate.

use std::time::Instant;

#[cfg(test)]
use rg_def_map::PackageDefMaps;
use rg_def_map::{CrateData, PackageDefMapsManifest};
use rg_ir_model::{CrateId, CrateRef};

use super::{PackageArtifactReader, PackageCacheReadError};
use crate::{
    cache::{PackageCacheCodec, codec::CRATE_SHARD_CONTAINER_PREFIX_BYTES},
    profile::metric,
};

impl PackageArtifactReader {
    /// Returns the compact routing directory without reading any crate DefMap payload.
    pub(crate) fn read_def_map_manifest(
        &self,
    ) -> Result<PackageDefMapsManifest, PackageCacheReadError> {
        Ok(self.def_map_index()?.manifest().clone())
    }

    /// Reads and decodes one crate's complete DefMap payload.
    pub(crate) fn read_def_map_crate(
        &self,
        crate_id: CrateId,
    ) -> Result<CrateData, PackageCacheReadError> {
        let index = self.def_map_index()?;
        let range = index.crate_range(crate_id).ok_or_else(|| {
            self.decode_error(anyhow::anyhow!(
                "DefMap manifest has no crate {:?}",
                crate_id,
            ))
        })?;
        let bytes = self.read_nested_range("def_map.crate", self.inner.layout.def_map, range)?;
        let package = rg_def_map::PackageSlot(
            usize::try_from(self.inner.probe.header.package.package.0).map_err(|error| {
                self.decode_error(anyhow::anyhow!(
                    "cached package slot does not fit usize: {error}"
                ))
            })?,
        );
        let started = Instant::now();
        let decoded = self
            .decode_with_names(|| {
                PackageCacheCodec::decode_def_map_crate(
                    &bytes,
                    index.manifest(),
                    CrateRef { package, crate_id },
                )
            })
            .map_err(|error| self.decode_error(error));
        metric::CACHE_SECTION_DECODE.record("def_map.crate", started.elapsed());
        decoded
    }

    /// Reconstruct the complete package for broad cache diagnostics and compatibility paths.
    #[cfg(test)]
    pub(crate) fn read_def_map(&self) -> Result<PackageDefMaps, PackageCacheReadError> {
        let manifest = self.read_def_map_manifest()?;
        let crates = (0..manifest.crates().len())
            .map(|crate_idx| self.read_def_map_crate(CrateId(crate_idx)))
            .collect::<Result<Vec<_>, _>>()?;
        PackageDefMaps::from_storage_parts(&manifest, crates)
            .map_err(|error| self.decode_error(error))
    }

    /// Initializes the DefMap directory with a prefix read followed by one bounded manifest read.
    fn def_map_index(
        &self,
    ) -> Result<&crate::cache::codec::PackageDefMapCacheIndex, PackageCacheReadError> {
        if self.inner.def_map_index.get().is_none() {
            let section = self.inner.layout.def_map;
            let index = self.read_nested_index(
                "DefMap",
                "def_map.manifest",
                section,
                CRATE_SHARD_CONTAINER_PREFIX_BYTES,
                PackageCacheCodec::decode_def_map_prefix,
                PackageCacheCodec::decode_def_map_index,
            )?;
            let _ = self.inner.def_map_index.set(index);
        }
        Ok(self
            .inner
            .def_map_index
            .get()
            .expect("DefMap index cell should be initialized after successful load"))
    }
}
