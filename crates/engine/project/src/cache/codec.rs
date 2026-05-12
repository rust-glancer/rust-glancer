//! Owned cache metadata encoding built on wincode.
//!
//! Cache-native metadata structs are the artifact schema, and retained package payloads archive
//! directly through their cache bundle wrappers.

use anyhow::Context as _;

use super::{
    CURRENT_PACKAGE_CACHE_SCHEMA_VERSION, PackageCacheArtifact, PackageCacheBodyIrState,
    PackageCacheHeader,
};

// Limit preallocation per package to 256mb.
// This is meant to be a protection against corrupted/absurd artifacts, yet be sufficient
// for any realistic payload.
const PACKAGE_CACHE_PREALLOCATION_LIMIT_BYTES: usize = 256 * 1024 * 1024;

type PackageCacheWincodeConfig =
    wincode::config::Configuration<true, PACKAGE_CACHE_PREALLOCATION_LIMIT_BYTES>;

/// Encodes and decodes cache artifact metadata.
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
        let header =
            wincode::config::deserialize::<PackageCacheHeader, _>(bytes, Self::wincode_config())
                .map_err(|error| anyhow::anyhow!("{error}"))
                .context("while attempting to deserialize package cache header")?;

        Self::validate_header(&header)?;

        Ok(header)
    }

    pub fn encode_artifact(artifact: &PackageCacheArtifact) -> anyhow::Result<Vec<u8>> {
        wincode::config::serialize(artifact, Self::wincode_config())
            .map_err(|error| anyhow::anyhow!("{error}"))
            .context("while attempting to serialize package cache artifact")
    }

    pub fn decode_artifact(bytes: &[u8]) -> anyhow::Result<PackageCacheArtifact> {
        let artifact =
            wincode::config::deserialize::<PackageCacheArtifact, _>(bytes, Self::wincode_config())
                .map_err(|error| anyhow::anyhow!("{error}"))
                .context("while attempting to deserialize package cache artifact")?;

        Self::validate_artifact(&artifact)?;

        Ok(artifact)
    }

    fn wincode_config() -> PackageCacheWincodeConfig {
        wincode::config::Configuration::default()
            .with_preallocation_size_limit::<PACKAGE_CACHE_PREALLOCATION_LIMIT_BYTES>()
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

    fn validate_artifact(artifact: &PackageCacheArtifact) -> anyhow::Result<()> {
        Self::validate_header(&artifact.header)?;

        let package = &artifact.header.package;
        let target_count = package.targets.len();

        // These checks reject cache files whose retained phases can no longer address the same
        // package/target slots. Deeper semantic invalidation stays a project-level decision.
        if artifact.payload.parse.target_root_count() != target_count {
            anyhow::bail!(
                "package cache artifact has {} parse targets but header has {} targets",
                artifact.payload.parse.target_root_count(),
                target_count,
            );
        }

        if artifact.payload.def_map.package().package_name() != package.name {
            anyhow::bail!(
                "package cache artifact belongs to def-map package `{}`, expected `{}`",
                artifact.payload.def_map.package().package_name(),
                package.name,
            );
        }

        if artifact.payload.def_map.package().targets().len() != target_count {
            anyhow::bail!(
                "package cache artifact has {} def-map targets but header has {} targets",
                artifact.payload.def_map.package().targets().len(),
                target_count,
            );
        }

        if artifact.payload.semantic_ir.package().targets().len() != target_count {
            anyhow::bail!(
                "package cache artifact has {} semantic IR targets but header has {} targets",
                artifact.payload.semantic_ir.package().targets().len(),
                target_count,
            );
        }

        if let PackageCacheBodyIrState::Built(body_ir) = &artifact.payload.body_ir {
            if body_ir.package().targets().len() != target_count {
                anyhow::bail!(
                    "package cache artifact has {} body IR targets but header has {} targets",
                    body_ir.package().targets().len(),
                    target_count,
                );
            }
        }

        Ok(())
    }
}
