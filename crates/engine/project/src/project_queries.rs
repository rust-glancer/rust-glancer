//! The project owns the mutable query runtime. Saved snapshots borrow only its accepted routing.

use std::{fmt, time::Duration};

use crate::{PackageResidencyPolicy, storage::cache::PackageCacheInstance};

// Change the epoch whenever file query semantics or their persisted schemas become incompatible.
const EPOCH: &str = "rust-glancer-file-declarations-1";
const RETENTION: ajika::Retention = ajika::Retention {
    max_entries: 100_000,
    max_bytes: 512 * 1024 * 1024,
    max_idle: Duration::from_secs(30 * 24 * 60 * 60),
};

pub(crate) struct ProjectQueries {
    pub(crate) database: ajika::Database,
    // A Cargo graph rebuild can replace the legacy artifact instance. Keep the original lock
    // while this database still uses that instance's directory. Phase 08 removes the split.
    _instance: PackageCacheInstance,
}

impl ProjectQueries {
    pub(crate) fn open(
        instance: PackageCacheInstance,
        residency: PackageResidencyPolicy,
    ) -> ajika::Result<Self> {
        let inputs = ajika::Inputs::default();
        let database = if residency.is_offloadable() {
            ajika::Database::open(instance.root().join("ajika"), EPOCH, inputs, (), RETENTION)?
        } else {
            ajika::Database::memory(inputs, (), RETENTION)
        };
        Ok(Self {
            database,
            _instance: instance,
        })
    }
}

impl fmt::Debug for ProjectQueries {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProjectQueries").finish_non_exhaustive()
    }
}
