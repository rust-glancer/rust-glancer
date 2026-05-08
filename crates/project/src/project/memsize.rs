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

macro_rules! record_fields {
    ($recorder:expr, $owner:expr, $($field:ident),+ $(,)?) => {
        $(
            $recorder.scope(stringify!($field), |recorder| {
                $owner.$field.record_memory_children(recorder);
            });
        )+
    };
}

impl MemorySize for ProjectState {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        record_fields!(
            recorder,
            self,
            workspace,
            cargo_metadata_config,
            cache_plan,
            package_source_fingerprints,
            body_ir_policy,
            package_residency_policy,
            package_residency,
            names,
            parse,
            def_map,
            semantic_ir,
            body_ir,
        );
    }
}

impl MemorySize for WorkspaceCachePlan {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        recorder.scope("packages", |recorder| {
            self.packages.record_memory_children(recorder);
        });
    }
}

impl MemorySize for CachedPackage {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        record_fields!(
            recorder,
            self,
            package,
            package_id,
            name,
            source,
            edition,
            manifest_path,
            targets,
            dependencies,
        );
    }
}

impl MemorySize for CachedTarget {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        record_fields!(recorder, self, name, kind, src_path);
    }
}

impl MemorySize for CachedPackageSlot {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        self.0.record_memory_children(recorder);
    }
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

impl MemorySize for CachedPackageSource {
    fn record_memory_children(&self, _recorder: &mut MemoryRecorder) {}
}

impl MemorySize for CachedRustEdition {
    fn record_memory_children(&self, _recorder: &mut MemoryRecorder) {}
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

impl MemorySize for CachedDependency {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        record_fields!(
            recorder, self, package_id, name, is_normal, is_build, is_dev,
        );
    }
}

impl MemorySize for PackageCacheHeader {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        record_fields!(recorder, self, schema_version, package, source_fingerprint);
    }
}

impl MemorySize for PackageCacheSchemaVersion {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        self.0.record_memory_children(recorder);
    }
}

impl MemorySize for Fingerprint {
    fn record_memory_children(&self, _recorder: &mut MemoryRecorder) {}
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

impl MemorySize for PackageCacheArtifact {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        record_fields!(recorder, self, header, payload);
    }
}

impl MemorySize for PackageCachePayload {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        record_fields!(recorder, self, parse, def_map, semantic_ir, body_ir);
    }
}

impl MemorySize for PackageResidencyPolicy {
    fn record_memory_children(&self, _recorder: &mut MemoryRecorder) {}
}

impl MemorySize for PackageResidencyPlan {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        record_fields!(recorder, self, policy, packages);
    }
}

impl MemorySize for PackageResidency {
    fn record_memory_children(&self, _recorder: &mut MemoryRecorder) {}
}

impl MemorySize for Project {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        recorder.scope("state", |recorder| {
            self.state.record_memory_children(recorder);
        });
    }
}

impl MemorySize for SavedFileChange {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        recorder.scope("path", |recorder| {
            self.path.record_memory_children(recorder);
        });
    }
}

impl MemorySize for AnalysisChangeSummary {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        record_fields!(
            recorder,
            self,
            changed_files,
            affected_packages,
            changed_targets,
        );
    }
}

impl MemorySize for ChangedFile {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        record_fields!(recorder, self, package, file);
    }
}

impl MemorySize for FileContext {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        record_fields!(recorder, self, package, file, targets);
    }
}
