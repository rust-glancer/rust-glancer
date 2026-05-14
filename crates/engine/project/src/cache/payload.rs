//! Package cache payload types.
//!
//! One package artifact stores the retained analysis phases together. Keeping the phases bundled
//! prevents cache states where DefMap, Semantic IR, and Body IR come from different builds.

use rg_body_ir::PackageBodies;
use rg_def_map::Package as DefMapPackage;
use rg_parse::PackageParseSnapshot;
use rg_semantic_ir::PackageIr;

use super::header::PackageCacheHeader;

/// One package artifact containing every retained analysis phase we currently cache.
#[derive(Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub struct PackageCacheArtifact {
    pub header: PackageCacheHeader,
    pub payload: PackageCachePayload,
}

impl PackageCacheArtifact {
    pub fn new(header: PackageCacheHeader, payload: PackageCachePayload) -> Self {
        Self { header, payload }
    }
}

/// Retained package data stored together to avoid mismatched phase fragments.
#[derive(Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
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
