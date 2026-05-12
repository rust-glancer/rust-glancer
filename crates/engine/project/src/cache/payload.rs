//! Package cache payload types.
//!
//! One package artifact stores the retained analysis phases together. Keeping the phases bundled
//! prevents cache states where DefMap, Semantic IR, and Body IR come from different builds.

use rg_body_ir::BodyIrPackageBundle;
use rg_def_map::DefMapPackageBundle;
use rg_parse::PackageParseSnapshot;
use rg_semantic_ir::SemanticIrPackageBundle;

use super::header::PackageCacheHeader;

/// Body IR payload state for one package artifact.
///
/// `SkippedByPolicy` is valid only when the current Body IR build policy does not require bodies
/// for this package. If a later policy needs bodies, the whole package artifact should be rejected
/// and rebuilt through the normal project-level path.
#[derive(Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub enum PackageCacheBodyIrState {
    Built(Box<BodyIrPackageBundle>),
    SkippedByPolicy,
}

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
    pub def_map: DefMapPackageBundle,
    pub semantic_ir: SemanticIrPackageBundle,
    pub body_ir: PackageCacheBodyIrState,
}

impl PackageCachePayload {
    pub fn new(
        parse: PackageParseSnapshot,
        def_map: DefMapPackageBundle,
        semantic_ir: SemanticIrPackageBundle,
        body_ir: PackageCacheBodyIrState,
    ) -> Self {
        Self {
            parse,
            def_map,
            semantic_ir,
            body_ir,
        }
    }
}
