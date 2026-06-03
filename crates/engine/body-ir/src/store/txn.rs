//! Read transactions over frozen Body IR package data.

use rg_def_map::PackageSlot;
use rg_ir_model::{BodyRef, TargetRef};
use rg_package_store::{PackageStoreError, PackageStoreReadTxn};

use crate::{BodyData, PackageBodies, TargetBodies};

/// Read-only Body IR access for one query transaction.
#[derive(Debug, Clone)]
pub struct BodyIrReadTxn<'db> {
    packages: PackageStoreReadTxn<'db, PackageBodies>,
}

impl<'db> BodyIrReadTxn<'db> {
    pub(crate) fn from_package_store(packages: PackageStoreReadTxn<'db, PackageBodies>) -> Self {
        Self { packages }
    }

    pub fn package(&self, package: PackageSlot) -> Result<&PackageBodies, PackageStoreError> {
        self.packages.read(package)
    }

    pub fn target_bodies(
        &self,
        target: TargetRef,
    ) -> Result<Option<&TargetBodies>, PackageStoreError> {
        let package = self.package(target.package)?;
        Ok(package.target(target.target))
    }

    /// Returns one body by project-wide body reference.
    pub fn body_data(&self, body_ref: BodyRef) -> Result<Option<&BodyData>, PackageStoreError> {
        Ok(self
            .target_bodies(body_ref.target)?
            .and_then(|target_bodies| target_bodies.body(body_ref.body)))
    }
}
