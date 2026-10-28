//! Outputs of one selected-package DefMap build.
//!
//! Frozen DefMap data and generated declaration payloads deliberately have different lifetimes.
//! The former becomes saved project state. The latter exists only long enough for Semantic IR to
//! copy signatures and declaration facts, then it is dropped before Body IR or cache writing.

use std::collections::HashMap;

use rg_ir_model::{CrateRef, PackageSlot};
use rg_parse::Package;
use rg_std::MemorySize;

use crate::{CrateData, DefMapDb, GeneratedItemStore, MacroExpansionLimitReport, PackageDefMaps};

/// Generated declaration stores for the crates rebuilt by one DefMap build.
#[derive(Debug, Clone, Default, MemorySize)]
pub struct GeneratedItemStores {
    crates: HashMap<CrateRef, GeneratedItemStore>,
}

impl GeneratedItemStores {
    pub fn crate_items(&self, crate_ref: CrateRef) -> Option<&GeneratedItemStore> {
        self.crates.get(&crate_ref)
    }

    pub(crate) fn insert(&mut self, crate_ref: CrateRef, items: GeneratedItemStore) {
        let previous = self.crates.insert(crate_ref, items);
        debug_assert!(previous.is_none(), "generated crate items inserted twice");
    }
}

/// Frozen DefMap replacement plus the transient declarations needed by the next build phase.
#[derive(Debug, MemorySize)]
pub struct DefMapBuildOutput {
    def_map: DefMapDb,
    generated_items: GeneratedItemStores,
}

impl DefMapBuildOutput {
    pub(crate) fn new(def_map: DefMapDb, generated_items: GeneratedItemStores) -> Self {
        Self {
            def_map,
            generated_items,
        }
    }

    pub fn into_parts(self) -> (DefMapDb, GeneratedItemStores) {
        (self.def_map, self.generated_items)
    }
}

/// Completed data from one crate. Publishing this result never needs the mutable builder.
pub(crate) struct CrateBuildOutput {
    pub(crate) crate_ref: CrateRef,
    pub(crate) data: CrateData,
    pub(crate) generated_items: GeneratedItemStore,
    pub(crate) macro_expansion_limit: Option<MacroExpansionLimitReport>,
}

struct CompletedPackage {
    crates: Vec<Option<CrateData>>,
    macro_expansion_limits: Vec<MacroExpansionLimitReport>,
}

/// Frozen results of selected crates, addressed by their original package and target slots.
///
/// A selected slot starts empty and becomes readable only after its crate has finished. Keeping
/// that distinction from an unselected package prevents dependency lookup from reading old data
/// while a replacement is still pending. These values move into package outputs without cloning.
pub(crate) struct CompletedCrateStore {
    packages: Vec<Option<CompletedPackage>>,
    generated_items: GeneratedItemStores,
}

impl CompletedCrateStore {
    pub(crate) fn new(packages: &[Package], selected: &[PackageSlot]) -> Self {
        let mut completed = Self {
            packages: (0..packages.len()).map(|_| None).collect(),
            generated_items: GeneratedItemStores::default(),
        };
        for slot in selected {
            completed.packages[slot.0] = Some(CompletedPackage {
                crates: (0..packages[slot.0].targets().len())
                    .map(|_| None)
                    .collect(),
                macro_expansion_limits: Vec::new(),
            });
        }
        completed
    }

    pub(crate) fn contains_package(&self, package: PackageSlot) -> bool {
        self.packages.get(package.0).is_some_and(Option::is_some)
    }

    pub(crate) fn crate_data(&self, krate: CrateRef) -> Option<&CrateData> {
        self.packages
            .get(krate.package.0)?
            .as_ref()?
            .crates
            .get(krate.crate_id.0)?
            .as_ref()
    }

    /// Publish one finished crate for its dependents and retain its other outputs for packaging.
    pub(crate) fn insert(&mut self, output: CrateBuildOutput) {
        let completed = self.packages[output.crate_ref.package.0]
            .as_mut()
            .expect("rebuilt crate has a selected package");
        if let Some(report) = output.macro_expansion_limit {
            completed.macro_expansion_limits.push(report);
        }
        self.generated_items
            .insert(output.crate_ref, output.generated_items);
        let slot = &mut completed.crates[output.crate_ref.crate_id.0];
        assert!(slot.is_none(), "crate should be completed once");
        *slot = Some(output.data);
    }

    pub(crate) fn take_package(&mut self, slot: PackageSlot, package: &Package) -> PackageDefMaps {
        let completed = self.packages[slot.0]
            .take()
            .expect("selected package has completed crates");
        PackageDefMaps::new(
            package.package_name().to_string(),
            package.edition(),
            completed
                .crates
                .into_iter()
                .map(|krate| krate.expect("all selected crates should be complete"))
                .collect(),
            completed.macro_expansion_limits,
        )
    }

    pub(crate) fn into_output(self, def_map: DefMapDb) -> DefMapBuildOutput {
        DefMapBuildOutput::new(def_map, self.generated_items)
    }
}
