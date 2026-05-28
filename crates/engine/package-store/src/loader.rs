//! Abstract package loading logic: this crate does not know how exactly
//! packages are loaded, this logic is injected.

use std::sync::Arc;

use rg_workspace::PackageSlot;

use crate::PackageStoreError;

/// Loads one offloaded package payload into a package-store read transaction.
pub trait LoadPackage<T>: std::fmt::Debug + Send + Sync {
    fn load(&self, slot: PackageSlot) -> Result<Arc<T>, PackageStoreError>;
}

/// Shared loader used by package-store read transactions to materialize offloaded slots.
pub struct PackageLoader<'db, T> {
    loader: Arc<dyn LoadPackage<T> + Send + Sync + 'db>,
}

impl<'db, T> PackageLoader<'db, T> {
    pub fn new(loader: impl LoadPackage<T> + 'db) -> Self {
        Self {
            loader: Arc::new(loader),
        }
    }

    pub fn from_arc(loader: Arc<impl LoadPackage<T> + 'db>) -> Self {
        Self { loader }
    }

    pub(crate) fn load(&self, slot: PackageSlot) -> Result<Arc<T>, PackageStoreError> {
        self.loader.load(slot)
    }
}

impl<T> Clone for PackageLoader<'_, T> {
    fn clone(&self) -> Self {
        Self {
            loader: Arc::clone(&self.loader),
        }
    }
}

impl<T> std::fmt::Debug for PackageLoader<'_, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.loader.fmt(f)
    }
}
