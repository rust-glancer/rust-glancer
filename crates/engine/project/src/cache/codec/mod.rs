//! Outer `.rgpkg` container and phase codecs.
//!
//! The file starts with a format magic followed by four little-endian section lengths. Sections
//! are contiguous and have a fixed semantic order:
//!
//! ```text
//! prefix: RGPKG magic | probe len | DefMap len | Semantic IR len | Body IR len
//! data:   probe       | DefMap    | Semantic IR                    | Body IR
//! ```
//!
//! This directory is deliberately fixed-size. Startup can validate it with one small read and then
//! fetch only the probe. A request that needs a later phase seeks directly to its byte range; it
//! does not deserialize earlier phases as framing. Each retained phase then has a nested directory:
//! DefMap and Semantic IR use crate shards, while Body IR uses source-file shards.
//!
//! Wincode is the representation inside each section, not a long-lived compatibility promise.
//! Schema compatibility comes from the version in the probe header. Every decoder also validates
//! structural relationships against the probe, exact section consumption, declared ranges, and
//! per-decode allocation limits before returning engine data.
//!
//! The codec only deals with bytes and engine values. It does not open files or decide whether a
//! cache miss should trigger a rebuild; those responsibilities belong to the cache store and the
//! project loading layer.

use anyhow::Context as _;
use rg_body_ir::PackageBodies;
use rg_def_map::PackageDefMaps as DefMapPackage;
use rg_ir_model::CrateId;
use rg_parse::FileId;
use rg_semantic_ir::PackageIr;
use wincode::{SchemaRead, SchemaWrite};

mod body;
mod crate_shards;
mod def_map;
mod semantic_ir;

use self::body::EncodedBodyIr;
pub(crate) use self::body::{BODY_CACHE_CONTAINER_PREFIX_BYTES, PackageBodyCacheIndex};
use self::crate_shards::EncodedCrateShards;
pub(crate) use self::{
    crate_shards::CRATE_SHARD_CONTAINER_PREFIX_BYTES,
    def_map::PackageDefMapCacheIndex,
    semantic_ir::{
        PackageSemanticIrCacheIndex, SEMANTIC_IR_CRATE_PREFIX_BYTES, SemanticIrCrateCacheIndex,
    },
};

use super::{
    CURRENT_PACKAGE_CACHE_SCHEMA_VERSION, PackageCacheBodyUpdateInput, PackageCacheHeader,
    PackageCacheProbe, PackageCacheWriteInput,
};
const PACKAGE_CACHE_CONTAINER_MAGIC: [u8; 8] = *b"RGPKG\0\0\x01";
/// Bytes needed to discover every outer section without decoding wincode data.
pub(crate) const PACKAGE_CACHE_CONTAINER_PREFIX_BYTES: usize = 8 + 4 * size_of::<u64>();

// Protect one independently decoded allocation from corrupted lengths while leaving ample room for
// realistic crate/file payloads. Aggregate phase sections may exceed this because they are nested
// lazy containers; their manifests and individual shards remain bounded.
const PACKAGE_CACHE_DECODE_LIMIT_BYTES: usize = 384 * 1024 * 1024;

type PackageCacheWincodeConfig =
    wincode::config::Configuration<true, PACKAGE_CACHE_DECODE_LIMIT_BYTES>;

/// Absolute byte range in the outer file, or relative range in a nested payload.
///
/// Code that stores a range is responsible for making its coordinate system clear. The outer
/// layout uses file offsets; nested phase directories use offsets from their payload starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub(crate) struct PackageCacheSectionRange {
    pub(crate) offset: u64,
    pub(crate) len: u64,
}

/// Validated byte directory for the four outer package sections.
///
/// Ranges are contiguous, start immediately after the prefix, and cover the file exactly. Once
/// this value exists, readers can seek to a section without repeating the arithmetic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PackageCacheLayout {
    pub(crate) probe: PackageCacheSectionRange,
    pub(crate) def_map: PackageCacheSectionRange,
    pub(crate) semantic_ir: PackageCacheSectionRange,
    pub(crate) body_ir: PackageCacheSectionRange,
}

/// Encoded package sections in their final on-disk order.
///
/// The fragments stay separate until `write_to` sends them to an atomic file or another byte sink.
/// This preserves one write path without allocating a final contiguous artifact buffer.
#[derive(Debug)]
pub(crate) struct EncodedPackageCacheArtifact {
    prefix: [u8; PACKAGE_CACHE_CONTAINER_PREFIX_BYTES],
    probe: Vec<u8>,
    def_map: EncodedDeclarationSection,
    semantic_ir: EncodedDeclarationSection,
    body_ir: EncodedBodyIr,
}

impl EncodedPackageCacheArtifact {
    /// Write the final fragments without first joining them into another artifact-sized buffer.
    pub(crate) fn write_to(&self, writer: &mut impl std::io::Write) -> std::io::Result<()> {
        for fragment in [&self.prefix[..], &self.probe] {
            writer.write_all(fragment)?;
        }
        self.def_map.write_to(writer)?;
        self.semantic_ir.write_to(writer)?;
        for fragment in self.body_ir.fragments() {
            writer.write_all(fragment)?;
        }
        Ok(())
    }
}

/// A declaration phase ready to occupy its unchanged place in the outer artifact.
///
/// Full writes provide newly encoded crate shards. A Body-only rewrite provides exact bytes copied
/// from the pinned prior artifact revision; both forms expose the same length/write interface to the
/// outer container encoder.
#[derive(Debug)]
enum EncodedDeclarationSection {
    CrateShards(EncodedCrateShards),
    Copied(Vec<u8>),
}

impl EncodedDeclarationSection {
    fn encoded_len(&self) -> usize {
        match self {
            Self::CrateShards(section) => section.encoded_len(),
            Self::Copied(bytes) => bytes.len(),
        }
    }

    fn write_to(&self, writer: &mut impl std::io::Write) -> std::io::Result<()> {
        match self {
            Self::CrateShards(section) => section.write_to(writer),
            Self::Copied(bytes) => writer.write_all(bytes),
        }
    }
}

impl PackageCacheLayout {
    /// Turn the fixed prefix into trusted file ranges.
    ///
    /// Besides checking magic and eagerly decoded section limits, this requires the four ranges to
    /// end exactly at `file_len`. A truncated file and a file with unexplained trailing bytes are
    /// both rejected before any wincode decoder sees their contents.
    pub(crate) fn decode_prefix(prefix: &[u8], file_len: u64) -> anyhow::Result<Self> {
        // 1. The caller should have read exactly the fixed directory. Validate that assumption and
        // reject unrelated files before interpreting their next bytes as lengths.
        anyhow::ensure!(
            prefix.len() == PACKAGE_CACHE_CONTAINER_PREFIX_BYTES,
            "package cache prefix has {} bytes, expected {}",
            prefix.len(),
            PACKAGE_CACHE_CONTAINER_PREFIX_BYTES,
        );
        anyhow::ensure!(
            prefix[..PACKAGE_CACHE_CONTAINER_MAGIC.len()] == PACKAGE_CACHE_CONTAINER_MAGIC,
            "package cache container magic is invalid",
        );

        // 2. Read the four little-endian lengths in their fixed semantic order.
        let mut lengths = [0_u64; 4];
        let mut cursor = PACKAGE_CACHE_CONTAINER_MAGIC.len();
        for length in &mut lengths {
            let end = cursor + size_of::<u64>();
            *length = u64::from_le_bytes(
                prefix[cursor..end]
                    .try_into()
                    .expect("fixed package cache length field should contain eight bytes"),
            );
            cursor = end;
        }

        // 3. Only the probe is decoded as one outer section. Every retained IR section has a nested
        // directory over independently bounded payloads, so its aggregate length is not an
        // allocation bound.
        anyhow::ensure!(
            lengths[0] <= PACKAGE_CACHE_DECODE_LIMIT_BYTES as u64,
            "package cache probe section has {} bytes, limit is {PACKAGE_CACHE_DECODE_LIMIT_BYTES}",
            lengths[0],
        );

        // 4. Convert lengths into contiguous absolute ranges. The checked cursor prevents a
        // corrupted length from wrapping around to an earlier part of the file.
        let mut next_offset = u64::try_from(PACKAGE_CACHE_CONTAINER_PREFIX_BYTES)
            .expect("package cache prefix length should fit into u64");
        let mut next_range = |len: u64| -> anyhow::Result<PackageCacheSectionRange> {
            let range = PackageCacheSectionRange {
                offset: next_offset,
                len,
            };
            next_offset = next_offset
                .checked_add(len)
                .context("package cache section ranges overflow u64")?;
            Ok(range)
        };

        let layout = Self {
            probe: next_range(lengths[0])?,
            def_map: next_range(lengths[1])?,
            semantic_ir: next_range(lengths[2])?,
            body_ir: next_range(lengths[3])?,
        };
        // 5. All declared sections together must account for the complete artifact.
        anyhow::ensure!(
            next_offset == file_len,
            "package cache sections end at byte {next_offset}, file has {file_len} bytes",
        );
        Ok(layout)
    }

    /// Encode section lengths in the same fixed order used by `decode_prefix`.
    fn encode_prefix(
        section_lengths: [usize; 4],
    ) -> anyhow::Result<[u8; PACKAGE_CACHE_CONTAINER_PREFIX_BYTES]> {
        // Retained IR sections are nested lazy containers. Their manifests and individual shards
        // enforce the allocation bound; only the outer probe is a single decoded value.
        anyhow::ensure!(
            section_lengths[0] <= PACKAGE_CACHE_DECODE_LIMIT_BYTES,
            "package cache probe section has {} bytes, limit is {PACKAGE_CACHE_DECODE_LIMIT_BYTES}",
            section_lengths[0],
        );

        let mut prefix = [0_u8; PACKAGE_CACHE_CONTAINER_PREFIX_BYTES];
        prefix[..PACKAGE_CACHE_CONTAINER_MAGIC.len()]
            .copy_from_slice(&PACKAGE_CACHE_CONTAINER_MAGIC);
        let mut cursor = PACKAGE_CACHE_CONTAINER_MAGIC.len();
        for length in section_lengths {
            let end = cursor + size_of::<u64>();
            prefix[cursor..end].copy_from_slice(
                &u64::try_from(length)
                    .context("package cache section length does not fit into u64")?
                    .to_le_bytes(),
            );
            cursor = end;
        }
        Ok(prefix)
    }
}

/// Encodes complete artifacts and independently decodes their analysis sections.
///
/// The store is responsible for reading the declared bytes from disk. The codec is responsible for
/// turning exactly those bytes into engine values and checking their cross-section invariants.
pub struct PackageCacheCodec;

impl PackageCacheCodec {
    /// Encode independently writable fragments from borrowed resident phase data.
    pub(crate) fn encode_write_input(
        input: PackageCacheWriteInput<'_>,
    ) -> anyhow::Result<EncodedPackageCacheArtifact> {
        let probe = PackageCacheProbe::from_write_input(input);
        Self::validate_write_input(input, &probe).context("validate package cache write input")?;
        let def_map = Self::encode_def_map(input.def_map).context("encode DefMap cache section")?;
        let semantic_ir = Self::encode_semantic_ir(input.semantic_ir)
            .context("encode Semantic IR cache section")?;
        let body_ir =
            Self::encode_body_ir(input.body_ir).context("encode Body IR cache section")?;
        Self::encode_sections(
            probe,
            EncodedDeclarationSection::CrateShards(def_map),
            EncodedDeclarationSection::CrateShards(semantic_ir),
            body_ir,
        )
    }

    /// Encode a package overlay while copying cached sibling Body IR shards verbatim.
    pub(crate) fn encode_write_input_reusing_cached_body_ir(
        input: PackageCacheWriteInput<'_>,
        mut read_cached_shard: impl FnMut(CrateId, FileId) -> anyhow::Result<Vec<u8>>,
    ) -> anyhow::Result<EncodedPackageCacheArtifact> {
        let probe = PackageCacheProbe::from_write_input(input);
        Self::validate_write_input(input, &probe).context("validate package cache write input")?;
        let def_map = Self::encode_def_map(input.def_map).context("encode DefMap cache section")?;
        let semantic_ir = Self::encode_semantic_ir(input.semantic_ir)
            .context("encode Semantic IR cache section")?;
        let body_ir =
            Self::encode_body_ir_reusing_cached_shards(input.body_ir, &mut read_cached_shard)
                .context("encode Body IR cache section with cached shards")?;
        Self::encode_sections(
            probe,
            EncodedDeclarationSection::CrateShards(def_map),
            EncodedDeclarationSection::CrateShards(semantic_ir),
            body_ir,
        )
    }

    /// Rewrites Body IR while preserving the prior artifact's exact declaration bytes.
    ///
    /// DefMap and Semantic IR have already been offloaded when this path is used, so decoding and
    /// re-encoding them would defeat the storage boundary. The pinned reader supplies their complete
    /// section bytes, while only Body IR coverage and shards are rebuilt.
    pub(crate) fn encode_body_update_reusing_cached_sections(
        input: PackageCacheBodyUpdateInput<'_>,
        def_map: Vec<u8>,
        semantic_ir: Vec<u8>,
        mut read_cached_shard: impl FnMut(CrateId, FileId) -> anyhow::Result<Vec<u8>>,
    ) -> anyhow::Result<EncodedPackageCacheArtifact> {
        let probe = PackageCacheProbe::from_body_update(input);
        Self::validate_probe(&probe).context("validate package cache probe")?;
        Self::validate_body_ir(input.body_ir, &probe).context("validate Body IR cache update")?;
        let body_ir =
            Self::encode_body_ir_reusing_cached_shards(input.body_ir, &mut read_cached_shard)
                .context("encode Body IR cache update with cached shards")?;
        Self::encode_sections(
            probe,
            EncodedDeclarationSection::Copied(def_map),
            EncodedDeclarationSection::Copied(semantic_ir),
            body_ir,
        )
    }

    /// Assembles fresh or copied phase sections under one newly encoded outer directory.
    fn encode_sections(
        probe: PackageCacheProbe,
        def_map: EncodedDeclarationSection,
        semantic_ir: EncodedDeclarationSection,
        body_ir: EncodedBodyIr,
    ) -> anyhow::Result<EncodedPackageCacheArtifact> {
        // The resulting lengths become the fixed outer directory, so readers can later decode one
        // phase without walking through the preceding phases.
        let probe = wincode::config::serialize(&probe, Self::wincode_config())
            .map_err(|error| anyhow::anyhow!("{error}"))
            .context("while attempting to serialize package cache probe")?;

        // The prefix is built only after every section has a final length.
        let prefix = PackageCacheLayout::encode_prefix([
            probe.len(),
            def_map.encoded_len(),
            semantic_ir.encoded_len(),
            body_ir.encoded_len(),
        ])
        .context("encode package cache outer directory")?;
        Ok(EncodedPackageCacheArtifact {
            prefix,
            probe,
            def_map,
            semantic_ir,
            body_ir,
        })
    }

    /// Decode the small startup section and validate its package-wide counts.
    pub(crate) fn decode_probe(bytes: &[u8]) -> anyhow::Result<PackageCacheProbe> {
        let probe = wincode::config::deserialize_exact::<PackageCacheProbe, _>(
            bytes,
            Self::wincode_config(),
        )
        .map_err(|error| anyhow::anyhow!("{error}"))
        .context("while attempting to deserialize package cache probe")?;
        Self::validate_probe(&probe).context("validate package cache probe")?;
        Ok(probe)
    }

    #[cfg(test)]
    fn section_slice<'a>(
        bytes: &'a [u8],
        range: PackageCacheSectionRange,
        label: &'static str,
    ) -> anyhow::Result<&'a [u8]> {
        let start = usize::try_from(range.offset)
            .with_context(|| format!("{label} offset does not fit usize"))?;
        let len = usize::try_from(range.len)
            .with_context(|| format!("{label} length does not fit usize"))?;
        let end = start
            .checked_add(len)
            .with_context(|| format!("{label} range overflows usize"))?;
        bytes
            .get(start..end)
            .with_context(|| format!("{label} range ends outside its section"))
    }

    /// Use one bounded wincode configuration for every independently decoded storage unit.
    ///
    /// The preallocation limit is defensive against malformed cache lengths. It is not a total
    /// artifact limit: every retained phase can contain many independently bounded storage units.
    fn wincode_config() -> PackageCacheWincodeConfig {
        wincode::config::Configuration::default()
            .with_preallocation_size_limit::<PACKAGE_CACHE_DECODE_LIMIT_BYTES>()
    }

    fn validate_header(header: &PackageCacheHeader) -> anyhow::Result<()> {
        if header.schema_version != CURRENT_PACKAGE_CACHE_SCHEMA_VERSION {
            anyhow::bail!(
                "unsupported package cache schema version {}, expected {}",
                header.schema_version.0,
                CURRENT_PACKAGE_CACHE_SCHEMA_VERSION.0,
            );
        }
        Ok(())
    }

    /// Check the facts later section decoders use as their package-wide reference point.
    fn validate_probe(probe: &PackageCacheProbe) -> anyhow::Result<()> {
        Self::validate_header(&probe.header).context("validate package cache header")?;
        let cargo_target_count = probe.header.package.targets.len();
        anyhow::ensure!(
            probe.parse.target_root_count() == cargo_target_count,
            "package cache probe has {} parse targets but header has {} Cargo targets",
            probe.parse.target_root_count(),
            cargo_target_count,
        );
        anyhow::ensure!(
            probe.body_ir_coverage.len() == cargo_target_count,
            "package cache probe has {} Body IR coverage entries but header has {} Cargo targets",
            probe.body_ir_coverage.len(),
            cargo_target_count,
        );
        Ok(())
    }

    fn validate_def_map(def_map: &DefMapPackage, probe: &PackageCacheProbe) -> anyhow::Result<()> {
        let package = &probe.header.package;
        anyhow::ensure!(
            def_map.package_name() == package.name,
            "package cache artifact belongs to def-map package `{}`, expected `{}`",
            def_map.package_name(),
            package.name,
        );
        anyhow::ensure!(
            def_map.crates().len() == package.targets.len(),
            "package cache artifact has {} def-map crates but header has {} Cargo targets",
            def_map.crates().len(),
            package.targets.len(),
        );
        Ok(())
    }

    fn validate_semantic_ir(
        semantic_ir: &PackageIr,
        probe: &PackageCacheProbe,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            semantic_ir.crates().len() == probe.header.package.targets.len(),
            "package cache artifact has {} semantic IR crates but header has {} Cargo targets",
            semantic_ir.crates().len(),
            probe.header.package.targets.len(),
        );
        Ok(())
    }

    fn validate_body_ir(body_ir: &PackageBodies, probe: &PackageCacheProbe) -> anyhow::Result<()> {
        anyhow::ensure!(
            body_ir.crates().len() == probe.header.package.targets.len(),
            "package cache artifact has {} Body IR crates but header has {} Cargo targets",
            body_ir.crates().len(),
            probe.header.package.targets.len(),
        );
        let coverage = body_ir
            .crates()
            .iter()
            .map(|crate_bodies| crate_bodies.coverage())
            .collect::<Vec<_>>();
        anyhow::ensure!(
            coverage == probe.body_ir_coverage,
            "package cache Body IR coverage does not match its probe",
        );
        Ok(())
    }

    /// Check all cross-section relationships before writing bytes.
    fn validate_write_input(
        input: PackageCacheWriteInput<'_>,
        probe: &PackageCacheProbe,
    ) -> anyhow::Result<()> {
        Self::validate_probe(probe).context("validate package cache probe")?;
        Self::validate_def_map(input.def_map, probe).context("validate DefMap cache input")?;
        Self::validate_semantic_ir(input.semantic_ir, probe)
            .context("validate Semantic IR cache input")?;
        Self::validate_body_ir(input.body_ir, probe)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_layout_allows_body_ir_larger_than_one_decode_unit() {
        // No allocation follows this declared length: opening Body IR reads its small directory,
        // then one independently bounded payload. Exercise both prefix writing and validation.
        let body_ir_len = PACKAGE_CACHE_DECODE_LIMIT_BYTES + 1;
        let prefix = PackageCacheLayout::encode_prefix([0, 0, 0, body_ir_len])
            .expect("aggregate Body IR should not use the per-decode limit");
        let body_ir_len =
            u64::try_from(body_ir_len).expect("fixture Body IR section length should fit u64");
        let file_len = u64::try_from(PACKAGE_CACHE_CONTAINER_PREFIX_BYTES)
            .expect("package cache prefix length should fit u64")
            .checked_add(body_ir_len)
            .expect("fixture package cache length should fit u64");

        let layout = PackageCacheLayout::decode_prefix(&prefix, file_len)
            .expect("aggregate Body IR layout should validate");

        assert_eq!(layout.body_ir.len, body_ir_len);
    }
}
