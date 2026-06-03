//! Read transaction handles for package-store payloads.

use std::sync::{Arc, OnceLock};

use rg_workspace::PackageSlot;

use crate::{PackageLoader, PackageStoreError};

/// Read-only package-store view used by query transactions.
pub struct PackageStoreReadTxn<'db, T> {
    packages: Vec<PackageReadEntry<'db, T>>,
}

impl<'db, T> PackageStoreReadTxn<'db, T> {
    /// Materializes every included package.
    ///
    /// Important: package order is stable, but there must be no assumptions about `PackageSlot`
    /// values for packages.
    /// If you need to have a mapping from package slot to package, it's recommended
    /// to iterate via `PackageSlot` and use `read` instead.
    pub fn included_packages(&self) -> impl Iterator<Item = Result<&T, PackageStoreError>> {
        (0..self.packages.len()).filter_map(|id| {
            if self.packages[id].is_excluded() {
                None
            } else {
                Some(self.read(PackageSlot(id)))
            }
        })
    }

    pub(crate) fn from_store_entries(
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
        }
    }

    pub fn read(&self, package: PackageSlot) -> Result<&T, PackageStoreError> {
        let Some(entry) = self.packages.get(package.0) else {
            return Err(PackageStoreError::MissingSlot { slot: package });
        };
        entry.read(package)
    }
}

impl<T> Clone for PackageStoreReadTxn<'_, T> {
    fn clone(&self) -> Self {
        Self {
            packages: self.packages.clone(),
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

impl<'db, T> Clone for PackageReadEntry<'db, T> {
    fn clone(&self) -> Self {
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
}
