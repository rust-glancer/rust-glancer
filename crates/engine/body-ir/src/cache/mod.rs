//! Body IR package cache boundary.
//!
//! Project-level cache artifacts store the retained package data directly. This wrapper keeps the
//! cache payload API stable without introducing a parallel Body IR representation.

use crate::PackageBodies;

/// One package worth of Body IR data as it will be serialized into an artifact.
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct BodyIrPackageBundle {
    package: PackageBodies,
}

impl BodyIrPackageBundle {
    pub fn new(package: PackageBodies) -> Self {
        Self { package }
    }

    pub fn package(&self) -> &PackageBodies {
        &self.package
    }

    pub fn into_package(self) -> PackageBodies {
        self.package
    }
}
