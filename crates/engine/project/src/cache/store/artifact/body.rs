//! Reads the nested Body IR directory and its independently encoded storage units.
//!
//! The outer artifact layout gives the reader one large Body IR range. The first Body IR request
//! reads a fixed nested prefix and the manifest following it, then caches the validated directory
//! in `body_index`. Later requests use that directory to read one file shard.
//!
//! Ranges in the decoded index are relative to the Body IR section. The shared nested-range reader
//! checks them against that section and translates them into outer-file offsets.

use std::time::Instant;

use rg_body_ir::{BodyFileShard, CrateBodies, PackageBodiesManifest};
use rg_ir_model::CrateId;
use rg_parse::FileId;

use super::{PackageArtifactReader, PackageCacheReadError};
use crate::{
    cache::{PackageCacheCodec, codec::BODY_CACHE_CONTAINER_PREFIX_BYTES},
    profile::metric,
};

impl PackageArtifactReader {
    /// Return the logical body-to-file directory without decoding crate or file payloads.
    ///
    /// The returned value is cloned out of the cached physical index so the Body IR transaction can
    /// keep it as its request-local routing table.
    pub(crate) fn read_body_ir_manifest(
        &self,
    ) -> Result<PackageBodiesManifest, PackageCacheReadError> {
        Ok(self.body_index()?.manifest().clone())
    }

    /// Read one source file's bodies and validate them against the crate manifest.
    ///
    /// Both crate and file must be declared by the directory. Their absence is malformed cache
    /// state, not an empty Body IR result.
    pub(crate) fn read_body_file_shard(
        &self,
        crate_id: CrateId,
        file: FileId,
    ) -> Result<BodyFileShard, PackageCacheReadError> {
        let index = self.body_index()?;
        let crate_manifest = index.manifest().crate_manifest(crate_id).ok_or_else(|| {
            self.decode_error(anyhow::anyhow!(
                "Body IR manifest has no crate {:?}",
                crate_id,
            ))
        })?;
        let range = index.file_range(crate_id, file).ok_or_else(|| {
            self.decode_error(anyhow::anyhow!(
                "Body IR manifest has no file {:?} in crate {:?}",
                file,
                crate_id,
            ))
        })?;
        let bytes = self.read_nested_range("body_ir.file", self.inner.layout.body_ir, range)?;
        let started = Instant::now();
        let decoded = self
            .decode_with_names(|| {
                PackageCacheCodec::decode_body_file_shard(&bytes, crate_manifest, file)
            })
            .map_err(|error| self.decode_error(error));
        metric::CACHE_SECTION_DECODE.record("body_ir.file", started.elapsed());
        decoded
    }

    /// Read one validated encoded shard for a package artifact rewrite.
    ///
    /// Exact target materialization uses this path for untouched siblings. Their logical manifest
    /// has already been decoded and validated, while their potentially large body arenas never
    /// need to be reconstructed just to copy the bytes into the replacement package artifact.
    pub(crate) fn read_encoded_body_file_shard(
        &self,
        crate_id: CrateId,
        file: FileId,
    ) -> Result<Vec<u8>, PackageCacheReadError> {
        let index = self.body_index()?;
        let crate_manifest = index.manifest().crate_manifest(crate_id).ok_or_else(|| {
            self.decode_error(anyhow::anyhow!(
                "Body IR manifest has no crate {:?}",
                crate_id,
            ))
        })?;
        if !crate_manifest.files().contains(&file) {
            return Err(self.decode_error(anyhow::anyhow!(
                "Body IR manifest has no file {:?} in crate {:?}",
                file,
                crate_id,
            )));
        }
        let range = index.file_range(crate_id, file).ok_or_else(|| {
            self.decode_error(anyhow::anyhow!(
                "Body IR directory has no file {:?} in crate {:?}",
                file,
                crate_id,
            ))
        })?;
        self.read_nested_range("body_ir.file.copy", self.inner.layout.body_ir, range)
    }

    /// Reconstruct a complete resident crate from every declared file shard.
    ///
    /// This is the intentionally broad loading path used when a caller asks for `CrateBodies`
    /// rather than a file-local view.
    pub(crate) fn read_body_crate(
        &self,
        crate_id: CrateId,
    ) -> Result<CrateBodies, PackageCacheReadError> {
        let index = self.body_index()?;
        let crate_manifest = index.manifest().crate_manifest(crate_id).ok_or_else(|| {
            self.decode_error(anyhow::anyhow!(
                "Body IR manifest has no crate {:?}",
                crate_id,
            ))
        })?;
        let shards = crate_manifest
            .files()
            .iter()
            .map(|&file| self.read_body_file_shard(crate_id, file))
            .collect::<Result<Vec<_>, _>>()?;
        CrateBodies::from_storage_parts(crate_manifest, shards)
            .map_err(|error| self.decode_error(error))
    }

    /// Load and validate the nested Body IR directory once for this artifact revision.
    ///
    /// The manifest is variable-sized, so it takes two reads: the fixed prefix tells us how many
    /// manifest bytes to fetch. Payload bytes remain untouched until another method asks for them.
    fn body_index(
        &self,
    ) -> Result<&crate::cache::codec::PackageBodyCacheIndex, PackageCacheReadError> {
        if self.inner.body_index.get().is_none() {
            let section = self.inner.layout.body_ir;
            let index = self.read_nested_index(
                "Body IR",
                "body_ir.manifest",
                section,
                BODY_CACHE_CONTAINER_PREFIX_BYTES,
                PackageCacheCodec::decode_body_prefix,
                PackageCacheCodec::decode_body_index,
            )?;
            // A failed attempt leaves the cell empty, so no partially validated directory becomes
            // visible to later reads.
            let _ = self.inner.body_index.set(index);
        }
        Ok(self
            .inner
            .body_index
            .get()
            .expect("Body IR index cell should be initialized after successful load"))
    }
}
