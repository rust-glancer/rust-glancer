//! Crate-granular Semantic IR section codec.
//!
//! The outer directory assigns one range to each dense crate slot. A crate range is itself a small
//! container:
//!
//! ```text
//! items length | lookup-index length | encoded ItemStore | encoded ItemLookupIndex
//! ```
//!
//! Known-item queries can read declarations alone. Visibility-wide name lookup can read the smaller
//! index alone, even when it composes indexes from several dependency crates.

use anyhow::Context as _;
use rg_ir_model::CrateId;
#[cfg(test)]
use rg_semantic_ir::CrateIr;
use rg_semantic_ir::{ItemLookupIndex, ItemStore, PackageIr, PackageIrManifest};
use wincode::{SchemaRead, SchemaWrite};

#[cfg(test)]
use super::crate_shards::CRATE_SHARD_CONTAINER_PREFIX_BYTES;
use super::{
    PackageCacheCodec, PackageCacheProbe, PackageCacheSectionRange,
    crate_shards::{
        CrateShardCacheIndex, CrateShardPayloadShape, EncodedCrateShards, decode_crate_shard_prefix,
    },
};

const SEMANTIC_IR_CACHE_CONTAINER_MAGIC: [u8; 8] = *b"RGSEM\0\0\x01";
/// Bytes occupied by the item and lookup-index length fields inside one crate shard.
pub(crate) const SEMANTIC_IR_CRATE_PREFIX_BYTES: usize = size_of::<u64>() * 2;

/// Validated Semantic IR package directory paired with the physical range of every crate shard.
pub(crate) type PackageSemanticIrCacheIndex = CrateShardCacheIndex<PackageIrManifest>;

/// Serialized Semantic IR directory: the logical crate count plus physical crate ranges.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite)]
struct PackageSemanticIrCacheManifest {
    semantic_ir: PackageIrManifest,
    crates: Vec<PackageCacheSectionRange>,
}

/// Validated ranges inside one Semantic IR crate shard.
///
/// Both ranges are relative to the start of that crate shard. Artifact readers nest them under the
/// crate's section-relative range before converting them to absolute file offsets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SemanticIrCrateCacheIndex {
    items: PackageCacheSectionRange,
    lookup_index: PackageCacheSectionRange,
}

impl SemanticIrCrateCacheIndex {
    pub(crate) fn items(self) -> PackageCacheSectionRange {
        self.items
    }

    pub(crate) fn lookup_index(self) -> PackageCacheSectionRange {
        self.lookup_index
    }
}

impl PackageCacheCodec {
    /// Encodes each crate as independently readable declaration and lookup-index payloads.
    pub(super) fn encode_semantic_ir(
        semantic_ir: &PackageIr,
    ) -> anyhow::Result<EncodedCrateShards> {
        let logical_manifest = semantic_ir.manifest();
        EncodedCrateShards::encode(
            SEMANTIC_IR_CACHE_CONTAINER_MAGIC,
            "Semantic IR",
            semantic_ir.crates().len(),
            CrateShardPayloadShape::NestedContainer,
            |crate_idx, payload| {
                // Item data is opened by exact declaration reads, while the smaller lookup index
                // is composed across visible dependencies. Keep both under one crate shard but
                // frame them independently so either query can read only its half.
                let prefix_start = payload.len();
                payload.resize(prefix_start + SEMANTIC_IR_CRATE_PREFIX_BYTES, 0);
                let items_start = payload.len();
                wincode::config::serialize_into(
                    &mut *payload,
                    semantic_ir.crates()[crate_idx].items(),
                    Self::wincode_config(),
                )
                .map_err(|error| anyhow::anyhow!("{error}"))
                .context("while attempting to serialize package cache Semantic IR items")?;
                let items_len = payload
                    .len()
                    .checked_sub(items_start)
                    .context("Semantic IR item payload end precedes its start")?;
                let lookup_start = payload.len();
                wincode::config::serialize_into(
                    &mut *payload,
                    semantic_ir.crates()[crate_idx].lookup_index(),
                    Self::wincode_config(),
                )
                .map_err(|error| anyhow::anyhow!("{error}"))
                .context("while attempting to serialize package cache Semantic IR lookup index")?;
                let lookup_len = payload
                    .len()
                    .checked_sub(lookup_start)
                    .context("Semantic IR lookup payload end precedes its start")?;
                anyhow::ensure!(
                    items_len <= super::PACKAGE_CACHE_DECODE_LIMIT_BYTES,
                    "package cache Semantic IR items have {items_len} bytes, limit is {}",
                    super::PACKAGE_CACHE_DECODE_LIMIT_BYTES,
                );
                anyhow::ensure!(
                    lookup_len <= super::PACKAGE_CACHE_DECODE_LIMIT_BYTES,
                    "package cache Semantic IR lookup index has {lookup_len} bytes, limit is {}",
                    super::PACKAGE_CACHE_DECODE_LIMIT_BYTES,
                );
                let items_len = u64::try_from(items_len)
                    .context("Semantic IR item payload length does not fit u64")?;
                let lookup_len = u64::try_from(lookup_len)
                    .context("Semantic IR lookup payload length does not fit u64")?;
                payload[prefix_start..prefix_start + size_of::<u64>()]
                    .copy_from_slice(&items_len.to_le_bytes());
                payload[prefix_start + size_of::<u64>()
                    ..prefix_start + SEMANTIC_IR_CRATE_PREFIX_BYTES]
                    .copy_from_slice(&lookup_len.to_le_bytes());
                Ok(())
            },
            |crates| {
                wincode::config::serialize(
                    &PackageSemanticIrCacheManifest {
                        semantic_ir: logical_manifest,
                        crates,
                    },
                    Self::wincode_config(),
                )
                .map_err(|error| anyhow::anyhow!("{error}"))
                .context("while attempting to serialize package cache Semantic IR manifest")
            },
        )
    }

    /// Validates the Semantic IR container prefix and returns its manifest length.
    pub(crate) fn decode_semantic_ir_prefix(prefix: &[u8]) -> anyhow::Result<usize> {
        decode_crate_shard_prefix(prefix, SEMANTIC_IR_CACHE_CONTAINER_MAGIC, "Semantic IR")
    }

    /// Decodes and validates the package directory and its physical crate ranges.
    pub(crate) fn decode_semantic_ir_index(
        manifest_bytes: &[u8],
        section_len: u64,
        probe: &PackageCacheProbe,
    ) -> anyhow::Result<PackageSemanticIrCacheIndex> {
        let manifest = wincode::config::deserialize_exact::<PackageSemanticIrCacheManifest, _>(
            manifest_bytes,
            Self::wincode_config(),
        )
        .map_err(|error| anyhow::anyhow!("{error}"))
        .context("while attempting to deserialize package cache Semantic IR manifest")?;
        Self::validate_semantic_ir_manifest(&manifest, probe)
            .context("validate package cache Semantic IR manifest")?;
        CrateShardCacheIndex::new(
            manifest.semantic_ir,
            manifest.crates,
            manifest_bytes.len(),
            section_len,
            "Semantic IR",
            CrateShardPayloadShape::NestedContainer,
        )
    }

    /// Validates one crate prefix and returns the ranges of its two independently decoded parts.
    ///
    /// The declared lengths must both satisfy the allocation bound and cover the crate shard exactly.
    pub(crate) fn decode_semantic_ir_crate_index(
        prefix: &[u8],
        crate_len: u64,
    ) -> anyhow::Result<SemanticIrCrateCacheIndex> {
        anyhow::ensure!(
            prefix.len() == SEMANTIC_IR_CRATE_PREFIX_BYTES,
            "Semantic IR crate prefix has {} bytes, expected {SEMANTIC_IR_CRATE_PREFIX_BYTES}",
            prefix.len(),
        );
        let items_len = u64::from_le_bytes(
            prefix[..size_of::<u64>()]
                .try_into()
                .expect("Semantic IR item length should contain eight bytes"),
        );
        let lookup_len = u64::from_le_bytes(
            prefix[size_of::<u64>()..]
                .try_into()
                .expect("Semantic IR lookup length should contain eight bytes"),
        );
        for (label, len) in [("items", items_len), ("lookup index", lookup_len)] {
            anyhow::ensure!(
                len <= super::PACKAGE_CACHE_DECODE_LIMIT_BYTES as u64,
                "package cache Semantic IR {label} has {len} bytes, limit is {}",
                super::PACKAGE_CACHE_DECODE_LIMIT_BYTES,
            );
        }
        let items_offset = SEMANTIC_IR_CRATE_PREFIX_BYTES as u64;
        let lookup_offset = items_offset
            .checked_add(items_len)
            .context("Semantic IR lookup offset overflows u64")?;
        let end = lookup_offset
            .checked_add(lookup_len)
            .context("Semantic IR crate ranges overflow u64")?;
        anyhow::ensure!(
            end == crate_len,
            "Semantic IR crate ranges end at byte {end}, shard has {crate_len} bytes",
        );
        Ok(SemanticIrCrateCacheIndex {
            items: PackageCacheSectionRange {
                offset: items_offset,
                len: items_len,
            },
            lookup_index: PackageCacheSectionRange {
                offset: lookup_offset,
                len: lookup_len,
            },
        })
    }

    /// Decodes declarations for one valid crate slot.
    pub(crate) fn decode_semantic_ir_items(
        bytes: &[u8],
        manifest: PackageIrManifest,
        crate_id: CrateId,
    ) -> anyhow::Result<ItemStore> {
        anyhow::ensure!(
            crate_id.0 < manifest.crate_count(),
            "Semantic IR manifest has no crate {:?}",
            crate_id,
        );
        wincode::config::deserialize_exact::<ItemStore, _>(bytes, Self::wincode_config())
            .map_err(|error| anyhow::anyhow!("{error}"))
            .context("while attempting to deserialize package cache Semantic IR items")
    }

    /// Decodes the visibility lookup index for one valid crate slot.
    pub(crate) fn decode_semantic_ir_lookup_index(
        bytes: &[u8],
        manifest: PackageIrManifest,
        crate_id: CrateId,
    ) -> anyhow::Result<ItemLookupIndex> {
        anyhow::ensure!(
            crate_id.0 < manifest.crate_count(),
            "Semantic IR manifest has no crate {:?}",
            crate_id,
        );
        wincode::config::deserialize_exact::<ItemLookupIndex, _>(bytes, Self::wincode_config())
            .map_err(|error| anyhow::anyhow!("{error}"))
            .context("while attempting to deserialize package cache Semantic IR lookup index")
    }

    #[cfg(test)]
    pub(crate) fn decode_semantic_ir(
        bytes: &[u8],
        probe: &PackageCacheProbe,
    ) -> anyhow::Result<PackageIr> {
        let (manifest_bytes, section_len) = Self::semantic_ir_manifest_bytes(bytes)
            .context("read package cache Semantic IR manifest bytes")?;
        let index = Self::decode_semantic_ir_index(manifest_bytes, section_len, probe)
            .context("decode package cache Semantic IR index")?;
        let manifest = *index.manifest();
        let mut crates = Vec::with_capacity(manifest.crate_count());
        for crate_idx in 0..manifest.crate_count() {
            let crate_id = CrateId(crate_idx);
            let range = index
                .crate_range(crate_id)
                .context("Semantic IR cache index should contain every manifest crate")?;
            let shard = Self::section_slice(bytes, range, "Semantic IR crate")
                .context("read package cache Semantic IR crate bytes")?;
            anyhow::ensure!(
                shard.len() >= SEMANTIC_IR_CRATE_PREFIX_BYTES,
                "Semantic IR crate is shorter than its prefix",
            );
            let crate_index = Self::decode_semantic_ir_crate_index(
                &shard[..SEMANTIC_IR_CRATE_PREFIX_BYTES],
                u64::try_from(shard.len()).context("Semantic IR crate length does not fit u64")?,
            )
            .with_context(|| format!("decode package cache Semantic IR crate {crate_idx} index"))?;
            let items = Self::section_slice(shard, crate_index.items(), "Semantic IR items")
                .context("read package cache Semantic IR item bytes")?;
            let lookup_index = Self::section_slice(
                shard,
                crate_index.lookup_index(),
                "Semantic IR lookup index",
            )
            .context("read package cache Semantic IR lookup bytes")?;
            crates.push(CrateIr::from_storage_parts(
                Self::decode_semantic_ir_items(items, manifest, crate_id)
                    .with_context(|| format!("decode Semantic IR items for crate {crate_idx}"))?,
                Self::decode_semantic_ir_lookup_index(lookup_index, manifest, crate_id)
                    .with_context(|| {
                        format!("decode Semantic IR lookup index for crate {crate_idx}")
                    })?,
            ));
        }
        PackageIr::from_storage_parts(manifest, crates)
    }

    #[cfg(test)]
    fn semantic_ir_manifest_bytes(bytes: &[u8]) -> anyhow::Result<(&[u8], u64)> {
        anyhow::ensure!(
            bytes.len() >= CRATE_SHARD_CONTAINER_PREFIX_BYTES,
            "package cache Semantic IR section is shorter than its prefix",
        );
        let manifest_len =
            Self::decode_semantic_ir_prefix(&bytes[..CRATE_SHARD_CONTAINER_PREFIX_BYTES])
                .context("decode package cache Semantic IR prefix")?;
        let manifest_end = CRATE_SHARD_CONTAINER_PREFIX_BYTES
            .checked_add(manifest_len)
            .context("Semantic IR manifest end overflows usize")?;
        anyhow::ensure!(
            manifest_end <= bytes.len(),
            "package cache Semantic IR manifest ends at byte {manifest_end}, section has {} bytes",
            bytes.len(),
        );
        Ok((
            &bytes[CRATE_SHARD_CONTAINER_PREFIX_BYTES..manifest_end],
            u64::try_from(bytes.len()).context("Semantic IR section length does not fit u64")?,
        ))
    }

    fn validate_semantic_ir_manifest(
        manifest: &PackageSemanticIrCacheManifest,
        probe: &PackageCacheProbe,
    ) -> anyhow::Result<()> {
        let crate_count = probe.header.package.targets.len();
        anyhow::ensure!(
            manifest.semantic_ir.crate_count() == crate_count,
            "package cache Semantic IR manifest has {} crates but header has {crate_count} Cargo targets",
            manifest.semantic_ir.crate_count(),
        );
        anyhow::ensure!(
            manifest.crates.len() == crate_count,
            "package cache Semantic IR directory has {} crates but header has {crate_count} Cargo targets",
            manifest.crates.len(),
        );
        Ok(())
    }
}
