//! Package cache payload types.
//!
//! Logical contents of one package artifact.
//!
//! The cache writes these values as one atomic revision, but it does not encode them as one wincode
//! object. [`PackageCacheProbe`] is the small startup section; DefMap and Semantic IR are separate
//! sections; Body IR is further divided into target indexes and source-file shards. Writes borrow
//! those phase values through [`PackageCacheWriteInput`] instead of assembling an owned aggregate.

use rg_body_ir::{PackageBodies, TargetBodiesCoverage};
use rg_ir_storage::PackageDefMaps as DefMapPackage;
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
pub struct PackageCacheProbe {
    pub header: PackageCacheHeader,
    pub parse: PackageParseSnapshot,
    pub body_ir_coverage: Vec<TargetBodiesCoverage>,
}

impl PackageCacheProbe {
    pub(crate) fn from_write_input(input: PackageCacheWriteInput<'_>) -> Self {
        Self {
            header: input.header.clone(),
            parse: input.parse.clone(),
            body_ir_coverage: input
                .body_ir
                .targets()
                .iter()
                .map(|target| target.coverage())
                .collect(),
        }
    }
}
