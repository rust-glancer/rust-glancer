//! DefMap package cache boundary.
//!
//! Project-level cache artifacts store the retained package data directly. This wrapper keeps the
//! cache payload API stable without introducing a parallel DefMap representation.

use crate::Package;

/// One package worth of DefMap data as it will be serialized into an artifact.
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct DefMapPackageBundle {
    package: Package,
}

impl DefMapPackageBundle {
    pub fn new(package: Package) -> Self {
        Self { package }
    }

    pub fn package(&self) -> &Package {
        &self.package
    }

    pub fn into_package(self) -> Package {
        self.package
    }
}
