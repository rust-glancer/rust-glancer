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
//! does not deserialize earlier phases as framing. The Body IR section delegates to [`body`], whose
//! nested directory provides finer source-file granularity.
//!
//! Wincode is the representation inside each section, not a long-lived compatibility promise.
//! Schema compatibility comes from the version in the probe header. Every decoder also validates
//! structural relationships against the probe, exact section consumption, declared ranges, and
//! allocation limits before returning engine data.
//!
//! The codec only deals with bytes and engine values. It does not open files or decide whether a
//! cache miss should trigger a rebuild; those responsibilities belong to the cache store and the
//! project loading layer.

use anyhow::Context as _;
use rg_body_ir::PackageBodies;
use rg_ir_storage::PackageDefMaps as DefMapPackage;
use rg_semantic_ir::PackageIr;
use wincode::{SchemaRead, SchemaWrite};

mod body;

pub(crate) use self::body::{BODY_CACHE_CONTAINER_PREFIX_BYTES, PackageBodyCacheIndex};

#[cfg(test)]
use super::PackageCachePayload;
use super::{
    CURRENT_PACKAGE_CACHE_SCHEMA_VERSION, PackageCacheArtifact, PackageCacheHeader,
    PackageCacheProbe,
};
const PACKAGE_CACHE_CONTAINER_MAGIC: [u8; 8] = *b"RGPKG\0\0\x01";
/// Bytes needed to discover every outer section without decoding wincode data.
pub(crate) const PACKAGE_CACHE_CONTAINER_PREFIX_BYTES: usize = 8 + 4 * size_of::<u64>();

// Protect section decoding from corrupted lengths while leaving ample room for realistic large
// package phases. The complete container may exceed this because each phase is bounded separately.
const PACKAGE_CACHE_SECTION_LIMIT_BYTES: usize = 256 * 1024 * 1024;

type PackageCacheWincodeConfig =
    wincode::config::Configuration<true, PACKAGE_CACHE_SECTION_LIMIT_BYTES>;

/// Absolute byte range in the outer file, or relative range in a nested payload.
///
/// Code that stores a range is responsible for making its coordinate system clear. The outer
/// layout uses file offsets; the serialized Body IR directory uses offsets from its payload start.
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

impl PackageCacheLayout {
    /// Turn the fixed prefix into trusted file ranges.
    ///
    /// Besides checking magic and individual size limits, this requires the four ranges to end
    /// exactly at `file_len`. A truncated file and a file with unexplained trailing bytes are both
    /// rejected before any wincode decoder sees their contents.
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

        // 3. Convert lengths into contiguous absolute ranges. The checked cursor prevents a
        // corrupted length from wrapping around to an earlier part of the file.
        let mut next_offset = u64::try_from(PACKAGE_CACHE_CONTAINER_PREFIX_BYTES)
            .expect("package cache prefix length should fit into u64");
        let mut next_range = |len: u64| -> anyhow::Result<PackageCacheSectionRange> {
            anyhow::ensure!(
                len <= PACKAGE_CACHE_SECTION_LIMIT_BYTES as u64,
                "package cache section has {len} bytes, limit is {PACKAGE_CACHE_SECTION_LIMIT_BYTES}",
            );
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
        // 4. All declared sections together must account for the complete artifact.
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
        let mut prefix = [0_u8; PACKAGE_CACHE_CONTAINER_PREFIX_BYTES];
        prefix[..PACKAGE_CACHE_CONTAINER_MAGIC.len()]
            .copy_from_slice(&PACKAGE_CACHE_CONTAINER_MAGIC);
        let mut cursor = PACKAGE_CACHE_CONTAINER_MAGIC.len();
        for length in section_lengths {
            anyhow::ensure!(
                length <= PACKAGE_CACHE_SECTION_LIMIT_BYTES,
                "package cache section has {length} bytes, limit is {PACKAGE_CACHE_SECTION_LIMIT_BYTES}",
            );
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
    #[cfg(test)]
    pub(super) fn encode_header(header: &PackageCacheHeader) -> anyhow::Result<Vec<u8>> {
        wincode::config::serialize(header, Self::wincode_config())
            .map_err(|error| anyhow::anyhow!("{error}"))
            .context("while attempting to serialize package cache header")
    }

    #[cfg(test)]
    pub(super) fn decode_header(bytes: &[u8]) -> anyhow::Result<PackageCacheHeader> {
        let header = wincode::config::deserialize_exact::<PackageCacheHeader, _>(
            bytes,
            Self::wincode_config(),
        )
        .map_err(|error| anyhow::anyhow!("{error}"))
        .context("while attempting to deserialize package cache header")?;
        Self::validate_header(&header)?;
        Ok(header)
    }

    /// Encode one complete package revision while preserving independent section boundaries.
    ///
    /// Validation runs against the in-memory values first. This avoids publishing an artifact whose
    /// sections are individually serializable but disagree about package identity or target count.
    pub fn encode_artifact(artifact: &PackageCacheArtifact) -> anyhow::Result<Vec<u8>> {
        Self::validate_artifact(artifact)?;

        // Encode every phase separately. The resulting lengths become the fixed outer directory,
        // so readers can later decode one phase without walking through the preceding phases.
        let probe = PackageCacheProbe::from_artifact(artifact);
        let probe = wincode::config::serialize(&probe, Self::wincode_config())
            .map_err(|error| anyhow::anyhow!("{error}"))
            .context("while attempting to serialize package cache probe")?;
        let def_map = wincode::config::serialize(&artifact.payload.def_map, Self::wincode_config())
            .map_err(|error| anyhow::anyhow!("{error}"))
            .context("while attempting to serialize package cache def-map section")?;
        let semantic_ir =
            wincode::config::serialize(&artifact.payload.semantic_ir, Self::wincode_config())
                .map_err(|error| anyhow::anyhow!("{error}"))
                .context("while attempting to serialize package cache semantic IR section")?;
        let body_ir = Self::encode_body_ir(&artifact.payload.body_ir)?;

        // The prefix is written only after every section has a final length.
        let prefix = PackageCacheLayout::encode_prefix([
            probe.len(),
            def_map.len(),
            semantic_ir.len(),
            body_ir.len(),
        ])?;
        let total_len = prefix
            .len()
            .checked_add(probe.len())
            .and_then(|len| len.checked_add(def_map.len()))
            .and_then(|len| len.checked_add(semantic_ir.len()))
            .and_then(|len| len.checked_add(body_ir.len()))
            .context("package cache artifact length overflows usize")?;
        // Finally concatenate the directory and sections in the order promised by the format.
        let mut bytes = Vec::with_capacity(total_len);
        bytes.extend_from_slice(&prefix);
        bytes.extend_from_slice(&probe);
        bytes.extend_from_slice(&def_map);
        bytes.extend_from_slice(&semantic_ir);
        bytes.extend_from_slice(&body_ir);
        Ok(bytes)
    }

    #[cfg(test)]
    pub fn decode_artifact(bytes: &[u8]) -> anyhow::Result<PackageCacheArtifact> {
        let prefix = bytes
            .get(..PACKAGE_CACHE_CONTAINER_PREFIX_BYTES)
            .context("package cache artifact is shorter than its fixed prefix")?;
        let layout = PackageCacheLayout::decode_prefix(
            prefix,
            u64::try_from(bytes.len()).context("package cache artifact length does not fit u64")?,
        )?;
        let probe = Self::decode_probe(Self::section_bytes(bytes, layout.probe)?)?;
        let def_map = Self::decode_def_map(Self::section_bytes(bytes, layout.def_map)?, &probe)?;
        let semantic_ir =
            Self::decode_semantic_ir(Self::section_bytes(bytes, layout.semantic_ir)?, &probe)?;
        let body_ir = Self::decode_body_ir(Self::section_bytes(bytes, layout.body_ir)?, &probe)?;

        Ok(PackageCacheArtifact::new(
            probe.header,
            PackageCachePayload::new(probe.parse, def_map, semantic_ir, body_ir),
        ))
    }

    /// Decode the small startup section and validate its package-wide counts.
    pub(crate) fn decode_probe(bytes: &[u8]) -> anyhow::Result<PackageCacheProbe> {
        let probe = wincode::config::deserialize_exact::<PackageCacheProbe, _>(
            bytes,
            Self::wincode_config(),
        )
        .map_err(|error| anyhow::anyhow!("{error}"))
        .context("while attempting to deserialize package cache probe")?;
        Self::validate_probe(&probe)?;
        Ok(probe)
    }

    /// Decode DefMap and check that it belongs to the package described by the probe.
    pub(crate) fn decode_def_map(
        bytes: &[u8],
        probe: &PackageCacheProbe,
    ) -> anyhow::Result<DefMapPackage> {
        let def_map =
            wincode::config::deserialize_exact::<DefMapPackage, _>(bytes, Self::wincode_config())
                .map_err(|error| anyhow::anyhow!("{error}"))
                .context("while attempting to deserialize package cache def-map section")?;
        Self::validate_def_map(&def_map, probe)?;
        Ok(def_map)
    }

    /// Decode Semantic IR and check that its target arena matches the probe.
    pub(crate) fn decode_semantic_ir(
        bytes: &[u8],
        probe: &PackageCacheProbe,
    ) -> anyhow::Result<PackageIr> {
        let semantic_ir =
            wincode::config::deserialize_exact::<PackageIr, _>(bytes, Self::wincode_config())
                .map_err(|error| anyhow::anyhow!("{error}"))
                .context("while attempting to deserialize package cache semantic IR section")?;
        Self::validate_semantic_ir(&semantic_ir, probe)?;
        Ok(semantic_ir)
    }

    #[cfg(test)]
    fn section_bytes(bytes: &[u8], range: PackageCacheSectionRange) -> anyhow::Result<&[u8]> {
        let start = usize::try_from(range.offset)
            .context("package cache section offset does not fit usize")?;
        let len = usize::try_from(range.len)
            .context("package cache section length does not fit usize")?;
        let end = start
            .checked_add(len)
            .context("package cache section range overflows usize")?;
        bytes
            .get(start..end)
            .context("package cache section range is outside artifact bytes")
    }

    /// Use one bounded wincode configuration for every independently decoded storage unit.
    ///
    /// The preallocation limit is defensive against malformed cache lengths. It is not a total
    /// artifact limit: the outer file can contain several separately bounded sections.
    fn wincode_config() -> PackageCacheWincodeConfig {
        wincode::config::Configuration::default()
            .with_preallocation_size_limit::<PACKAGE_CACHE_SECTION_LIMIT_BYTES>()
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
        Self::validate_header(&probe.header)?;
        let target_count = probe.header.package.targets.len();
        anyhow::ensure!(
            probe.parse.target_root_count() == target_count,
            "package cache probe has {} parse targets but header has {} targets",
            probe.parse.target_root_count(),
            target_count,
        );
        anyhow::ensure!(
            probe.body_ir_coverage.len() == target_count,
            "package cache probe has {} Body IR coverage entries but header has {} targets",
            probe.body_ir_coverage.len(),
            target_count,
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
            def_map.def_maps().len() == package.targets.len(),
            "package cache artifact has {} def-map targets but header has {} targets",
            def_map.def_maps().len(),
            package.targets.len(),
        );
        Ok(())
    }

    fn validate_semantic_ir(
        semantic_ir: &PackageIr,
        probe: &PackageCacheProbe,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            semantic_ir.targets().len() == probe.header.package.targets.len(),
            "package cache artifact has {} semantic IR targets but header has {} targets",
            semantic_ir.targets().len(),
            probe.header.package.targets.len(),
        );
        Ok(())
    }

    fn validate_body_ir(body_ir: &PackageBodies, probe: &PackageCacheProbe) -> anyhow::Result<()> {
        anyhow::ensure!(
            body_ir.targets().len() == probe.header.package.targets.len(),
            "package cache artifact has {} Body IR targets but header has {} targets",
            body_ir.targets().len(),
            probe.header.package.targets.len(),
        );
        let coverage = body_ir
            .targets()
            .iter()
            .map(|target| target.coverage())
            .collect::<Vec<_>>();
        anyhow::ensure!(
            coverage == probe.body_ir_coverage,
            "package cache Body IR coverage does not match its probe",
        );
        Ok(())
    }

    /// Check all cross-section relationships before writing bytes.
    fn validate_artifact(artifact: &PackageCacheArtifact) -> anyhow::Result<()> {
        let probe = PackageCacheProbe::from_artifact(artifact);
        Self::validate_probe(&probe)?;
        Self::validate_def_map(&artifact.payload.def_map, &probe)?;
        Self::validate_semantic_ir(&artifact.payload.semantic_ir, &probe)?;
        Self::validate_body_ir(&artifact.payload.body_ir, &probe)
    }
}
