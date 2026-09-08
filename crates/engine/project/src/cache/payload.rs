//! Package cache payload types.
//!
//! Logical contents of one package artifact.
//!
//! The cache writes these values as one atomic revision, but it does not encode them as one wincode
//! object. [`PackageCacheProbe`] is the small startup section. DefMap is split into crate payloads,
//! Semantic IR splits each crate into declarations and a lookup index, and Body IR uses source-file
//! shards. Writes borrow those phase values through [`PackageCacheWriteInput`] instead of assembling
//! an owned aggregate.
//! When only body analysis changes, the writer combines new results with unchanged data copied
//! from the existing cache file.

use rg_body_ir::{
    CrateBodies, CrateBodiesCoverage, CrateBodiesManifest, PackageBodies, PackageBodiesManifest,
};
use rg_def_map::{PackageDefMaps as DefMapPackage, PackageDefMapsManifest};
use rg_ir_model::CrateId;
use rg_parse::PackageParseSnapshot;
use rg_semantic_ir::PackageIr;
use rg_std::MemorySize;
use wincode::{SchemaRead, SchemaWrite};

use super::header::PackageCacheHeader;

/// Borrowed resident phase data used to write one package artifact.
///
/// The cache writer only needs these values for the duration of one synchronous encode. Borrowing
/// DefMap, Semantic IR, and Body IR avoids cloning the arena-heavy resident packages immediately
/// before serializing them. The header and parse snapshot are borrowed as well so every writer uses
/// one representation with the same lifetime boundary.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PackageCacheWriteInput<'a> {
    pub(crate) header: &'a PackageCacheHeader,
    pub(crate) parse: &'a PackageParseSnapshot,
    pub(crate) def_map: &'a DefMapPackage,
    pub(crate) semantic_ir: &'a PackageIr,
    pub(crate) body_ir: &'a PackageBodies,
}

/// The bodies to write for each Cargo target in one package cache file.
///
/// New results are borrowed [`CrateBodies`]. Unchanged targets use a [`CrateBodiesManifest`] read
/// from the existing cache file, so the writer can copy their encoded bodies without reconstructing
/// expressions and inferred facts in memory. The manifest and copied bytes must come from the same
/// open [`PackageArtifactReader`](super::PackageArtifactReader).
#[derive(Debug)]
pub(crate) struct BodyIrWriteInput<'a> {
    pub(crate) crates: Vec<CrateBodyWriteInput<'a>>,
}

#[derive(Debug)]
pub(crate) enum CrateBodyWriteInput<'a> {
    Resident(&'a CrateBodies),
    /// Copy encoded bodies from the same open cache file that supplied this manifest.
    Cached(&'a CrateBodiesManifest),
}

impl<'a> BodyIrWriteInput<'a> {
    pub(crate) fn resident(package: &'a PackageBodies) -> Self {
        Self {
            crates: package
                .crates()
                .iter()
                .map(|bodies| CrateBodyWriteInput::Resident(bodies))
                .collect(),
        }
    }

    pub(crate) fn update(
        previous: &'a PackageBodiesManifest,
        replacements: &'a [(CrateId, CrateBodies)],
    ) -> Self {
        Self {
            crates: previous
                .crates()
                .iter()
                .enumerate()
                .map(
                    |(index, manifest)| match replacements.iter().find(|(id, _)| id.0 == index) {
                        Some((_, bodies)) => CrateBodyWriteInput::Resident(bodies),
                        None => CrateBodyWriteInput::Cached(manifest),
                    },
                )
                .collect(),
        }
    }

    pub(crate) fn manifest(&self) -> PackageBodiesManifest {
        PackageBodiesManifest::new(
            self.crates
                .iter()
                .map(|source| match source {
                    CrateBodyWriteInput::Resident(bodies) => bodies.manifest(),
                    CrateBodyWriteInput::Cached(manifest) => (*manifest).clone(),
                })
                .collect(),
        )
    }

    pub(crate) fn coverage(&self) -> Vec<CrateBodiesCoverage> {
        self.crates
            .iter()
            .map(|source| match source {
                CrateBodyWriteInput::Resident(bodies) => bodies.coverage().clone(),
                CrateBodyWriteInput::Cached(manifest) => manifest.coverage().clone(),
            })
            .collect()
    }
}

/// New body analysis and startup metadata for a package cache file.
///
/// DefMap and Semantic IR are unchanged. The writer copies their encoded data from the existing
/// cache file rather than loading the declarations into memory to serialize them again.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PackageCacheBodyUpdateInput<'a> {
    pub(crate) header: &'a PackageCacheHeader,
    pub(crate) parse: &'a PackageParseSnapshot,
    pub(crate) body_ir: &'a BodyIrWriteInput<'a>,
}

impl<'a> PackageCacheWriteInput<'a> {
    pub(crate) fn new(
        header: &'a PackageCacheHeader,
        parse: &'a PackageParseSnapshot,
        def_map: &'a DefMapPackage,
        semantic_ir: &'a PackageIr,
        body_ir: &'a PackageBodies,
    ) -> Self {
        Self {
            header,
            parse,
            def_map,
            semantic_ir,
            body_ir,
        }
    }
}

/// Small package state needed to validate a cache hit before retained IR is decoded.
///
/// The parse snapshot belongs here because it freezes the exact saved source bytes whose
/// fingerprint is in the header. Body coverage lets the project preserve materialization policy
/// without opening the large Body IR section.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub(crate) struct PackageCacheProbe {
    pub(crate) header: PackageCacheHeader,
    pub(crate) parse: PackageParseSnapshot,
    pub(crate) body_ir_coverage: Vec<CrateBodiesCoverage>,
}

/// Validated startup data retained after the temporary artifact reader is closed.
///
/// The probe owns source identity and Body IR coverage. The [`PackageDefMapsManifest`] is retained
/// because dependency visibility and file routing are frequent cross-package queries that should
/// not reopen every artifact merely to discover which crate payload would be relevant.
#[derive(Debug, Clone)]
pub(crate) struct PackageCacheStartup {
    pub(crate) probe: PackageCacheProbe,
    pub(crate) def_map_manifest: PackageDefMapsManifest,
}

impl PackageCacheProbe {
    /// Build the small validation section without serializing the retained phase payloads.
    pub(crate) fn from_write_input(input: PackageCacheWriteInput<'_>) -> Self {
        Self {
            header: input.header.clone(),
            parse: input.parse.clone(),
            body_ir_coverage: input
                .body_ir
                .crates()
                .iter()
                .map(|bodies| bodies.coverage().clone())
                .collect(),
        }
    }

    /// Update the startup record of which bodies have been analyzed, leaving declarations alone.
    pub(crate) fn from_body_update(input: PackageCacheBodyUpdateInput<'_>) -> Self {
        Self {
            header: input.header.clone(),
            parse: input.parse.clone(),
            body_ir_coverage: input.body_ir.coverage(),
        }
    }
}
