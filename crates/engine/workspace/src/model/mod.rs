mod dependency;
mod edition;
mod metadata;
mod package;
mod target;

pub use self::{
    dependency::PackageDependency,
    edition::RustEdition,
    metadata::WorkspaceMetadata,
    package::{Package, PackageId, PackageOrigin, PackageSlot, PackageSource},
    target::{Target, TargetKind},
};
