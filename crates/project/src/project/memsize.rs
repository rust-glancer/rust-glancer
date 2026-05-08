use rg_memsize::{MemoryRecorder, MemorySize};

use crate::cache::{
    CachedDependency, CachedPackage, CachedPackageId, CachedPackageSlot, CachedPackageSource,
    CachedPath, CachedRustEdition, CachedTarget, CachedTargetKind, Fingerprint,
    PackageCacheArtifact, PackageCacheBodyIrState, PackageCacheHeader, PackageCachePayload,
    PackageCacheSchemaVersion, WorkspaceCachePlan,
};
use crate::{PackageResidency, PackageResidencyPlan, PackageResidencyPolicy};

use super::{
    AnalysisChangeSummary, ChangedFile, FileContext, Project, SavedFileChange, state::ProjectState,
};

rg_memsize::impl_memory_size_leaf!(
    CachedPackageSlot,
    CachedPackageSource,
    CachedRustEdition,
    PackageCacheSchemaVersion,
    Fingerprint,
    PackageResidencyPolicy,
    PackageResidency,
);

rg_memsize::impl_memory_size_children! {
    ProjectState => workspace, cargo_metadata_config, cache_plan, package_source_fingerprints,
        body_ir_policy, package_residency_policy, package_residency, names, parse, def_map,
        semantic_ir, body_ir;
    WorkspaceCachePlan => packages;
    CachedPackage => package, package_id, name, source, edition, manifest_path, targets,
        dependencies;
    CachedTarget => name, kind, src_path;
    CachedDependency => package_id, name, is_normal, is_build, is_dev;
    PackageCacheHeader => schema_version, package, source_fingerprint;
    PackageCacheArtifact => header, payload;
    PackageCachePayload => parse, def_map, semantic_ir, body_ir;
    PackageResidencyPlan => policy, packages;
    Project => state;
    SavedFileChange => path;
    AnalysisChangeSummary => changed_files, affected_packages, changed_targets;
    ChangedFile => package, file;
    FileContext => package, file, targets;
}

impl MemorySize for CachedPackageId {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        self.0.record_memory_children(recorder);
    }
}

impl MemorySize for CachedPath {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        self.0.record_memory_children(recorder);
    }
}

impl MemorySize for CachedTargetKind {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            Self::Other(kind) => kind.record_memory_children(recorder),
            Self::Lib
            | Self::Bin
            | Self::Example
            | Self::Test
            | Self::Bench
            | Self::CustomBuild => {}
        }
    }
}

impl MemorySize for PackageCacheBodyIrState {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            Self::Built(bundle) => recorder.scope("built", |recorder| {
                bundle.record_memory_children(recorder);
            }),
            Self::SkippedByPolicy => {}
        }
    }
}
