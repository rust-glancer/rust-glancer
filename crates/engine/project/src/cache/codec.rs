//! Owned cache metadata encoding built on rkyv.
//!
//! Cache-native metadata structs are the artifact schema, and retained package payloads archive
//! directly through their cache bundle wrappers.

use anyhow::Context as _;

use super::{
    CURRENT_PACKAGE_CACHE_SCHEMA_VERSION, PackageCacheArtifact, PackageCacheBodyIrState,
    PackageCacheHeader,
};

/// Encodes and decodes cache artifact metadata.
pub struct PackageCacheCodec;

impl PackageCacheCodec {
    #[cfg(test)]
    pub(super) fn encode_header(
        header: &PackageCacheHeader,
    ) -> anyhow::Result<rkyv::util::AlignedVec> {
        Self::encode_with_cleared_arena(header)
            .context("while attempting to serialize package cache header")
    }

    #[cfg(test)]
    pub(super) fn decode_header(bytes: &[u8]) -> anyhow::Result<PackageCacheHeader> {
        let header = rkyv::from_bytes::<PackageCacheHeader, rkyv::rancor::Error>(bytes)
            .map_err(|error| anyhow::anyhow!("{error}"))
            .context("while attempting to deserialize package cache header")?;

        Self::validate_header(&header)?;

        Ok(header)
    }

    pub fn encode_artifact(
        artifact: &PackageCacheArtifact,
    ) -> anyhow::Result<rkyv::util::AlignedVec> {
        Self::encode_with_cleared_arena(artifact)
            .context("while attempting to serialize package cache artifact")
    }

    fn encode_with_cleared_arena<T>(value: &T) -> anyhow::Result<rkyv::util::AlignedVec>
    where
        T: for<'a> rkyv::Serialize<
                rkyv::api::high::HighSerializer<
                    rkyv::util::AlignedVec,
                    rkyv::ser::allocator::ArenaHandle<'a>,
                    rkyv::rancor::Error,
                >,
            >,
    {
        let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(value)
            .map_err(|error| anyhow::anyhow!("{error}"));
        rkyv::util::clear_arena();
        bytes
    }

    pub fn decode_artifact(bytes: &[u8]) -> anyhow::Result<PackageCacheArtifact> {
        let artifact = rkyv::from_bytes::<PackageCacheArtifact, rkyv::rancor::Error>(bytes)
            .map_err(|error| anyhow::anyhow!("{error}"))
            .context("while attempting to deserialize package cache artifact")?;

        Self::validate_artifact(&artifact)?;

        Ok(artifact)
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
