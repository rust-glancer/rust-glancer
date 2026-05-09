use std::collections::HashMap;

use rg_item_tree::VisibilityLevel;
use rg_text::Name;

use crate::{DefId, ModuleRef};

/// Frozen module scope optimized for retained query data.
///
/// Build-time import resolution uses `ModuleScopeBuilder`; once scopes stabilize, entries are
/// sorted and boxed here so retained modules do not keep hash-table and `Vec` capacity overhead.
#[derive(
    Debug, Clone, PartialEq, Eq, Default, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub struct ModuleScope {
    pub(crate) entries: Box<[ScopeNameEntry]>,
}

impl ModuleScope {
    /// Returns the frozen scope entry for one textual name.
    pub fn entry(&self, name: &str) -> Option<&ScopeEntry> {
        self.entries
            .binary_search_by(|entry| entry.name.as_str().cmp(name))
            .ok()
            .map(|idx| &self.entries[idx].entry)
    }

    /// Iterates over entries in stable textual-name order.
    pub fn entries(&self) -> impl Iterator<Item = (&Name, &ScopeEntry)> {
        self.entries.iter().map(|entry| (&entry.name, &entry.entry))
    }

    pub(crate) fn shrink_to_fit(&mut self) {
        for entry in &mut self.entries {
            entry.name.shrink_to_fit();
            entry.entry.shrink_to_fit();
        }
    }
}

/// One sorted name entry inside a frozen module scope.
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub(crate) struct ScopeNameEntry {
    pub(crate) name: Name,
    pub(crate) entry: ScopeEntry,
}

/// Mutable module scope used while import resolution is finding a fixed point.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct ModuleScopeBuilder {
    names: HashMap<Name, ScopeEntryBuilder>,
}

impl ModuleScopeBuilder {
    pub(super) fn insert_binding(
        &mut self,
        name: &Name,
        namespace: Namespace,
        binding: ScopeBinding,
    ) -> bool {
        let entry = self.names.entry(name.clone()).or_default();
        entry.insert_binding(namespace, binding)
    }

    pub(super) fn copy_visible_bindings(
        &mut self,
        name: &Name,
        entry: ScopeEntryRef<'_>,
        visibility: VisibilityLevel,
        owner: ModuleRef,
    ) {
        for binding in entry.types() {
            self.insert_binding(
                name,
                Namespace::Types,
                ScopeBinding {
                    def: binding.def,
                    visibility: visibility.clone(),
                    owner,
                },
            );
        }

        for binding in entry.values() {
            self.insert_binding(
                name,
                Namespace::Values,
                ScopeBinding {
                    def: binding.def,
                    visibility: visibility.clone(),
                    owner,
                },
            );
        }

        for binding in entry.macros() {
            self.insert_binding(
                name,
                Namespace::Macros,
                ScopeBinding {
                    def: binding.def,
                    visibility: visibility.clone(),
                    owner,
                },
            );
        }
    }

    pub(super) fn entry(&self, name: &str) -> Option<ScopeEntryRef<'_>> {
        self.names.get(name).map(ScopeEntryBuilder::as_ref)
    }

    pub(super) fn entries(&self) -> impl Iterator<Item = (&Name, ScopeEntryRef<'_>)> {
        self.names
            .iter()
            .map(|(name, entry)| (name, entry.as_ref()))
    }

    pub(super) fn freeze(&self) -> ModuleScope {
        let mut entries = self
            .names
            .iter()
            .map(|(name, entry)| ScopeNameEntry {
                name: name.clone(),
                entry: entry.freeze(),
            })
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| left.name.cmp(&right.name));

        ModuleScope {
            entries: entries.into_boxed_slice(),
        }
    }
}

/// Frozen namespace slots for one textual name.
#[derive(
    Debug, Clone, PartialEq, Eq, Default, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub struct ScopeEntry {
    pub(crate) types: Box<[ScopeBinding]>,
    pub(crate) values: Box<[ScopeBinding]>,
    pub(crate) macros: Box<[ScopeBinding]>,
}

impl ScopeEntry {
    pub fn types(&self) -> &[ScopeBinding] {
        &self.types
    }

    pub fn values(&self) -> &[ScopeBinding] {
        &self.values
    }

    pub fn macros(&self) -> &[ScopeBinding] {
        &self.macros
    }

    pub fn is_empty(&self) -> bool {
        self.types.is_empty() && self.values.is_empty() && self.macros.is_empty()
    }

    pub(super) fn as_ref(&self) -> ScopeEntryRef<'_> {
        ScopeEntryRef {
            types: &self.types,
            values: &self.values,
            macros: &self.macros,
        }
    }

    fn shrink_to_fit(&mut self) {
        for binding in self
            .types
            .iter_mut()
            .chain(self.values.iter_mut())
            .chain(self.macros.iter_mut())
        {
            binding.shrink_to_fit();
        }
    }
}

/// Mutable namespace slots for one textual name.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct ScopeEntryBuilder {
    types: Vec<ScopeBinding>,
    values: Vec<ScopeBinding>,
    macros: Vec<ScopeBinding>,
}

impl ScopeEntryBuilder {
    fn insert_binding(&mut self, namespace: Namespace, binding: ScopeBinding) -> bool {
        let bucket = match namespace {
            Namespace::Types => &mut self.types,
            Namespace::Values => &mut self.values,
            Namespace::Macros => &mut self.macros,
        };

        if bucket.contains(&binding) {
            return false;
        }

        bucket.push(binding);
        true
    }

    fn as_ref(&self) -> ScopeEntryRef<'_> {
        ScopeEntryRef {
            types: &self.types,
            values: &self.values,
            macros: &self.macros,
        }
    }

    fn freeze(&self) -> ScopeEntry {
        ScopeEntry {
            types: self.types.clone().into_boxed_slice(),
            values: self.values.clone().into_boxed_slice(),
            macros: self.macros.clone().into_boxed_slice(),
        }
    }
}

/// Borrowed view over either a mutable-build or frozen scope entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ScopeEntryRef<'a> {
    types: &'a [ScopeBinding],
    values: &'a [ScopeBinding],
    macros: &'a [ScopeBinding],
}

impl<'a> ScopeEntryRef<'a> {
    pub(super) fn types(self) -> &'a [ScopeBinding] {
        self.types
    }

    pub(super) fn values(self) -> &'a [ScopeBinding] {
        self.values
    }

    pub(super) fn macros(self) -> &'a [ScopeBinding] {
        self.macros
    }
}

/// One definition together with the visibility of the binding that introduced it.
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ScopeBinding {
    pub def: DefId,
    pub visibility: VisibilityLevel,
    pub owner: ModuleRef,
}

impl ScopeBinding {
    fn shrink_to_fit(&mut self) {
        match &mut self.visibility {
            VisibilityLevel::Restricted(path) | VisibilityLevel::Unknown(path) => {
                path.shrink_to_fit();
            }
            VisibilityLevel::Private
            | VisibilityLevel::Public
            | VisibilityLevel::Crate
            | VisibilityLevel::Super
            | VisibilityLevel::Self_ => {}
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Namespace {
    Types,
    Values,
    Macros,
}

#[cfg(test)]
mod tests {
    use rg_item_tree::VisibilityLevel;
    use rg_parse::TargetId;

    use crate::{DefId, ModuleId, ModuleRef, PackageSlot, TargetRef};

    use super::{ModuleScopeBuilder, Namespace, ScopeBinding};

    #[test]
    fn builder_rejects_duplicate_bindings() {
        let mut scope = ModuleScopeBuilder::default();
        let name = "User".into();
        let binding = binding(0);

        assert!(scope.insert_binding(&name, Namespace::Types, binding.clone()));
        assert!(!scope.insert_binding(&name, Namespace::Types, binding));

        let entry = scope.entry("User").expect("entry should exist");
        assert_eq!(entry.types().len(), 1);
    }

    #[test]
    fn frozen_scope_looks_up_sorted_entries() {
        let mut scope = ModuleScopeBuilder::default();
        scope.insert_binding(&"zeta".into(), Namespace::Types, binding(0));
        scope.insert_binding(&"alpha".into(), Namespace::Values, binding(1));

        let frozen = scope.freeze();
        let names = frozen
            .entries()
            .map(|(name, _)| name.to_string())
            .collect::<Vec<_>>();

        assert_eq!(names, ["alpha", "zeta"]);
        assert_eq!(frozen.entry("alpha").unwrap().values().len(), 1);
        assert!(frozen.entry("missing").is_none());
    }

    #[test]
    fn builder_copies_namespace_buckets_from_borrowed_entries() {
        let mut source = ModuleScopeBuilder::default();
        source.insert_binding(&"Thing".into(), Namespace::Types, binding(0));
        source.insert_binding(&"Thing".into(), Namespace::Values, binding(1));
        let source_entry = source.entry("Thing").expect("source entry should exist");

        let mut target = ModuleScopeBuilder::default();
        target.copy_visible_bindings(
            &"Alias".into(),
            source_entry,
            VisibilityLevel::Public,
            owner(2),
        );

        let frozen = target.freeze();
        let entry = frozen.entry("Alias").expect("copied entry should exist");
        assert_eq!(entry.types().len(), 1);
        assert_eq!(entry.values().len(), 1);
        assert!(entry.macros().is_empty());
    }

    fn binding(module: usize) -> ScopeBinding {
        ScopeBinding {
            def: DefId::Module(owner(module)),
            visibility: VisibilityLevel::Public,
            owner: owner(0),
        }
    }

    fn owner(module: usize) -> ModuleRef {
        ModuleRef {
            target: TargetRef {
                package: PackageSlot(0),
                target: TargetId(0),
            },
            module: ModuleId(module),
        }
    }
}
