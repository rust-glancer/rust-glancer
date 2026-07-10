//! Package cache payload types.
//!
//! Logical contents of one package artifact.
//!
//! The cache writes these values as one atomic revision, but it does not encode them as one wincode
//! object. [`PackageCacheProbe`] is the small startup section; DefMap and Semantic IR are separate
//! sections; Body IR is further divided into target indexes and source-file shards. The aggregate
//! types here remain useful for building artifacts and for round-trip tests.

use rg_body_ir::{PackageBodies, TargetBodiesCoverage};
use rg_ir_storage::PackageDefMaps as DefMapPackage;
use rg_parse::PackageParseSnapshot;
use rg_semantic_ir::PackageIr;
use rg_std::MemorySize;
use wincode::{SchemaRead, SchemaWrite};

use super::header::PackageCacheHeader;

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
    pub fn from_artifact(artifact: &PackageCacheArtifact) -> Self {
        Self {
            header: artifact.header.clone(),
            parse: artifact.payload.parse.clone(),
            body_ir_coverage: artifact
                .payload
                .body_ir
                .targets()
                .iter()
                .map(|target| target.coverage())
                .collect(),
        }
    }
}

/// Logical view of every retained analysis phase in one atomic package revision.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub struct PackageCacheArtifact {
    pub header: PackageCacheHeader,
    pub payload: PackageCachePayload,
}

impl PackageCacheArtifact {
    pub fn new(header: PackageCacheHeader, payload: PackageCachePayload) -> Self {
        Self { header, payload }
    }
}

/// In-memory input used to encode all independently decodable sections.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize)]
pub struct PackageCachePayload {
    pub parse: PackageParseSnapshot,
    pub def_map: DefMapPackage,
    pub semantic_ir: PackageIr,
    pub body_ir: PackageBodies,
}

impl PackageCachePayload {
    pub fn new(
        parse: PackageParseSnapshot,
        def_map: DefMapPackage,
        semantic_ir: PackageIr,
        body_ir: PackageBodies,
    ) -> Self {
        Self {
            parse,
            def_map,
            semantic_ir,
            body_ir,
        }
    }
}
