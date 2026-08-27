//! Shared framing for phase sections split into dense crate payloads.
//!
//! DefMap and Semantic IR have different logical manifests, but their physical storage is the
//! same: a small manifest followed by one range per `CrateId`. A range can be a decode unit, as in
//! DefMap, or another nested container, as in Semantic IR. This module owns only that byte-level
//! shape; phase modules keep their validation and engine types.
//!
//! ```text
//! phase magic | manifest length | encoded manifest | crate 0 | crate 1 | ...
//! ```
//!
//! Ranges stored in the manifest are relative to the first crate byte. A validated index translates
//! them into ranges relative to the start of the complete phase section.

use anyhow::Context as _;
use rg_ir_model::CrateId;

use super::{PACKAGE_CACHE_DECODE_LIMIT_BYTES, PackageCacheSectionRange};

/// Bytes occupied by the phase magic and encoded-manifest length.
pub(crate) const CRATE_SHARD_CONTAINER_PREFIX_BYTES: usize = 8 + size_of::<u64>();

/// Whether one crate range is decoded directly or contains its own bounded parts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CrateShardPayloadShape {
    /// A crate range is one wincode value and must satisfy the per-decode allocation limit.
    DecodeUnit,
    /// A crate range has another directory whose individually decoded parts enforce the limit.
    NestedContainer,
}

/// Encoded crate-shard section kept as write-ready fragments.
///
/// The manifest cannot be encoded until all crate ranges are known. Keeping the prefix, manifest,
/// and concatenated payload separate avoids joining them into a second phase-sized buffer.
#[derive(Debug)]
pub(super) struct EncodedCrateShards {
    prefix: [u8; CRATE_SHARD_CONTAINER_PREFIX_BYTES],
    manifest: Vec<u8>,
    payload: Vec<u8>,
    encoded_len: usize,
}

impl EncodedCrateShards {
    /// Encodes all dense crate slots and then builds the manifest that points at them.
    ///
    /// `encode_crate` appends one logical crate to the shared payload. Once every relative range is
    /// known, `encode_manifest` combines those ranges with the phase's engine-level directory.
    pub(super) fn encode(
        magic: [u8; 8],
        label: &'static str,
        crate_count: usize,
        payload_shape: CrateShardPayloadShape,
        mut encode_crate: impl FnMut(usize, &mut Vec<u8>) -> anyhow::Result<()>,
        encode_manifest: impl FnOnce(Vec<PackageCacheSectionRange>) -> anyhow::Result<Vec<u8>>,
    ) -> anyhow::Result<Self> {
        // Crate bytes come first in memory so each callback can append directly and its start/end
        // positions become a range relative to the concatenated payload.
        let mut payload = Vec::new();
        let mut ranges = Vec::with_capacity(crate_count);
        for crate_idx in 0..crate_count {
            let start = payload.len();
            encode_crate(crate_idx, &mut payload)
                .with_context(|| format!("while attempting to encode {label} crate {crate_idx}"))?;
            let len = payload
                .len()
                .checked_sub(start)
                .context("crate payload end precedes its start")?;
            if matches!(payload_shape, CrateShardPayloadShape::DecodeUnit) {
                anyhow::ensure!(
                    len <= PACKAGE_CACHE_DECODE_LIMIT_BYTES,
                    "package cache {label} crate has {len} bytes, limit is {PACKAGE_CACHE_DECODE_LIMIT_BYTES}",
                );
            }
            ranges.push(PackageCacheSectionRange {
                offset: u64::try_from(start).context("crate payload offset does not fit u64")?,
                len: u64::try_from(len).context("crate payload length does not fit u64")?,
            });
        }

        // The logical manifest is encoded only after physical crate ranges are final.
        let manifest = encode_manifest(ranges)
            .with_context(|| format!("while attempting to encode {label} crate manifest"))?;
        anyhow::ensure!(
            manifest.len() <= PACKAGE_CACHE_DECODE_LIMIT_BYTES,
            "package cache {label} manifest has {} bytes, limit is {PACKAGE_CACHE_DECODE_LIMIT_BYTES}",
            manifest.len(),
        );
        // The fixed prefix makes the manifest discoverable with one bounded read.
        let manifest_len = u64::try_from(manifest.len())
            .context("crate-shard manifest length does not fit u64")?;
        let encoded_len = CRATE_SHARD_CONTAINER_PREFIX_BYTES
            .checked_add(manifest.len())
            .and_then(|len| len.checked_add(payload.len()))
            .context("crate-shard section length overflows usize")?;

        let mut prefix = [0_u8; CRATE_SHARD_CONTAINER_PREFIX_BYTES];
        prefix[..magic.len()].copy_from_slice(&magic);
        prefix[magic.len()..].copy_from_slice(&manifest_len.to_le_bytes());
        Ok(Self {
            prefix,
            manifest,
            payload,
            encoded_len,
        })
    }

    pub(super) fn encoded_len(&self) -> usize {
        self.encoded_len
    }

    pub(super) fn write_to(&self, writer: &mut impl std::io::Write) -> std::io::Result<()> {
        for fragment in [&self.prefix[..], &self.manifest, &self.payload] {
            writer.write_all(fragment)?;
        }
        Ok(())
    }
}

/// Validated logical manifest and byte directory for one crate-sharded phase.
///
/// Serialized crate ranges are relative to the payload after the manifest. [`Self::crate_range`]
/// returns ranges relative to the complete phase section so artifact readers can nest them under
/// the phase's absolute file range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CrateShardCacheIndex<M> {
    manifest: M,
    crates: Vec<PackageCacheSectionRange>,
    payload_offset: u64,
}

impl<M> CrateShardCacheIndex<M> {
    /// Validates that dense crate ranges cover the payload once, in order, with no gaps.
    ///
    /// Direct decode units are also checked against the allocation bound here. Nested containers
    /// enforce that bound on their smaller inner ranges when their own prefix is decoded.
    pub(super) fn new(
        manifest: M,
        crates: Vec<PackageCacheSectionRange>,
        manifest_len: usize,
        section_len: u64,
        label: &'static str,
        payload_shape: CrateShardPayloadShape,
    ) -> anyhow::Result<Self> {
        let payload_offset = u64::try_from(CRATE_SHARD_CONTAINER_PREFIX_BYTES)
            .expect("crate-shard prefix length should fit u64")
            .checked_add(
                u64::try_from(manifest_len)
                    .context("crate-shard manifest length does not fit u64")?,
            )
            .context("crate-shard payload offset overflows u64")?;
        anyhow::ensure!(
            payload_offset <= section_len,
            "package cache {label} manifest ends at byte {payload_offset}, section has {section_len} bytes",
        );

        let payload_len = section_len - payload_offset;
        let mut next_offset = 0_u64;
        for range in &crates {
            anyhow::ensure!(
                range.offset == next_offset,
                "package cache {label} crate range starts at byte {}, expected {next_offset}",
                range.offset,
            );
            if matches!(payload_shape, CrateShardPayloadShape::DecodeUnit) {
                anyhow::ensure!(
                    range.len <= PACKAGE_CACHE_DECODE_LIMIT_BYTES as u64,
                    "package cache {label} crate has {} bytes, limit is {PACKAGE_CACHE_DECODE_LIMIT_BYTES}",
                    range.len,
                );
            }
            next_offset = next_offset
                .checked_add(range.len)
                .context("crate payload ranges overflow u64")?;
        }
        anyhow::ensure!(
            next_offset == payload_len,
            "package cache {label} crate ranges end at byte {next_offset}, payload has {payload_len} bytes",
        );

        Ok(Self {
            manifest,
            crates,
            payload_offset,
        })
    }

    pub(crate) fn manifest(&self) -> &M {
        &self.manifest
    }

    pub(crate) fn crate_range(&self, crate_id: CrateId) -> Option<PackageCacheSectionRange> {
        self.crates
            .get(crate_id.0)
            .map(|range| PackageCacheSectionRange {
                offset: self
                    .payload_offset
                    .checked_add(range.offset)
                    .expect("validated crate payload range should not overflow"),
                len: range.len,
            })
    }
}

/// Validates a crate-shard prefix and returns the encoded-manifest length.
///
/// This intentionally does not decode the manifest. Artifact readers use the returned length to
/// perform the second bounded read before passing those bytes to the phase-specific codec.
pub(super) fn decode_crate_shard_prefix(
    prefix: &[u8],
    magic: [u8; 8],
    label: &'static str,
) -> anyhow::Result<usize> {
    anyhow::ensure!(
        prefix.len() == CRATE_SHARD_CONTAINER_PREFIX_BYTES,
        "package cache {label} prefix has {} bytes, expected {CRATE_SHARD_CONTAINER_PREFIX_BYTES}",
        prefix.len(),
    );
    anyhow::ensure!(
        prefix[..magic.len()] == magic,
        "package cache {label} container magic is invalid",
    );
    let manifest_len = u64::from_le_bytes(
        prefix[magic.len()..]
            .try_into()
            .expect("fixed crate-shard manifest length should contain eight bytes"),
    );
    let manifest_len =
        usize::try_from(manifest_len).context("crate-shard manifest length does not fit usize")?;
    anyhow::ensure!(
        manifest_len <= PACKAGE_CACHE_DECODE_LIMIT_BYTES,
        "package cache {label} manifest has {manifest_len} bytes, limit is {PACKAGE_CACHE_DECODE_LIMIT_BYTES}",
    );
    Ok(manifest_len)
}
