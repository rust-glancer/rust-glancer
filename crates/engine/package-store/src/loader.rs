//! Abstract package loading logic: this crate does not know how exactly
//! packages are loaded, this logic is injected.

use std::{fmt, marker::PhantomData, sync::Arc};

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

impl<T: 'static> PackageLoader<'static, T> {
    pub fn resident_only(context: &'static str) -> Self {
        Self::new(ResidentOnlyPackageLoader {
            context,
            _marker: PhantomData,
        })
    }
}

/// Loader for read transactions that should only observe already-resident packages.
struct ResidentOnlyPackageLoader<T> {
    context: &'static str,
    _marker: PhantomData<fn() -> T>,
}

impl<T> fmt::Debug for ResidentOnlyPackageLoader<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResidentOnlyPackageLoader")
            .field("context", &self.context)
            .finish()
    }
}

impl<T> LoadPackage<T> for ResidentOnlyPackageLoader<T> {
    fn load(&self, package: PackageSlot) -> Result<Arc<T>, PackageStoreError> {
        panic!(
            "{} should not load offloaded package {}",
            self.context, package.0,
        )
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
