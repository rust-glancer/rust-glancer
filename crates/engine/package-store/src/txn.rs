//! Read transaction handles for package-store payloads.

use std::{
    ops::Deref,
    sync::{Arc, OnceLock},
};

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

    fn load(&self, slot: PackageSlot) -> Result<Arc<T>, PackageStoreError> {
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

/// Read-only package-store view used by query transactions.
pub struct PackageStoreReadTxn<'db, T> {
    packages: Vec<PackageReadEntry<'db, T>>,
    _marker: std::marker::PhantomData<&'db T>,
}

impl<'db, T> PackageStoreReadTxn<'db, T> {
    pub(crate) fn from_store_entries(
        packages: impl IntoIterator<Item = Option<Arc<T>>>,
        loader: PackageLoader<'db, T>,
    ) -> Self {
        Self::from_subset_store_entries(packages.into_iter().map(|package| (true, package)), loader)
    }

    pub(crate) fn from_subset_store_entries(
        packages: impl IntoIterator<Item = (bool, Option<Arc<T>>)>,
        loader: PackageLoader<'db, T>,
    ) -> Self {
        Self {
            packages: packages
                .into_iter()
                .map(|(included, package)| {
                    if !included {
                        return PackageReadEntry::Excluded;
                    }

                    match package {
                        Some(package) => PackageReadEntry::Ready(package),
                        None => PackageReadEntry::Lazy {
                            loaded: OnceLock::new(),
                            loader: loader.clone(),
                        },
                    }
                })
                .collect(),
            _marker: std::marker::PhantomData,
        }
    }

    pub fn read(&self, package: PackageSlot) -> Result<PackageRead<'_, T>, PackageStoreError> {
        let Some(entry) = self.packages.get(package.0) else {
            return Err(PackageStoreError::MissingSlot { slot: package });
        };
        entry.read(package).map(PackageRead)
    }

    /// Materializes every included package together with its original package slot.
    pub fn materialize_included_packages_with_slots(
        &self,
    ) -> Result<Vec<(PackageSlot, PackageRead<'_, T>)>, PackageStoreError> {
        let mut packages = Vec::new();

        for package_idx in 0..self.packages.len() {
            let package = PackageSlot(package_idx);
            if self.packages[package_idx].is_excluded() {
                continue;
            }
            packages.push((package, self.read(package)?));
        }

        Ok(packages)
    }
}

impl<T> Clone for PackageStoreReadTxn<'_, T> {
    fn clone(&self) -> Self {
        Self {
            packages: self
                .packages
                .iter()
                .map(PackageReadEntry::clone_for_txn)
                .collect(),
            _marker: std::marker::PhantomData,
        }
    }
}

impl<T> std::fmt::Debug for PackageStoreReadTxn<'_, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PackageStoreReadTxn")
            .field("package_count", &self.packages.len())
            .finish()
    }
}

enum PackageReadEntry<'db, T> {
    Ready(Arc<T>),
    Lazy {
        loaded: OnceLock<Arc<T>>,
        loader: PackageLoader<'db, T>,
    },
    Excluded,
}

impl<T> PackageReadEntry<'_, T> {
    fn read(&self, package: PackageSlot) -> Result<&T, PackageStoreError> {
        match self {
            Self::Ready(package) => Ok(package.as_ref()),
            Self::Lazy { loaded, loader } => {
                if let Some(package) = loaded.get() {
                    return Ok(package.as_ref());
                }

                let package_data = loader.load(package)?;
                let _ = loaded.set(package_data);
                Ok(loaded
                    .get()
                    .expect("lazy package entry should be initialized after successful load")
                    .as_ref())
            }
            Self::Excluded => Err(PackageStoreError::ExcludedSlot { slot: package }),
        }
    }

    fn is_excluded(&self) -> bool {
        matches!(self, Self::Excluded)
    }

    fn clone_for_txn(&self) -> Self {
        match self {
            Self::Ready(package) => Self::Ready(Arc::clone(package)),
            Self::Lazy { loaded, loader } => match loaded.get() {
                Some(package) => Self::Ready(Arc::clone(package)),
                None => Self::Lazy {
                    loaded: OnceLock::new(),
                    loader: loader.clone(),
                },
            },
            Self::Excluded => Self::Excluded,
        }
    }
}

/// One package payload read through a package-store transaction.
#[derive(Debug)]
pub struct PackageRead<'db, T>(&'db T);

impl<'db, T> PackageRead<'db, T> {
    pub fn into_ref(self) -> &'db T {
        self.0
    }
}

impl<T> Clone for PackageRead<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for PackageRead<'_, T> {}

impl<T> Deref for PackageRead<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.0
    }
}
