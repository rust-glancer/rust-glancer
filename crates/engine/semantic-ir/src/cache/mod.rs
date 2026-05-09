//! Semantic IR package cache boundary.
//!
//! Project-level cache artifacts store the retained package data directly. This wrapper keeps the
//! cache payload API stable without introducing a parallel Semantic IR representation.

use crate::PackageIr;

/// One package worth of Semantic IR data as it will be serialized into an artifact.
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct SemanticIrPackageBundle {
    package: PackageIr,
}

impl SemanticIrPackageBundle {
    pub fn new(package: PackageIr) -> Self {
        Self { package }
    }

    pub fn package(&self) -> &PackageIr {
        &self.package
    }

    pub fn into_package(self) -> PackageIr {
        self.package
    }
}
