//! Runs import resolution while DefMap scopes are still mutable.
//!
//! Imports can depend on names introduced by other imports, so one pass is not enough. For example:
//!
//! ```text
//! mod source { pub struct User; }
//! mod bridge { pub use crate::source::*; }
//! pub use bridge::*;
//! ```
//!
//! `bridge` gains `User` in one wave. The crate root can import that new binding only in a later
//! wave. `ScopeResolver` owns the Rust lookup rules; this module owns the repeated work around it.
//! It groups imports by their destination module, remembers which mutable module scopes each group
//! read, and reruns a group only after one of those scopes changed.
//!
//! Each rebuilt scope is paired with its unresolved-import list. Once no group needs another run,
//! both results can be frozen without one more traversal over every import in the project.

mod worklist;

use rg_ir_model::ModuleRef;

pub(super) use self::worklist::{ImportResolutionExecutor, ImportWorklist};

/// Import counters collected by the workers in one wave.
///
/// Every module job can update these counters without shared state; the coordinator merges them
/// after all jobs finish. Package, crate, and module counts come from the worklist itself and live
/// in `ModuleSetShape` instead.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct ImportApplicationStats {
    pub(super) imports_evaluated: usize,
    pub(super) glob_imports_evaluated: usize,
    pub(super) glob_bindings_emitted: usize,
}

impl ImportApplicationStats {
    pub(super) fn merge(&mut self, other: Self) {
        self.imports_evaluated += other.imports_evaluated;
        self.glob_imports_evaluated += other.glob_imports_evaluated;
        self.glob_bindings_emitted += other.glob_bindings_emitted;
    }
}

/// Number of packages, crates, and modules represented by a sorted module list.
///
/// For example, this list:
///
/// ```text
/// package 0 / crate 0 / module 1
/// package 0 / crate 0 / module 3
/// package 0 / crate 1 / module 0
/// package 2 / crate 0 / module 0
/// ```
///
/// has two packages, three crates, and four modules. Worklist modules already use this order, so
/// adjacent package and crate changes give the counts without allocating temporary sets.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct ModuleSetShape {
    pub(super) packages: usize,
    pub(super) crates: usize,
    pub(super) modules: usize,
}

impl ModuleSetShape {
    /// Count a module list sorted by package, crate, then module id.
    ///
    /// Calling this with another order can count the same package or crate more than once.
    pub(super) fn from_sorted_modules(modules: &[ModuleRef]) -> Self {
        let mut shape = Self {
            modules: modules.len(),
            ..Self::default()
        };
        let mut previous_package = None;
        let mut previous_crate = None;

        for module in modules {
            let crate_ref = module
                .origin
                .as_crate_ref()
                .expect("crate import should belong to a crate module");
            if previous_package != Some(crate_ref.package) {
                shape.packages += 1;
                previous_package = Some(crate_ref.package);
            }
            if previous_crate != Some(crate_ref) {
                shape.crates += 1;
                previous_crate = Some(crate_ref);
            }
        }

        shape
    }
}
