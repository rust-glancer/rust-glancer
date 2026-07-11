//! Reads the nested Body IR directory and its independently encoded storage units.
//!
//! The outer artifact layout gives the reader one large Body IR range. The first Body IR request
//! reads a fixed nested prefix and the manifest following it, then caches the validated directory
//! in `body_index`. Later requests use that directory to read one target index or file shard.
//!
//! Ranges in the decoded index are relative to the Body IR section. `read_body_range` checks them
//! against that section and translates them into outer-file offsets before using the shared reader.

use std::time::Instant;

use rg_body_ir::{BodyFileShard, PackageBodiesManifest, TargetBodies};
use rg_ir_storage::ItemLookupIndex;
use rg_parse::{FileId, TargetId};

use super::{PackageArtifactReader, PackageCacheReadError};
use crate::{
    cache::{
        PackageCacheCodec,
        codec::{BODY_CACHE_CONTAINER_PREFIX_BYTES, PackageCacheSectionRange},
    },
    profile::metric,
};

impl PackageArtifactReader {
    /// Return the logical body-to-file directory without decoding target or file payloads.
    ///
    /// The returned value is cloned out of the cached physical index so the Body IR transaction can
    /// keep it as its request-local routing table.
    pub(crate) fn read_body_ir_manifest(
        &self,
    ) -> Result<PackageBodiesManifest, PackageCacheReadError> {
        Ok(self.body_index()?.manifest().clone())
    }

    /// Read one target-global semantic index without reading its bodies.
    pub(crate) fn read_body_semantic_index(
        &self,
        target: TargetId,
    ) -> Result<ItemLookupIndex, PackageCacheReadError> {
        let range = self
            .body_index()?
            .semantic_index_range(target)
            .ok_or_else(|| {
                self.decode_error(anyhow::anyhow!(
                    "Body IR manifest has no target index for {:?}",
                    target,
                ))
            })?;
        let bytes = self.read_body_range("body_ir.target_index", range)?;
        let started = Instant::now();
        let decoded = self
            .decode_with_names(|| PackageCacheCodec::decode_body_semantic_index(&bytes))
            .map_err(|error| self.decode_error(error));
        metric::CACHE_SECTION_DECODE.record("body_ir.target_index", started.elapsed());
        decoded
    }

    /// Read one source file's bodies and validate them against the target manifest.
    ///
    /// Both target and file must be declared by the directory. Their absence is malformed cache
    /// state, not an empty Body IR result.
    pub(crate) fn read_body_file_shard(
        &self,
        target: TargetId,
        file: FileId,
    ) -> Result<BodyFileShard, PackageCacheReadError> {
        let index = self.body_index()?;
        let target_manifest = index.manifest().target(target).ok_or_else(|| {
            self.decode_error(anyhow::anyhow!(
                "Body IR manifest has no target {:?}",
                target,
            ))
        })?;
        let range = index.file_range(target, file).ok_or_else(|| {
            self.decode_error(anyhow::anyhow!(
                "Body IR manifest has no file {:?} in target {:?}",
                file,
                target,
            ))
        })?;
        let bytes = self.read_body_range("body_ir.file", range)?;
        let started = Instant::now();
        let decoded = self
            .decode_with_names(|| {
                PackageCacheCodec::decode_body_file_shard(&bytes, target_manifest, file)
            })
            .map_err(|error| self.decode_error(error));
        metric::CACHE_SECTION_DECODE.record("body_ir.file", started.elapsed());
        decoded
    }

    /// Reconstruct a complete resident target from its index and every declared file shard.
    ///
    /// This is the intentionally broad loading path used when a caller asks for `TargetBodies`
    /// rather than a file-local view.
    pub(crate) fn read_body_target(
        &self,
        target: TargetId,
    ) -> Result<TargetBodies, PackageCacheReadError> {
        let index = self.body_index()?;
        let target_manifest = index.manifest().target(target).ok_or_else(|| {
            self.decode_error(anyhow::anyhow!(
                "Body IR manifest has no target {:?}",
                target,
            ))
        })?;
        let semantic_index = self.read_body_semantic_index(target)?;
        let shards = target_manifest
            .files()
            .iter()
            .map(|&file| self.read_body_file_shard(target, file))
            .collect::<Result<Vec<_>, _>>()?;
        TargetBodies::from_storage_parts(target_manifest, semantic_index, shards)
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
            // 1. The outer layout bounds the complete Body IR section. It must be large enough to
            // contain the nested magic and manifest-length field.
            let body_section = self.inner.layout.body_ir;
            let prefix_len = BODY_CACHE_CONTAINER_PREFIX_BYTES as u64;
            if body_section.len < prefix_len {
                return Err(self.decode_error(anyhow::anyhow!(
                    "Body IR section is shorter than its {BODY_CACHE_CONTAINER_PREFIX_BYTES}-byte prefix"
                )));
            }
            // 2. Read only the fixed nested prefix and discover the variable manifest length.
            let prefix = self.read_body_range(
                "body_ir.manifest",
                PackageCacheSectionRange {
                    offset: 0,
                    len: prefix_len,
                },
            )?;
            let manifest_len = PackageCacheCodec::decode_body_prefix(&prefix)
                .map_err(|error| self.decode_error(error))?;
            let manifest_len = u64::try_from(manifest_len)
                .map_err(|error| self.decode_error(anyhow::anyhow!(error)))?;
            // 3. Check the declared manifest against the outer Body IR section before allocating
            // or reading its bytes.
            let manifest_end = prefix_len.checked_add(manifest_len).ok_or_else(|| {
                self.decode_error(anyhow::anyhow!("Body IR manifest overflows u64"))
            })?;
            if manifest_end > body_section.len {
                return Err(self.decode_error(anyhow::anyhow!(
                    "Body IR manifest ends at byte {manifest_end}, section has {} bytes",
                    body_section.len,
                )));
            }
            // 4. Decode and validate the directory. A failed attempt leaves `body_index` empty, so
            // no partially validated state becomes visible to later reads.
            let bytes = self.read_body_range(
                "body_ir.manifest",
                PackageCacheSectionRange {
                    offset: prefix_len,
                    len: manifest_len,
                },
            )?;
            let started = Instant::now();
            let index =
                PackageCacheCodec::decode_body_index(&bytes, body_section.len, &self.inner.probe)
                    .map_err(|error| self.decode_error(error))?;
            metric::CACHE_SECTION_DECODE.record("body_ir.manifest", started.elapsed());
            let _ = self.inner.body_index.set(index);
        }
        Ok(self
            .inner
            .body_index
            .get()
            .expect("Body IR index cell should be initialized after successful load"))
    }

    /// Validate a Body IR section-relative range and translate it to an outer-file read.
    fn read_body_range(
        &self,
        label: &'static str,
        range: PackageCacheSectionRange,
    ) -> Result<Vec<u8>, PackageCacheReadError> {
        let body = self.inner.layout.body_ir;
        let end = range
            .offset
            .checked_add(range.len)
            .ok_or_else(|| self.decode_error(anyhow::anyhow!("Body IR range overflows u64")))?;
        if end > body.len {
            return Err(self.decode_error(anyhow::anyhow!(
                "Body IR range ends at byte {end}, section has {} bytes",
                body.len,
            )));
        }
        let offset = body.offset.checked_add(range.offset).ok_or_else(|| {
            self.decode_error(anyhow::anyhow!("Body IR file offset overflows u64"))
        })?;
        self.read_section(
            label,
            PackageCacheSectionRange {
                offset,
                len: range.len,
            },
        )
    }
}
