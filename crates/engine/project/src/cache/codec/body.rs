//! Nested directory and codecs for crate indexes and file-granular Body IR payloads.
//!
//! Body IR is commonly the largest phase, and most interactive queries care about the file under
//! the cursor. Its section therefore has another container:
//!
//! ```text
//! RGBODY magic | manifest len | manifest | crate 0 index | file shard | ...
//! ```
//!
//! The manifest maps every stable body id to a source file and records the relative byte range for
//! every crate index and file shard. A file-local query reads the manifest once and then one shard.
//! Crate-global operations may read every shard or ask the loader to reconstruct a full crate.
//!
//! This is still one section of one atomically published package file. Sharding changes read and
//! decode granularity without creating a transaction protocol for hundreds of independent files.
//!
//! There are two kinds of information in the serialized manifest:
//!
//! - `PackageBodiesManifest` describes the logical Body IR shape: crates, body ids, and files.
//! - `CrateBodyCacheLayout` describes where the corresponding encoded payloads live.
//!
//! Layout ranges are relative to the payload after the manifest. The decoded
//! [`PackageBodyCacheIndex`] adds the payload offset once, so the artifact reader receives ranges
//! relative to the beginning of the Body IR section.

use anyhow::Context as _;
use rg_body_ir::{BodyFileShard, CrateBodiesManifest, PackageBodies, PackageBodiesManifest};
use rg_ir_model::CrateId;
use rg_parse::FileId;
use rg_semantic_ir::ItemLookupIndex;
use wincode::{SchemaRead, SchemaWrite};

use super::{
    PACKAGE_CACHE_SECTION_LIMIT_BYTES, PackageCacheCodec, PackageCacheProbe,
    PackageCacheSectionRange,
};

const BODY_CACHE_CONTAINER_MAGIC: [u8; 8] = *b"RGBODY\0\x01";
/// Bytes needed to discover the variable-size Body IR manifest.
pub(crate) const BODY_CACHE_CONTAINER_PREFIX_BYTES: usize = 8 + size_of::<u64>();

/// Validated Body IR directory used by lazy artifact reads.
///
/// `manifest` answers logical routing questions such as `BodyId -> FileId`. `crates` holds the
/// encoded byte ranges. `payload_offset` joins those two worlds by translating serialized
/// payload-relative ranges into ranges relative to the complete Body IR section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PackageBodyCacheIndex {
    manifest: PackageBodiesManifest,
    crates: Vec<CrateBodyCacheLayout>,
    payload_offset: u64,
}

/// Serialized Body IR directory: logical routing plus physical payload ranges.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite)]
struct PackageBodyCacheManifest {
    bodies: PackageBodiesManifest,
    crates: Vec<CrateBodyCacheLayout>,
}

/// Relative ranges for one crate's global index and source-file shards.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite)]
struct CrateBodyCacheLayout {
    semantic_index: PackageCacheSectionRange,
    files: Vec<BodyFileCacheRange>,
}

/// One source file and the relative range containing its encoded Body IR shard.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite)]
struct BodyFileCacheRange {
    file: FileId,
    range: PackageCacheSectionRange,
}

/// Encoded Body IR pieces in their final on-disk order.
///
/// Keeping the prefix, manifest, and payload separate avoids allocating another Body-IR-sized
/// vector merely to concatenate bytes that the atomic file writer can write sequentially.
#[derive(Debug)]
pub(super) struct EncodedBodyIr {
    prefix: [u8; BODY_CACHE_CONTAINER_PREFIX_BYTES],
    manifest: Vec<u8>,
    payload: Vec<u8>,
    encoded_len: usize,
}

impl EncodedBodyIr {
    pub(super) fn encoded_len(&self) -> usize {
        self.encoded_len
    }

    pub(super) fn fragments(&self) -> [&[u8]; 3] {
        [&self.prefix, &self.manifest, &self.payload]
    }
}

impl PackageBodyCacheIndex {
    pub(crate) fn manifest(&self) -> &PackageBodiesManifest {
        &self.manifest
    }

    /// Return the section-relative range for one crate-global semantic index.
    pub(crate) fn semantic_index_range(
        &self,
        crate_id: CrateId,
    ) -> Option<PackageCacheSectionRange> {
        self.crates
            .get(crate_id.0)
            .map(|crate_layout| self.payload_range(crate_layout.semantic_index))
    }

    /// Return the section-relative range for one crate and source file.
    pub(crate) fn file_range(
        &self,
        crate_id: CrateId,
        file: FileId,
    ) -> Option<PackageCacheSectionRange> {
        self.crates.get(crate_id.0).and_then(|crate_layout| {
            crate_layout
                .files
                .iter()
                .find(|entry| entry.file == file)
                .map(|entry| self.payload_range(entry.range))
        })
    }

    /// Translate a validated payload-relative range into Body IR section coordinates.
    fn payload_range(&self, range: PackageCacheSectionRange) -> PackageCacheSectionRange {
        PackageCacheSectionRange {
            offset: self
                .payload_offset
                .checked_add(range.offset)
                .expect("validated Body IR payload range should not overflow"),
            len: range.len,
        }
    }
}

impl PackageCacheCodec {
    /// Encode Body IR as a small directory followed by independently decodable payloads.
    ///
    /// Body ids stay stable throughout this transformation. A file shard carries the original ids,
    /// while the logical manifest records which file owns each id.
    pub(super) fn encode_body_ir(body_ir: &PackageBodies) -> anyhow::Result<EncodedBodyIr> {
        // 1. Build the logical directory first. It tells us which source-file shards each crate
        // needs, but does not contain encoded byte ranges yet.
        let bodies = body_ir.manifest();
        let mut payload = Vec::new();
        let mut crates = Vec::with_capacity(body_ir.crates().len());

        // 2. Serialize one crate index and one source file at a time. Each append returns its range
        // relative to `payload`. This avoids a second package-sized set of temporary shard objects.
        for (crate_idx, crate_bodies) in body_ir.crates().iter().enumerate() {
            let semantic_index_start = payload.len();
            wincode::config::serialize_into(
                &mut payload,
                crate_bodies.semantic_index(),
                Self::wincode_config(),
            )
            .map_err(|error| anyhow::anyhow!("{error}"))
            .context("while attempting to serialize package cache Body IR crate index")?;
            let semantic_index = Self::body_payload_range(semantic_index_start, payload.len())?;

            let crate_id = CrateId(crate_idx);
            let crate_manifest = bodies
                .crate_manifest(crate_id)
                .expect("Body IR manifest should mirror package crates");
            let mut files = Vec::with_capacity(crate_manifest.files().len());
            for &file in crate_manifest.files() {
                let shard = crate_bodies.file_shard(file);
                let shard_start = payload.len();
                wincode::config::serialize_into(&mut payload, &shard, Self::wincode_config())
                    .map_err(|error| anyhow::anyhow!("{error}"))
                    .context("while attempting to serialize package cache Body IR file shard")?;
                files.push(BodyFileCacheRange {
                    file,
                    range: Self::body_payload_range(shard_start, payload.len())?,
                });
            }
            crates.push(CrateBodyCacheLayout {
                semantic_index,
                files,
            });
        }

        // 3. The physical directory can be encoded only after every payload range is known.
        let manifest = PackageBodyCacheManifest { bodies, crates };
        let manifest = wincode::config::serialize(&manifest, Self::wincode_config())
            .map_err(|error| anyhow::anyhow!("{error}"))
            .context("while attempting to serialize package cache Body IR manifest")?;
        let manifest_len = u64::try_from(manifest.len())
            .context("package cache Body IR manifest length does not fit u64")?;
        let total_len = BODY_CACHE_CONTAINER_PREFIX_BYTES
            .checked_add(manifest.len())
            .and_then(|len| len.checked_add(payload.len()))
            .context("package cache Body IR section length overflows usize")?;
        anyhow::ensure!(
            total_len <= PACKAGE_CACHE_SECTION_LIMIT_BYTES,
            "package cache Body IR section has {total_len} bytes, limit is {PACKAGE_CACHE_SECTION_LIMIT_BYTES}",
        );

        // 4. Keep the three final fragments separate. The package writer can emit them in this
        // order without allocating and copying another complete Body IR buffer.
        let mut prefix = [0_u8; BODY_CACHE_CONTAINER_PREFIX_BYTES];
        prefix[..BODY_CACHE_CONTAINER_MAGIC.len()].copy_from_slice(&BODY_CACHE_CONTAINER_MAGIC);
        prefix[BODY_CACHE_CONTAINER_MAGIC.len()..].copy_from_slice(&manifest_len.to_le_bytes());
        Ok(EncodedBodyIr {
            prefix,
            manifest,
            payload,
            encoded_len: total_len,
        })
    }

    /// Describe bytes appended directly to the combined payload.
    fn body_payload_range(start: usize, end: usize) -> anyhow::Result<PackageCacheSectionRange> {
        let len = end
            .checked_sub(start)
            .context("Body IR payload end precedes its start")?;
        Ok(PackageCacheSectionRange {
            offset: u64::try_from(start).context("Body IR payload offset does not fit u64")?,
            len: u64::try_from(len).context("Body IR payload length does not fit u64")?,
        })
    }

    /// Validate the fixed Body IR prefix and return the following manifest length.
    ///
    /// The artifact reader can call this after reading only the prefix, then issue one exact read
    /// for the variable-size manifest.
    pub(crate) fn decode_body_prefix(prefix: &[u8]) -> anyhow::Result<usize> {
        anyhow::ensure!(
            prefix.len() == BODY_CACHE_CONTAINER_PREFIX_BYTES,
            "package cache Body IR prefix has {} bytes, expected {}",
            prefix.len(),
            BODY_CACHE_CONTAINER_PREFIX_BYTES,
        );
        anyhow::ensure!(
            prefix[..BODY_CACHE_CONTAINER_MAGIC.len()] == BODY_CACHE_CONTAINER_MAGIC,
            "package cache Body IR container magic is invalid",
        );
        let manifest_len = u64::from_le_bytes(
            prefix[BODY_CACHE_CONTAINER_MAGIC.len()..]
                .try_into()
                .expect("fixed Body IR manifest length should contain eight bytes"),
        );
        let manifest_len = usize::try_from(manifest_len)
            .context("package cache Body IR manifest length does not fit usize")?;
        anyhow::ensure!(
            manifest_len <= PACKAGE_CACHE_SECTION_LIMIT_BYTES,
            "package cache Body IR manifest has {manifest_len} bytes, limit is {PACKAGE_CACHE_SECTION_LIMIT_BYTES}",
        );
        Ok(manifest_len)
    }

    /// Decode and validate the Body IR directory without touching crate or file payloads.
    ///
    /// Validation connects the directory to the outer probe, checks that logical files match
    /// physical file ranges, and requires payload ranges to cover the remaining section exactly.
    pub(crate) fn decode_body_index(
        manifest_bytes: &[u8],
        section_len: u64,
        probe: &PackageCacheProbe,
    ) -> anyhow::Result<PackageBodyCacheIndex> {
        let manifest = wincode::config::deserialize_exact::<PackageBodyCacheManifest, _>(
            manifest_bytes,
            Self::wincode_config(),
        )
        .map_err(|error| anyhow::anyhow!("{error}"))
        .context("while attempting to deserialize package cache Body IR manifest")?;
        Self::validate_body_manifest(&manifest, probe)?;

        // Serialized ranges start at the payload. Record where that payload begins inside the Body
        // IR section so callers no longer need to know about prefix or manifest framing.
        let payload_offset = u64::try_from(BODY_CACHE_CONTAINER_PREFIX_BYTES)
            .expect("Body IR prefix length should fit u64")
            .checked_add(
                u64::try_from(manifest_bytes.len())
                    .context("Body IR manifest length does not fit u64")?,
            )
            .context("Body IR payload offset overflows u64")?;
        anyhow::ensure!(
            payload_offset <= section_len,
            "package cache Body IR manifest ends at byte {payload_offset}, section has {section_len} bytes",
        );
        let payload_len = section_len - payload_offset;
        Self::validate_body_ranges(&manifest, payload_len)?;

        Ok(PackageBodyCacheIndex {
            manifest: manifest.bodies,
            crates: manifest.crates,
            payload_offset,
        })
    }

    /// Decode one crate-global index from its validated range.
    pub(crate) fn decode_body_semantic_index(bytes: &[u8]) -> anyhow::Result<ItemLookupIndex> {
        wincode::config::deserialize_exact::<ItemLookupIndex, _>(bytes, Self::wincode_config())
            .map_err(|error| anyhow::anyhow!("{error}"))
            .context("while attempting to deserialize package cache Body IR crate index")
    }

    /// Decode one file shard and verify that it contains exactly the bodies assigned to that file.
    pub(crate) fn decode_body_file_shard(
        bytes: &[u8],
        manifest: &CrateBodiesManifest,
        file: FileId,
    ) -> anyhow::Result<BodyFileShard> {
        let shard =
            wincode::config::deserialize_exact::<BodyFileShard, _>(bytes, Self::wincode_config())
                .map_err(|error| anyhow::anyhow!("{error}"))
                .context("while attempting to deserialize package cache Body IR file shard")?;
        Self::validate_body_file_shard(&shard, manifest, file)?;
        Ok(shard)
    }

    /// Check that the logical Body IR shape and physical directory describe the same package.
    fn validate_body_manifest(
        manifest: &PackageBodyCacheManifest,
        probe: &PackageCacheProbe,
    ) -> anyhow::Result<()> {
        let crate_count = probe.header.package.targets.len();
        anyhow::ensure!(
            manifest.bodies.crates().len() == crate_count,
            "package cache Body IR manifest has {} crates but header has {crate_count} Cargo targets",
            manifest.bodies.crates().len(),
        );
        anyhow::ensure!(
            manifest.crates.len() == crate_count,
            "package cache Body IR directory has {} crates but header has {crate_count} Cargo targets",
            manifest.crates.len(),
        );
        let coverage = manifest
            .bodies
            .crates()
            .iter()
            .map(CrateBodiesManifest::coverage)
            .collect::<Vec<_>>();
        anyhow::ensure!(
            coverage == probe.body_ir_coverage,
            "package cache Body IR manifest coverage does not match its probe",
        );

        for (crate_idx, (crate_manifest, layout)) in manifest
            .bodies
            .crates()
            .iter()
            .zip(&manifest.crates)
            .enumerate()
        {
            let files = layout
                .files
                .iter()
                .map(|entry| entry.file)
                .collect::<Vec<_>>();
            anyhow::ensure!(
                files == crate_manifest.files(),
                "package cache Body IR crate {crate_idx} file directory does not match its manifest",
            );
        }
        Ok(())
    }

    /// Require payload ranges to be ordered, contiguous, bounded, and complete.
    ///
    /// This rejects overlaps, gaps, and unclaimed trailing bytes. Later reads can therefore trust a
    /// range from the index without rechecking it against every other range.
    fn validate_body_ranges(
        manifest: &PackageBodyCacheManifest,
        payload_len: u64,
    ) -> anyhow::Result<()> {
        let mut next_offset = 0_u64;
        for crate_layout in &manifest.crates {
            let ranges = std::iter::once(crate_layout.semantic_index)
                .chain(crate_layout.files.iter().map(|file| file.range));
            for range in ranges {
                anyhow::ensure!(
                    range.offset == next_offset,
                    "package cache Body IR payload range starts at byte {}, expected {next_offset}",
                    range.offset,
                );
                anyhow::ensure!(
                    range.len <= PACKAGE_CACHE_SECTION_LIMIT_BYTES as u64,
                    "package cache Body IR payload has {} bytes, limit is {PACKAGE_CACHE_SECTION_LIMIT_BYTES}",
                    range.len,
                );
                next_offset = next_offset
                    .checked_add(range.len)
                    .context("package cache Body IR payload ranges overflow u64")?;
            }
        }
        anyhow::ensure!(
            next_offset == payload_len,
            "package cache Body IR payload ranges end at byte {next_offset}, payload has {payload_len} bytes",
        );
        Ok(())
    }

    /// Check both directions of the manifest-to-shard relationship.
    ///
    /// Every entry must route to this file, no body may appear twice, and the entry count must equal
    /// the number of body ids assigned to the file. Together those checks also catch missing bodies.
    fn validate_body_file_shard(
        shard: &BodyFileShard,
        manifest: &CrateBodiesManifest,
        file: FileId,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            shard.file() == file,
            "package cache Body IR file shard belongs to {:?}, expected {:?}",
            shard.file(),
            file,
        );
        let expected_count = (0..manifest.body_count())
            .filter(|&body| manifest.body_file(rg_ir_model::BodyId(body)) == Some(file))
            .count();
        anyhow::ensure!(
            shard.entries().len() == expected_count,
            "package cache Body IR file shard has {} bodies, expected {expected_count}",
            shard.entries().len(),
        );
        let mut seen = vec![false; manifest.body_count()];
        for entry in shard.entries() {
            let body = entry.body();
            anyhow::ensure!(
                manifest.body_file(body) == Some(file),
                "package cache Body IR shard stores body {:?} under the wrong file",
                body,
            );
            anyhow::ensure!(
                !seen[body.0],
                "package cache Body IR shard duplicates body {:?}",
                body,
            );
            seen[body.0] = true;
        }
        Ok(())
    }
}
