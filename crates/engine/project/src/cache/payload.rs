//! Package cache payload types.
//!
//! Logical contents of one package artifact.
//!
//! The cache writes these values as one atomic revision, but it does not encode them as one wincode
//! object. [`PackageCacheProbe`] is the small startup section. DefMap is split into crate payloads,
//! Semantic IR splits each crate into declarations and a lookup index, and Body IR uses source-file
//! shards. Writes borrow those phase values through [`PackageCacheWriteInput`] instead of assembling
//! an owned aggregate.

use rg_body_ir::{CrateBodiesCoverage, PackageBodies};
use rg_def_map::{PackageDefMaps as DefMapPackage, PackageDefMapsManifest};
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

/// Resident data used when an exact Body IR rebuild preserves cached declarations.
///
/// DefMap and Semantic IR are deliberately absent. Their encoded sections are copied from the
/// pinned prior artifact, while this input supplies the new source snapshot and Body IR coverage.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PackageCacheBodyUpdateInput<'a> {
    pub(crate) header: &'a PackageCacheHeader,
    pub(crate) parse: &'a PackageParseSnapshot,
    pub(crate) body_ir: &'a PackageBodies,
}

impl<'a> PackageCacheBodyUpdateInput<'a> {
    pub(crate) fn new(
        header: &'a PackageCacheHeader,
        parse: &'a PackageParseSnapshot,
        body_ir: &'a PackageBodies,
    ) -> Self {
        Self {
            header,
            parse,
            body_ir,
        }
    }
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
/// The probe owns source identity and Body IR coverage. The DefMap directory is also retained
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
        Self::from_body_update(PackageCacheBodyUpdateInput::new(
            input.header,
            input.parse,
            input.body_ir,
        ))
    }

    /// Rebuilds startup data after a Body-only update without touching declaration payloads.
    pub(crate) fn from_body_update(input: PackageCacheBodyUpdateInput<'_>) -> Self {
        Self {
            header: input.header.clone(),
            parse: input.parse.clone(),
            body_ir_coverage: input
                .body_ir
                .crates()
                .iter()
                .map(|crate_bodies| crate_bodies.coverage())
                .collect(),
        }
    }
}
