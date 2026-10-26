//! Crate-granular DefMap section codec.
//!
//! The section starts with a compact [`PackageDefMapsManifest`] and a byte range for each dense
//! crate slot. Every range then contains one complete [`CrateData`] value. A source query can read
//! the manifest to choose the right Cargo target and decode only that target's module scopes.

use anyhow::Context as _;
use rg_def_map::{CrateData, PackageDefMaps, PackageDefMapsManifest};
use rg_ir_model::CrateRef;
use wincode::{SchemaRead, SchemaWrite};

use super::{
    PackageCacheCodec, PackageCacheProbe, PackageCacheSectionRange,
    crate_shards::{
        CrateShardCacheIndex, CrateShardPayloadShape, EncodedCrateShards, decode_crate_shard_prefix,
    },
};

const DEF_MAP_CACHE_CONTAINER_MAGIC: [u8; 8] = *b"RGDEFM\0\x01";

/// Validated DefMap package directory paired with the physical range of every crate payload.
pub(crate) type PackageDefMapCacheIndex = CrateShardCacheIndex<PackageDefMapsManifest>;

/// Serialized DefMap directory: logical routing facts plus physical crate ranges.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite)]
struct PackageDefMapCacheManifest {
    def_map: PackageDefMapsManifest,
    crates: Vec<PackageCacheSectionRange>,
}

impl PackageCacheCodec {
    /// Encodes the routing manifest and one independently decodable [`CrateData`] per crate slot.
    pub(super) fn encode_def_map(def_map: &PackageDefMaps) -> anyhow::Result<EncodedCrateShards> {
        let logical_manifest = def_map.manifest();
        EncodedCrateShards::encode(
            DEF_MAP_CACHE_CONTAINER_MAGIC,
            "DefMap",
            def_map.crates().len(),
            CrateShardPayloadShape::DecodeUnit,
            |crate_idx, payload| {
                wincode::config::serialize_into(
                    payload,
                    &def_map.crates()[crate_idx],
                    Self::wincode_config(),
                )
                .map_err(|error| anyhow::anyhow!("{error}"))
                .context("while attempting to serialize package cache DefMap crate")
            },
            |crates| {
                wincode::config::serialize(
                    &PackageDefMapCacheManifest {
                        def_map: logical_manifest,
                        crates,
                    },
                    Self::wincode_config(),
                )
                .map_err(|error| anyhow::anyhow!("{error}"))
                .context("while attempting to serialize package cache DefMap manifest")
            },
        )
    }

    /// Validates the DefMap container prefix and returns its manifest length.
    pub(crate) fn decode_def_map_prefix(prefix: &[u8]) -> anyhow::Result<usize> {
        decode_crate_shard_prefix(prefix, DEF_MAP_CACHE_CONTAINER_MAGIC, "DefMap")
    }

    /// Decodes and validates the logical directory and its physical crate ranges.
    ///
    /// The package name and dense crate count are checked against the already validated probe before
    /// the result can address bytes in the artifact.
    pub(crate) fn decode_def_map_index(
        manifest_bytes: &[u8],
        section_len: u64,
        probe: &PackageCacheProbe,
    ) -> anyhow::Result<PackageDefMapCacheIndex> {
        let manifest = wincode::config::deserialize_exact::<PackageDefMapCacheManifest, _>(
            manifest_bytes,
            Self::wincode_config(),
        )
        .map_err(|error| anyhow::anyhow!("{error}"))
        .context("while attempting to deserialize package cache DefMap manifest")?;
        Self::validate_def_map_manifest(&manifest, probe)
            .context("validate package cache DefMap manifest")?;
        CrateShardCacheIndex::new(
            manifest.def_map,
            manifest.crates,
            manifest_bytes.len(),
            section_len,
            "DefMap",
            CrateShardPayloadShape::DecodeUnit,
        )
    }

    /// Decodes one crate payload and checks that it belongs to the target in the logical directory.
    pub(crate) fn decode_def_map_crate(
        bytes: &[u8],
        manifest: &PackageDefMapsManifest,
        crate_ref: CrateRef,
    ) -> anyhow::Result<CrateData> {
        let crate_data =
            wincode::config::deserialize_exact::<CrateData, _>(bytes, Self::wincode_config())
                .map_err(|error| anyhow::anyhow!("{error}"))
                .context("while attempting to deserialize package cache DefMap crate")?;
        let crate_manifest = manifest
            .crate_manifest(crate_ref.crate_id)
            .with_context(|| format!("DefMap manifest has no crate {:?}", crate_ref.crate_id))?;
        anyhow::ensure!(
            crate_data.cargo_target() == crate_manifest.cargo_target(),
            "package cache DefMap crate {:?} belongs to Cargo target {:?}, expected {:?}",
            crate_ref.crate_id,
            crate_data.cargo_target(),
            crate_manifest.cargo_target(),
        );
        Ok(crate_data)
    }

    fn validate_def_map_manifest(
        manifest: &PackageDefMapCacheManifest,
        probe: &PackageCacheProbe,
    ) -> anyhow::Result<()> {
        let package = &probe.header.package;
        anyhow::ensure!(
            manifest.def_map.package_name() == package.name,
            "package cache artifact belongs to DefMap package `{}`, expected `{}`",
            manifest.def_map.package_name(),
            package.name,
        );
        anyhow::ensure!(
            manifest.def_map.crates().len() == package.targets.len(),
            "package cache DefMap manifest has {} crates but header has {} Cargo targets",
            manifest.def_map.crates().len(),
            package.targets.len(),
        );
        anyhow::ensure!(
            manifest.crates.len() == package.targets.len(),
            "package cache DefMap directory has {} crates but header has {} Cargo targets",
            manifest.crates.len(),
            package.targets.len(),
        );
        Ok(())
    }
}
