mod dependency;
mod metadata;
mod package;
mod target;

pub use self::{
    dependency::PackageDependency,
    metadata::WorkspaceMetadata,
    package::{Package, PackageId, PackageOrigin, PackageSlot, PackageSource},
    target::{Target, TargetKind},
};
