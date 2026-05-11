mod build;
mod cache;
mod collect;
mod cursor;
mod data;
mod db;
mod ids;
mod import;
mod memsize;
mod path;
mod path_resolution;
mod scope;
mod txn;

use rg_arena::Arena;
use rg_parse::TargetId;
pub use rg_workspace::PackageSlot;

pub use self::cursor::DefMapCursorCandidate;

pub use self::{
    cache::DefMapPackageBundle,
    data::{DefMap, LocalDefData, LocalDefKind, LocalImplData, ModuleData, ModuleOrigin},
    db::{DefMapDb, DefMapStats},
    ids::{
        DefId, ImportId, ImportRef, LocalDefId, LocalDefRef, LocalImplId, LocalImplRef, ModuleId,
        ModuleRef, TargetRef,
    },
    import::{ImportBinding, ImportData, ImportKind, ImportPath, ImportSourcePath},
    path::{Path, PathSegment},
    path_resolution::ResolvePathResult,
    scope::{ModuleScope, ScopeBinding, ScopeEntry},
    txn::DefMapReadTxn,
};

/// Def maps for all targets inside one parsed package.
#[derive(
    Debug, Clone, PartialEq, Eq, Default, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub struct Package {
    name: String,
    target_names: Arena<TargetId, String>,
    targets: Arena<TargetId, DefMap>,
}

impl Package {
    /// Returns the Cargo package name this def-map package belongs to.
    pub fn package_name(&self) -> &str {
        &self.name
    }

    /// Returns the crate name for one target, if that target exists.
    pub fn target_name(&self, target_id: TargetId) -> Option<&str> {
        self.target_names.get(target_id).map(String::as_str)
    }

    /// Returns all target def maps in target-id order.
    pub fn targets(&self) -> &[DefMap] {
        self.targets.as_slice()
    }

    /// Returns one target def map by target id.
    pub fn target(&self, target_id: TargetId) -> Option<&DefMap> {
        self.targets.get(target_id)
    }

    fn shrink_to_fit(&mut self) {
        self.name.shrink_to_fit();
        self.target_names.shrink_to_fit();
        for target_name in self.target_names.iter_mut() {
            target_name.shrink_to_fit();
        }
        self.targets.shrink_to_fit();
        for target in self.targets.iter_mut() {
            target.shrink_to_fit();
        }
    }
}

#[cfg(test)]
mod tests;
