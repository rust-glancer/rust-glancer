//! Package cache payload types.
//!
//! Logical contents of one package artifact.
//!
//! The cache writes these values as one atomic revision, but it does not encode them as one wincode
//! object. [`PackageCacheProbe`] is the small startup section; DefMap and Semantic IR are separate
//! sections; Body IR is further divided into item lookup indexes and source-file shards. Writes
//! borrow those phase values through [`PackageCacheWriteInput`] instead of assembling an owned
//! aggregate.

use anyhow::Context as _;
use rg_body_ir::{CrateBodiesCoverage, PackageBodies};
use rg_def_map::PackageDefMaps as DefMapPackage;
use rg_parse::PackageParseSnapshot;
use rg_semantic_ir::PackageIr;
use rg_std::MemorySize;
use wincode::{SchemaRead, SchemaWrite};

use super::{
    fingerprint::{Fingerprint, FingerprintBuilder},
    header::PackageCacheHeader,
};

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
/// without opening the large Body IR section. Per-crate lookup fingerprints let a dirty rebuild
/// compare its new declarations with the saved indexes before decoding those indexes.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub(crate) struct PackageCacheProbe {
    pub(crate) header: PackageCacheHeader,
    pub(crate) parse: PackageParseSnapshot,
    pub(crate) body_ir_coverage: Vec<CrateBodiesCoverage>,
    pub(super) lookup_index_fingerprints: Vec<Fingerprint>,
}

impl PackageCacheProbe {
    /// Build the small validation section without serializing the retained phase payloads.
    ///
    /// DefMap and Semantic IR have matching crate slots. Their paired facts become one fingerprint
    /// per target, in the same order as Body IR stores its saved indexes.
    pub(crate) fn from_write_input(input: PackageCacheWriteInput<'_>) -> anyhow::Result<Self> {
        anyhow::ensure!(
            input.def_map.crates().len() == input.semantic_ir.crates().len(),
            "package cache has {} DefMap crates but {} semantic IR crates for item lookup indexing",
            input.def_map.crates().len(),
            input.semantic_ir.crates().len(),
        );
        let lookup_index_fingerprints = input
            .def_map
            .crates()
            .iter()
            .zip(input.semantic_ir.crates())
            .map(|(crate_data, items)| FingerprintBuilder::item_lookup_index(crate_data, items))
            .collect::<anyhow::Result<Vec<_>>>()
            .context("while attempting to fingerprint package item lookup indexes")?;

        Ok(Self {
            header: input.header.clone(),
            parse: input.parse.clone(),
            body_ir_coverage: input
                .body_ir
                .crates()
                .iter()
                .map(|crate_bodies| crate_bodies.coverage())
                .collect(),
            lookup_index_fingerprints,
        })
    }

    /// Check whether every rebuilt crate can reuse its saved item lookup index.
    ///
    /// This is an all-or-nothing package decision. Returning `false` only disables index reuse;
    /// the dirty rebuild can still construct fresh indexes from the rebuilt declarations.
    pub(crate) fn lookup_indexes_match(
        &self,
        def_map: &DefMapPackage,
        semantic_ir: &PackageIr,
    ) -> anyhow::Result<bool> {
        anyhow::ensure!(
            def_map.crates().len() == semantic_ir.crates().len(),
            "rebuilt package has {} DefMap crates but {} semantic IR crates",
            def_map.crates().len(),
            semantic_ir.crates().len(),
        );
        if self.body_ir_coverage.len() != semantic_ir.crates().len()
            || self.lookup_index_fingerprints.len() != semantic_ir.crates().len()
        {
            return Ok(false);
        }

        // A skipped or missing crate stores an empty placeholder rather than a visibility-scoped
        // index. Matching declarations do not make that placeholder reusable when the dirty build
        // materializes the crate for the first time.
        if self
            .body_ir_coverage
            .iter()
            .any(|coverage| !coverage.is_materialized())
        {
            return Ok(false);
        }

        // Recreate each key from the post-edit declarations. One mismatch is enough to reject all
        // saved indexes for this package, which keeps the later Body IR handoff simple and aligned.
        for (crate_idx, (crate_data, items)) in def_map
            .crates()
            .iter()
            .zip(semantic_ir.crates())
            .enumerate()
        {
            let rebuilt =
                FingerprintBuilder::item_lookup_index(crate_data, items).with_context(|| {
                    format!("while attempting to fingerprint item lookup index {crate_idx}")
                })?;
            if self.lookup_index_fingerprints[crate_idx] != rebuilt {
                return Ok(false);
            }
        }

        Ok(true)
    }
}
