//! Adds imported names to the module scopes being built.
//!
//! A `use` path can depend on another import, and a resolved glob can gain names when its source
//! grows. The collector owns those dependencies while `NameResolver` handles path lookup and
//! visibility. Crate construction and body-local construction use the same collector.
//!
//! Scopes keep the answers already found. Named imports wait for missing namespace answers;
//! globs remember their source and forward later changes. During crate construction, the scope
//! buffers return to their builders for macro expansion, then come back here with new declarations.
//! Only the selected scope contents and unresolved-import reports survive freezing.

mod collector;

use rg_ir_model::ModuleRef;

pub use self::collector::{
    ImportCollectionEnv, ImportCollectionOutput, ImportCollector, ModuleImportInput,
    ResolvedModuleImports,
};

/// Work performed while collecting imports and forwarding glob bindings.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct ImportApplicationStats {
    pub(super) imports_evaluated: usize,
    pub(super) glob_imports_evaluated: usize,
    pub(super) glob_bindings_emitted: usize,
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
/// has two packages, three crates, and four modules. The collector reports modules in this order, so
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
            let crate_ref = module.origin.origin_crate();
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
