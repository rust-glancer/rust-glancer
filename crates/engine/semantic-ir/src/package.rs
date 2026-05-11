use rg_arena::Arena;
use rg_parse::TargetId;

use crate::TargetIr;

/// Semantic IR for one Cargo package.
///
/// Packages keep target IRs in the same stable order as parse/def-map packages, so a
/// `TargetRef { package, target }` can address every phase without an extra translation table.
#[derive(
    Debug, Clone, PartialEq, Eq, Default, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub struct PackageIr {
    pub(crate) targets: Arena<TargetId, TargetIr>,
}

impl PackageIr {
    pub(crate) fn new(targets: Vec<TargetIr>) -> Self {
        Self {
            targets: Arena::from_vec(targets),
        }
    }

    pub(crate) fn shrink_to_fit(&mut self) {
        self.targets.shrink_to_fit();
        for target in self.targets.iter_mut() {
            target.shrink_to_fit();
        }
    }

    /// Returns all target IRs for this package in target-id order.
    pub fn targets(&self) -> &[TargetIr] {
        self.targets.as_slice()
    }

    /// Returns one target IR by package-local target id.
    pub fn target(&self, target: TargetId) -> Option<&TargetIr> {
        self.targets.get(target)
    }

    pub(crate) fn target_mut(&mut self, target: TargetId) -> Option<&mut TargetIr> {
        self.targets.get_mut(target)
    }
}
