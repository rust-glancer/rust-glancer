use rg_memsize::{MemoryRecorder, MemorySize};

use crate::{
    CargoMetadataConfig, CargoMetadataTarget, Package, PackageDependency, PackageId, PackageOrigin,
    PackageSlot, PackageSource, RustEdition, SysrootCrate, SysrootSources, Target, TargetKind,
    WorkspaceMetadata,
};

rg_memsize::impl_memory_size_leaf!(PackageSlot, PackageSource, SysrootCrate, RustEdition);

rg_memsize::impl_memory_size_children! {
    WorkspaceMetadata => workspace_root, target_cfg_options, packages, package_by_id;
    CargoMetadataConfig => target;
    SysrootSources => sysroot_root, library_root;
    Package => id, name, edition, origin, source, is_workspace_member, manifest_path, targets,
        cfg_options, dependencies;
    Target => name, kind, src_path;
    PackageDependency => package, name, is_normal, is_build, is_dev;
}

impl MemorySize for CargoMetadataTarget {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            Self::Auto => {}
            Self::Triple(target) => target.record_memory_children(recorder),
        }
    }
}

impl MemorySize for PackageId {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        self.0.record_memory_children(recorder);
    }
}

impl MemorySize for PackageOrigin {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            Self::Workspace | Self::Dependency => {}
            Self::Sysroot(krate) => krate.record_memory_children(recorder),
        }
    }
}

impl MemorySize for TargetKind {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        match self {
            Self::Lib
            | Self::Bin
            | Self::Example
            | Self::Test
            | Self::Bench
            | Self::CustomBuild => {}
            Self::Other(name) => name.record_memory_children(recorder),
        }
    }
}
