//! Item names available in a module or a body-local declaration scope.
//!
//! An item scope maps each spelling to its type/value/macro bindings. It also keeps traits imported
//! as `_`, which affect method lookup without introducing a name. Lexical variable bindings live
//! in Body IR; a body-local item scope only handles declarations and imports.
//!
//! Construction uses a hash map and shared storage for extra namespace slots. Freezing sorts the
//! names and keeps compact owned bindings. The public binding view hides those storage choices
//! from name resolution.

mod storage;

use rg_ir_model::{DefId, ImportId, ModuleRef};
use rg_std::{MemorySize, Shrink};
use rg_text::Name;
use rustc_hash::FxHashMap;
use wincode::{SchemaRead, SchemaWrite};

pub use self::storage::{NameBindings, NameBindingsView};
use self::storage::{NameBindingsBuilder, SlotArena};
use crate::{NameBinding, Namespace};

/// Item names and unnamed trait imports retained for lookup after construction.
///
/// Build-time import resolution uses `ItemScopeBuilder`; once scopes stabilize, entries are
/// sorted and boxed here so retained modules do not keep hash-table and `Vec` capacity overhead.
#[derive(Debug, Clone, PartialEq, Eq, Default, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct ItemScope {
    entries: Box<[NameEntry]>,
    /// Traits imported with an underscore are in method scope without occupying a type name.
    ///
    /// For example, `use crate::Render as _;` makes `value.render()` legal while leaving
    /// `Render` unavailable as a path. Keeping that fact outside `entries` preserves both sides
    /// of the language rule instead of inventing a synthetic name for the import.
    unnamed_trait_bindings: Box<[NameBinding]>,
}

impl ItemScope {
    pub fn entry(&self, name: &str) -> Option<&NameBindings> {
        self.entries
            .binary_search_by(|entry| entry.name.as_str().cmp(name))
            .ok()
            .map(|idx| &self.entries[idx].bindings)
    }

    pub fn entries(&self) -> impl Iterator<Item = (&Name, &NameBindings)> {
        self.entries
            .iter()
            .map(|entry| (&entry.name, &entry.bindings))
    }

    pub(crate) fn unnamed_trait_bindings(&self) -> &[NameBinding] {
        &self.unnamed_trait_bindings
    }
}

/// One spelling and its namespace bindings in the sorted item table.
///
/// `ItemScope::entry` binary-searches these values by `name`; the separate `NameBindings` owns the
/// sparse type/value/macro results for that spelling.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
struct NameEntry {
    name: Name,
    bindings: NameBindings,
}

/// Collects item declarations and import answers before producing an `ItemScope`.
///
/// Declarations and imports extend the same name table. Each spelling gathers its namespace
/// answers while lookup reads them through `NameBindingsView`. Freezing produces owned
/// `NameBindings` and discards the import IDs used to track which routes won during construction.
///
/// The ID answers "which `use` may refresh this target?"; presentation attributes answer "which
/// `use` should supply documentation and stability flags?". Those can be different routes to the
/// same definition.
#[derive(Debug, Clone, Default)]
pub struct ItemScopeBuilder {
    names: FxHashMap<Name, NameBindingsBuilder>,
    extra_slots: SlotArena,
    unnamed_trait_bindings: Vec<NameBinding>,
}

impl ItemScopeBuilder {
    /// Store the binding selected by declaration collection or import resolution.
    pub fn set_binding(&mut self, name: &Name, namespace: Namespace, binding: NameBinding) {
        self.names.entry(name.clone()).or_default().set_binding(
            namespace,
            binding,
            None,
            &mut self.extra_slots,
        )
    }

    /// Import allowed to refresh this slot's target, independent of its presentation attributes.
    pub(crate) fn binding_import(&self, name: &str, namespace: Namespace) -> Option<ImportId> {
        self.names
            .get(name)?
            .binding_import(namespace, &self.extra_slots)
    }

    /// Store the selected answer together with the import route that won the slot.
    /// A slot selected from a direct declaration has no winning import ID.
    pub(crate) fn set_import_binding(
        &mut self,
        name: &Name,
        namespace: Namespace,
        binding: NameBinding,
        import: Option<ImportId>,
    ) {
        self.names.entry(name.clone()).or_default().set_binding(
            namespace,
            binding,
            import,
            &mut self.extra_slots,
        );
    }

    pub fn entry(&self, name: &str) -> Option<NameBindingsView<'_>> {
        self.names
            .get(name)
            .map(|entry| entry.as_view(&self.extra_slots))
    }

    pub fn binding(&self, name: &str, namespace: Namespace) -> Option<&NameBinding> {
        self.entry(name)?.binding(namespace)
    }

    pub fn entries(&self) -> impl Iterator<Item = (&Name, NameBindingsView<'_>)> {
        self.names
            .iter()
            .map(|(name, entry)| (name, entry.as_view(&self.extra_slots)))
    }

    pub(crate) fn unnamed_trait_binding(&self, def: DefId) -> Option<&NameBinding> {
        self.unnamed_trait_bindings
            .iter()
            .find(|binding| binding.def == def)
    }

    /// Store one selected `use Trait as _` binding without adding a path-visible name.
    pub(crate) fn set_unnamed_trait_binding(&mut self, binding: NameBinding) {
        if let Some(existing) = self
            .unnamed_trait_bindings
            .iter_mut()
            .find(|existing| existing.def == binding.def)
        {
            *existing = binding;
        } else {
            self.unnamed_trait_bindings.push(binding);
        }
    }

    /// Restrict public visibility except for bindings explicitly allowed by the caller.
    ///
    /// Proc-macro targets use this after each import-resolution step: their implementation items
    /// remain visible throughout the defining crate, while only direct proc-macro exports can be
    /// named by downstream crates.
    pub(crate) fn censor_public_bindings(
        &mut self,
        visible_from: ModuleRef,
        mut should_remain_public: impl FnMut(Namespace, &NameBinding) -> bool,
    ) {
        for entry in self.names.values_mut() {
            entry.for_each_binding_mut(&mut self.extra_slots, |namespace, binding| {
                if !should_remain_public(namespace, binding) {
                    binding.restrict_public_visibility_to(visible_from);
                }
            });
        }
        for binding in &mut self.unnamed_trait_bindings {
            if !should_remain_public(Namespace::Types, binding) {
                binding.restrict_public_visibility_to(visible_from);
            }
        }
    }

    /// Turn a settled build scope into the compact representation retained by queries.
    ///
    /// Every name drains the extra namespace nodes it owns. Names are then sorted so lookup,
    /// equality, and serialization are independent of hash-map and import insertion order.
    pub fn freeze(self) -> ItemScope {
        let mut extra_slots = self.extra_slots;
        let mut entries = self
            .names
            .into_iter()
            .map(|(name, entry)| NameEntry {
                name,
                bindings: entry.freeze(&mut extra_slots),
            })
            .collect::<Vec<_>>();
        debug_assert!(
            extra_slots.is_drained(),
            "every extra slot belongs to one frozen name"
        );
        entries.sort_by(|left, right| left.name.cmp(&right.name));

        ItemScope {
            entries: entries.into_boxed_slice(),
            unnamed_trait_bindings: self.unnamed_trait_bindings.into_boxed_slice(),
        }
    }
}

#[cfg(test)]
mod tests {
    use rg_ir_model::{CrateId, CrateRef, DefId, ModuleId, ModuleRef, PackageSlot};
    use rg_text::Name;

    use super::ItemScopeBuilder;
    use crate::{NameBinding, NameBindingProvenance, Namespace, Visibility};

    #[test]
    fn frozen_scope_looks_up_sorted_entries() {
        let mut scope = ItemScopeBuilder::default();
        scope.set_binding(&Name::new("zeta"), Namespace::Types, direct_binding(0));
        scope.set_binding(&Name::new("alpha"), Namespace::Values, direct_binding(1));

        let frozen = scope.freeze();
        let names = frozen
            .entries()
            .map(|(name, _)| name.to_string())
            .collect::<Vec<_>>();

        assert_eq!(names, ["alpha", "zeta"]);
        assert!(
            frozen
                .entry("alpha")
                .expect("entry should exist")
                .binding(Namespace::Values)
                .is_some()
        );
        assert!(frozen.entry("missing").is_none());
    }

    #[test]
    fn frozen_scope_preserves_occupied_namespace_combinations() {
        let cases: [(&str, &[Namespace]); 7] = [
            ("type only", &[Namespace::Types]),
            ("value only", &[Namespace::Values]),
            ("macro only", &[Namespace::Macros]),
            ("type and value", &[Namespace::Types, Namespace::Values]),
            ("type and macro", &[Namespace::Types, Namespace::Macros]),
            ("value and macro", &[Namespace::Values, Namespace::Macros]),
            ("all", &Namespace::ALL),
        ];

        for (case, occupied) in cases {
            let mut scope = ItemScopeBuilder::default();
            let name = Name::new("Item");
            for (index, namespace) in occupied.iter().copied().enumerate() {
                scope.set_binding(&name, namespace, direct_binding(index));
            }

            assert_eq!(
                scope.entries().count(),
                1,
                "{case}: visit each spelling once"
            );
            let before = Namespace::ALL.map(|namespace| {
                scope
                    .entry("Item")
                    .expect("inserted name should exist")
                    .binding(namespace)
                    .copied()
            });
            let frozen = scope.freeze();
            let entry = frozen.entry("Item").expect("inserted name should freeze");
            for (namespace, before) in Namespace::ALL.into_iter().zip(before) {
                assert_eq!(
                    entry.binding(namespace).copied(),
                    before,
                    "{case}: preserve lookup"
                );
                assert_eq!(
                    entry.binding(namespace).is_some(),
                    occupied.contains(&namespace),
                    "{case}: unexpected {namespace:?} occupancy",
                );
            }
        }
    }

    #[test]
    fn frozen_scope_equality_ignores_name_and_namespace_insertion_order() {
        let alpha = Name::new("Alpha");
        let beta = Name::new("Beta");

        let mut left = ItemScopeBuilder::default();
        left.set_binding(&alpha, Namespace::Types, direct_binding(0));
        left.set_binding(&alpha, Namespace::Values, direct_binding(1));
        left.set_binding(&beta, Namespace::Types, direct_binding(2));
        left.set_binding(&beta, Namespace::Macros, direct_binding(3));

        // Reverse both name and namespace insertion without changing the selected bindings.
        let mut right = ItemScopeBuilder::default();
        right.set_binding(&beta, Namespace::Macros, direct_binding(3));
        right.set_binding(&beta, Namespace::Types, direct_binding(2));
        right.set_binding(&alpha, Namespace::Values, direct_binding(1));
        right.set_binding(&alpha, Namespace::Types, direct_binding(0));

        assert_eq!(left.freeze(), right.freeze());
    }

    fn direct_binding(module: usize) -> NameBinding {
        NameBinding::new(
            DefId::Module(owner(module)),
            Visibility::Public,
            NameBindingProvenance::Direct,
        )
    }

    fn owner(module: usize) -> ModuleRef {
        ModuleRef::krate(crate_ref(), ModuleId(module))
    }

    fn crate_ref() -> CrateRef {
        CrateRef {
            package: PackageSlot(0),
            crate_id: CrateId(0),
        }
    }
}
