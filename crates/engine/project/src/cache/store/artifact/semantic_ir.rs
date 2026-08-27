//! Reads the nested Semantic IR directory and exact crate parts.
//!
//! The package directory locates a crate shard. Its short prefix then locates declarations and the
//! lookup index inside that shard, so either part can be read without allocating the other.

use std::time::Instant;

use rg_ir_model::CrateId;
#[cfg(test)]
use rg_semantic_ir::{CrateIr, PackageIr};
use rg_semantic_ir::{ItemLookupIndex, ItemStore, PackageIrManifest};

use super::{PackageArtifactReader, PackageCacheReadError};
use crate::{
    cache::{
        PackageCacheCodec,
        codec::{
            CRATE_SHARD_CONTAINER_PREFIX_BYTES, PackageCacheSectionRange,
            SEMANTIC_IR_CRATE_PREFIX_BYTES, SemanticIrCrateCacheIndex,
        },
    },
    profile::metric,
};

impl PackageArtifactReader {
    /// Returns the compact crate directory without reading any crate payload.
    pub(crate) fn read_semantic_ir_manifest(
        &self,
    ) -> Result<PackageIrManifest, PackageCacheReadError> {
        Ok(*self.semantic_ir_index()?.manifest())
    }

    /// Reads declarations for one crate without reading its lookup index.
    pub(crate) fn read_semantic_ir_items(
        &self,
        crate_id: CrateId,
    ) -> Result<ItemStore, PackageCacheReadError> {
        let (manifest, crate_range, crate_index) = self.semantic_ir_crate_index(crate_id)?;
        let range = self.semantic_ir_crate_part_range(crate_range, crate_index.items())?;
        let bytes =
            self.read_nested_range("semantic_ir.items", self.inner.layout.semantic_ir, range)?;
        let started = Instant::now();
        let decoded = self
            .decode_with_names(|| {
                PackageCacheCodec::decode_semantic_ir_items(&bytes, manifest, crate_id)
            })
            .map_err(|error| self.decode_error(error));
        metric::CACHE_SECTION_DECODE.record("semantic_ir.items", started.elapsed());
        decoded
    }

    /// Reads the visibility lookup index for one crate without reading its declarations.
    pub(crate) fn read_semantic_ir_lookup_index(
        &self,
        crate_id: CrateId,
    ) -> Result<ItemLookupIndex, PackageCacheReadError> {
        let (manifest, crate_range, crate_index) = self.semantic_ir_crate_index(crate_id)?;
        let range = self.semantic_ir_crate_part_range(crate_range, crate_index.lookup_index())?;
        let bytes = self.read_nested_range(
            "semantic_ir.lookup_index",
            self.inner.layout.semantic_ir,
            range,
        )?;
        let started = Instant::now();
        let decoded = self
            .decode_with_names(|| {
                PackageCacheCodec::decode_semantic_ir_lookup_index(&bytes, manifest, crate_id)
            })
            .map_err(|error| self.decode_error(error));
        metric::CACHE_SECTION_DECODE.record("semantic_ir.lookup_index", started.elapsed());
        decoded
    }

    /// Reconstruct the complete package for broad cache diagnostics and compatibility paths.
    #[cfg(test)]
    pub(crate) fn read_semantic_ir(&self) -> Result<PackageIr, PackageCacheReadError> {
        let manifest = self.read_semantic_ir_manifest()?;
        let crates = (0..manifest.crate_count())
            .map(|crate_idx| {
                let crate_id = CrateId(crate_idx);
                Ok(CrateIr::from_storage_parts(
                    self.read_semantic_ir_items(crate_id)?,
                    self.read_semantic_ir_lookup_index(crate_id)?,
                ))
            })
            .collect::<Result<Vec<_>, _>>()?;
        PackageIr::from_storage_parts(manifest, crates).map_err(|error| self.decode_error(error))
    }

    /// Reads and validates the short inner directory for one crate shard.
    ///
    /// The returned package manifest, outer crate range, and inner ranges share one validated
    /// artifact revision and can therefore be composed without reopening the cache path.
    fn semantic_ir_crate_index(
        &self,
        crate_id: CrateId,
    ) -> Result<
        (
            PackageIrManifest,
            PackageCacheSectionRange,
            SemanticIrCrateCacheIndex,
        ),
        PackageCacheReadError,
    > {
        let index = self.semantic_ir_index()?;
        let crate_range = index.crate_range(crate_id).ok_or_else(|| {
            self.decode_error(anyhow::anyhow!(
                "Semantic IR manifest has no crate {:?}",
                crate_id,
            ))
        })?;
        let prefix_len = SEMANTIC_IR_CRATE_PREFIX_BYTES as u64;
        if crate_range.len < prefix_len {
            return Err(self.decode_error(anyhow::anyhow!(
                "Semantic IR crate is shorter than its {SEMANTIC_IR_CRATE_PREFIX_BYTES}-byte prefix"
            )));
        }
        let prefix = self.read_nested_range(
            "semantic_ir.crate_prefix",
            self.inner.layout.semantic_ir,
            PackageCacheSectionRange {
                offset: crate_range.offset,
                len: prefix_len,
            },
        )?;
        let crate_index =
            PackageCacheCodec::decode_semantic_ir_crate_index(&prefix, crate_range.len)
                .map_err(|error| self.decode_error(error))?;
        Ok((*index.manifest(), crate_range, crate_index))
    }

    /// Translates a crate-relative part range into Semantic-IR-section coordinates.
    fn semantic_ir_crate_part_range(
        &self,
        crate_range: PackageCacheSectionRange,
        part: PackageCacheSectionRange,
    ) -> Result<PackageCacheSectionRange, PackageCacheReadError> {
        let offset = crate_range.offset.checked_add(part.offset).ok_or_else(|| {
            self.decode_error(anyhow::anyhow!(
                "Semantic IR crate part offset overflows u64"
            ))
        })?;
        Ok(PackageCacheSectionRange {
            offset,
            len: part.len,
        })
    }

    /// Initializes the package directory with a prefix read followed by one bounded manifest read.
    fn semantic_ir_index(
        &self,
    ) -> Result<&crate::cache::codec::PackageSemanticIrCacheIndex, PackageCacheReadError> {
        if self.inner.semantic_ir_index.get().is_none() {
            let section = self.inner.layout.semantic_ir;
            let index = self.read_nested_index(
                "Semantic IR",
                "semantic_ir.manifest",
                section,
                CRATE_SHARD_CONTAINER_PREFIX_BYTES,
                PackageCacheCodec::decode_semantic_ir_prefix,
                PackageCacheCodec::decode_semantic_ir_index,
            )?;
            let _ = self.inner.semantic_ir_index.set(index);
        }
        Ok(self
            .inner
            .semantic_ir_index
            .get()
            .expect("Semantic IR index cell should be initialized after successful load"))
    }
}
