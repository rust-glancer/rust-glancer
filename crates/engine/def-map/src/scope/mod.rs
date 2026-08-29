//! Namespace slots and selected bindings stored by DefMap.
//!
//! One spelling can have separate type, value, and macro meanings. While scopes are built, each
//! slot selects direct and named bindings over globs, keeps equal-priority ambiguity explicit, and
//! merges multiple routes to the same definition. Imports written as `use Trait as _` live in a
//! separate unnamed trait lane because they affect method lookup without creating a path binding.
//! Frozen scopes retain both decisions so queries do not have to reconstruct them from imports.

mod namespace_bindings;

use self::namespace_bindings::{
    FrozenScopeBindings, MutableNamespaceBindingArena, ScopeEntryBuilder,
};

use rg_ir_model::{DefId, ImportRef, ModuleRef};
use rg_item_tree::FieldList;
use rg_std::{MemorySize, Shrink, UniqueVec};
use rg_text::Name;
use rustc_hash::FxHashMap;
use wincode::{SchemaRead, SchemaWrite};

/// The three independent meaning slots represented by a DefMap scope.
///
/// For example, a record struct and a function can share one spelling because the struct occupies
/// the type slot and the function occupies the value slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[memsize(leaf)]
#[shrink(leaf)]
pub enum Namespace {
    Types,
    Values,
    Macros,
}

impl Namespace {
    pub const ALL: [Self; 3] = [Self::Types, Self::Values, Self::Macros];

    /// Canonical retained-storage order. This is not binding precedence.
    pub(crate) fn sort_rank(self) -> u8 {
        match self {
            Self::Types => 0,
            Self::Values => 1,
            Self::Macros => 2,
        }
    }
}

/// A compact set of namespace slots used for lookup or occupied by one declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[memsize(leaf)]
#[shrink(leaf)]
pub struct NamespaceSet(u8);

impl NamespaceSet {
    pub const EMPTY: Self = Self(0);
    pub const TYPES: Self = Self(1 << 0);
    pub const VALUES: Self = Self(1 << 1);
    pub const MACROS: Self = Self(1 << 2);
    pub const TYPES_VALUES: Self = Self(Self::TYPES.0 | Self::VALUES.0);
    pub const ALL: Self = Self(Self::TYPES.0 | Self::VALUES.0 | Self::MACROS.0);

    pub fn contains(self, namespace: Namespace) -> bool {
        let mask = match namespace {
            Namespace::Types => Self::TYPES.0,
            Namespace::Values => Self::VALUES.0,
            Namespace::Macros => Self::MACROS.0,
        };
        self.0 & mask != 0
    }

    pub fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    pub fn iter(self) -> impl Iterator<Item = Namespace> {
        Namespace::ALL
            .into_iter()
            .filter(move |namespace| self.contains(*namespace))
    }

    /// Map a record or tuple/unit constructor shape to its DefMap namespace slots.
    pub(crate) fn for_field_list(fields: &FieldList) -> Self {
        if fields.has_value_constructor() {
            Self::TYPES_VALUES
        } else {
            Self::TYPES
        }
    }

    /// Path prefixes must name containers, while the final segment uses the caller's namespaces.
    ///
    /// In `api::make`, `api` is looked up in the type namespace even when `make` is requested only
    /// from the value namespace.
    pub(crate) fn for_segment(path_prefix: bool, terminal: Self) -> Self {
        if path_prefix { Self::TYPES } else { terminal }
    }
}

/// One value for each DefMap namespace.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, SchemaRead, SchemaWrite, MemorySize, Shrink,
)]
pub struct PerNs<T> {
    types: T,
    values: T,
    macros: T,
}

impl<T> PerNs<T> {
    pub fn new(types: T, values: T, macros: T) -> Self {
        Self {
            types,
            values,
            macros,
        }
    }

    pub fn get(&self, namespace: Namespace) -> &T {
        match namespace {
            Namespace::Types => &self.types,
            Namespace::Values => &self.values,
            Namespace::Macros => &self.macros,
        }
    }

    pub fn get_mut(&mut self, namespace: Namespace) -> &mut T {
        match namespace {
            Namespace::Types => &mut self.types,
            Namespace::Values => &mut self.values,
            Namespace::Macros => &mut self.macros,
        }
    }
}

/// Resolved visibility of one scope route.
///
/// Source declarations retain `VisibilityLevel` for rendering. Scope lookup only needs the module
/// from which a name is visible, so it stores that identity directly and never reparses source text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[memsize(leaf)]
#[shrink(leaf)]
pub enum Visibility {
    /// No module visibility ceiling once the route is reachable.
    Public,
    /// Visible from this module and its descendants inside the same DefMap origin.
    Module(ModuleRef),
    /// Source visibility could not be lowered to a valid module region.
    Invisible,
}

/// How one route to a selected definition entered its module scope.
///
/// Provenance preserves the distinction needed for glob precedence and legacy macro visibility.
/// Import provenance also identifies routes that reach the same definition through different use
/// items, so those routes can merge without becoming an ambiguity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[memsize(leaf)]
#[shrink(leaf)]
pub enum ScopeBindingProvenance {
    /// Ordinary source declaration.
    Direct,
    /// Direct `macro_rules!` binding whose named re-exports may reach anywhere in its crate.
    DirectMacroRules,
    /// Direct `#[macro_export] macro_rules!` binding whose named re-exports may be public.
    DirectMacroExport,
    NamedImport(ImportRef),
    GlobImport(ImportRef),
    ExternCrate,
    /// Public crate-root route injected for an exported `macro_rules!` definition.
    MacroExport,
}

impl ScopeBindingProvenance {
    fn priority(self) -> ScopeBindingPriority {
        match self {
            Self::GlobImport(_) => ScopeBindingPriority::Glob,
            Self::Direct
            | Self::DirectMacroRules
            | Self::DirectMacroExport
            | Self::NamedImport(_)
            | Self::ExternCrate
            | Self::MacroExport => ScopeBindingPriority::Explicit,
        }
    }

    pub fn is_direct(self) -> bool {
        matches!(
            self,
            Self::Direct | Self::DirectMacroRules | Self::DirectMacroExport
        )
    }
}

/// Precedence class used when two definitions compete for one name and namespace.
///
/// Direct declarations, named imports, and extern roots are all explicit. They replace glob
/// candidates regardless of which binding happened to be processed first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ScopeBindingPriority {
    Glob,
    Explicit,
}

/// One independently visible route to a selected definition.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct ScopeBindingRoute {
    pub visibility: Visibility,
    pub provenance: ScopeBindingProvenance,
}

/// One selected definition together with every equal-priority route that reaches it.
///
/// Keeping routes separate is important for visibility: two imports of the same definition are not
/// ambiguous, and either route may make the definition visible from a different module.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct ScopeBinding {
    pub def: DefId,
    routes: ScopeBindingRoutes,
}

impl ScopeBinding {
    pub fn new(def: DefId, visibility: Visibility, provenance: ScopeBindingProvenance) -> Self {
        Self {
            def,
            routes: ScopeBindingRoutes::One(ScopeBindingRoute {
                visibility,
                provenance,
            }),
        }
    }

    pub fn routes(&self) -> &[ScopeBindingRoute] {
        self.routes.as_slice()
    }

    /// Direct-only macro bindings are subject to textual source-order filtering.
    pub fn is_direct_only(&self) -> bool {
        self.routes()
            .iter()
            .all(|route| route.provenance.is_direct())
    }

    fn priority(&self) -> ScopeBindingPriority {
        let priority = self
            .routes()
            .first()
            .expect("scope binding should always retain at least one route")
            .provenance
            .priority();
        debug_assert!(
            self.routes()
                .iter()
                .all(|route| route.provenance.priority() == priority),
            "all routes for one selected binding should have equal priority"
        );
        priority
    }

    pub(crate) fn merge_routes(&mut self, other: Self) -> bool {
        debug_assert_eq!(self.def, other.def);
        debug_assert_eq!(self.priority(), other.priority());

        let mut changed = false;
        for route in other.routes.into_vec() {
            changed |= self.routes.push_unique(route);
        }
        changed
    }

    /// Keep a binding usable inside one crate while preventing its public routes from crossing the
    /// crate boundary.
    fn restrict_public_routes_to(&mut self, visible_from: ModuleRef) {
        for route in self.routes.as_mut_slice() {
            if route.visibility == Visibility::Public {
                route.visibility = Visibility::Module(visible_from);
            }
        }
    }
}

/// Compact storage for the independently visible routes retained by one selected definition.
///
/// Most bindings have one route. Multiple routes are needed when several imports reach the same
/// definition with different visibility regions; they still form one resolved binding rather than
/// an ambiguity.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
enum ScopeBindingRoutes {
    One(ScopeBindingRoute),
    Many(Box<[ScopeBindingRoute]>),
}

impl ScopeBindingRoutes {
    fn as_slice(&self) -> &[ScopeBindingRoute] {
        match self {
            Self::One(route) => std::slice::from_ref(route),
            Self::Many(routes) => routes,
        }
    }

    fn as_mut_slice(&mut self) -> &mut [ScopeBindingRoute] {
        match self {
            Self::One(route) => std::slice::from_mut(route),
            Self::Many(routes) => routes,
        }
    }

    fn into_vec(self) -> Vec<ScopeBindingRoute> {
        match self {
            Self::One(route) => vec![route],
            Self::Many(routes) => routes.into_vec(),
        }
    }

    fn push_unique(&mut self, route: ScopeBindingRoute) -> bool {
        if self.as_slice().contains(&route) {
            return false;
        }

        // Routes from the same source differ only when visibility was intersected through several
        // source routes. A public route subsumes every module-limited route from that same source.
        if self.as_slice().iter().any(|existing| {
            existing.provenance == route.provenance && existing.visibility == Visibility::Public
        }) {
            return false;
        }

        let placeholder = Self::Many(Box::default());
        let mut routes = std::mem::replace(self, placeholder).into_vec();
        if route.visibility == Visibility::Public {
            routes.retain(|existing| existing.provenance != route.provenance);
        }
        routes.push(route);
        *self = match routes.as_slice() {
            [route] => Self::One(route.clone()),
            _ => Self::Many(routes.into_boxed_slice()),
        };
        true
    }
}

/// Frozen selection state for one namespace slot.
///
/// Selection has already applied precedence here. A named import can replace a glob, two routes to
/// the same definition remain `Resolved`, and equal-priority routes to different definitions become
/// `Ambiguous`.
#[derive(Debug, Clone, PartialEq, Eq, Default, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub enum ScopeResolution {
    #[default]
    Empty,
    Resolved(ScopeBinding),
    Ambiguous(Box<[ScopeBinding]>),
}

impl ScopeResolution {
    fn as_ref(&self) -> ScopeResolutionRef<'_> {
        match self {
            Self::Empty => ScopeResolutionRef::Empty,
            Self::Resolved(binding) => ScopeResolutionRef::Resolved(binding),
            Self::Ambiguous(bindings) => ScopeResolutionRef::Ambiguous(bindings),
        }
    }
}

/// Borrowed selection state for one namespace slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeResolutionRef<'a> {
    Empty,
    Resolved(&'a ScopeBinding),
    Ambiguous(&'a [ScopeBinding]),
}

impl<'a> ScopeResolutionRef<'a> {
    pub fn bindings(self) -> &'a [ScopeBinding] {
        match self {
            Self::Empty => &[],
            Self::Resolved(binding) => std::slice::from_ref(binding),
            Self::Ambiguous(bindings) => bindings,
        }
    }
}

/// Frozen module scope optimized for retained query data.
///
/// Build-time import resolution uses `ModuleScopeBuilder`; once scopes stabilize, entries are
/// sorted and boxed here so retained modules do not keep hash-table and `Vec` capacity overhead.
#[derive(Debug, Clone, PartialEq, Eq, Default, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct ModuleScope {
    pub(crate) entries: Box<[ScopeNameEntry]>,
    /// Traits imported with an underscore are in method scope without occupying a type name.
    ///
    /// For example, `use crate::Render as _;` makes `value.render()` legal while leaving
    /// `Render` unavailable as a path. Keeping that fact outside `entries` preserves both sides
    /// of the language rule instead of inventing a synthetic name for the import.
    unnamed_trait_bindings: Box<[ScopeBinding]>,
}

impl ModuleScope {
    pub fn entry(&self, name: &str) -> Option<&ScopeEntry> {
        self.entries
            .binary_search_by(|entry| entry.name.as_str().cmp(name))
            .ok()
            .map(|idx| &self.entries[idx].entry)
    }

    pub fn entries(&self) -> impl Iterator<Item = (&Name, &ScopeEntry)> {
        self.entries.iter().map(|entry| (&entry.name, &entry.entry))
    }

    pub(crate) fn unnamed_trait_bindings(&self) -> &[ScopeBinding] {
        &self.unnamed_trait_bindings
    }
}

/// Name and namespace selections stored together in the sorted frozen module scope.
///
/// `ModuleScope::entry` binary-searches these values by `name`; the separate `ScopeEntry` owns the
/// sparse type/value/macro results for that spelling.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub(crate) struct ScopeNameEntry {
    pub(crate) name: Name,
    pub(crate) entry: ScopeEntry,
}

/// Mutable module scope used while collecting declarations and applying imports.
///
/// Each fixed-point pass needs fast lookup and insertion, so names live in a hash map. One
/// `ScopeEntryBuilder` keeps the first occupied namespace inline, while the scope-owned arena holds
/// second and third slots. Arena ids are only storage handles: equality follows the selected
/// namespace contents because `ModuleScopeBuilder` equality decides when import resolution has
/// converged.
#[derive(Debug, Clone, Default)]
pub struct ModuleScopeBuilder {
    names: FxHashMap<Name, ScopeEntryBuilder>,
    additional_namespace_bindings: MutableNamespaceBindingArena,
    unnamed_trait_bindings: UniqueVec<ScopeBinding>,
}

// Do not derive this equality. Arena ids depend on name and namespace insertion order, but the
// import fixed point must stop when two scopes expose the same semantic bindings.
impl PartialEq for ModuleScopeBuilder {
    fn eq(&self, other: &Self) -> bool {
        self.names.len() == other.names.len()
            && self.unnamed_trait_bindings == other.unnamed_trait_bindings
            && self.names.iter().all(|(name, entry)| {
                other.names.get(name).is_some_and(|other_entry| {
                    entry.has_same_bindings(
                        &self.additional_namespace_bindings,
                        other_entry,
                        &other.additional_namespace_bindings,
                    )
                })
            })
    }
}

impl Eq for ModuleScopeBuilder {}

impl ModuleScopeBuilder {
    /// Insert one route and update the selected result for only this namespace.
    ///
    /// Explicit bindings replace globs regardless of insertion order. Equal-priority bindings to
    /// different definitions stay ambiguous, while routes to the same definition are merged.
    pub fn insert_binding(
        &mut self,
        name: &Name,
        namespace: Namespace,
        binding: ScopeBinding,
    ) -> bool {
        self.names.entry(name.clone()).or_default().insert_binding(
            namespace,
            binding,
            &mut self.additional_namespace_bindings,
        )
    }

    pub fn entry(&self, name: &str) -> Option<ScopeEntryRef<'_>> {
        self.names
            .get(name)
            .map(|entry| entry.as_ref(&self.additional_namespace_bindings))
    }

    pub fn entries(&self) -> impl Iterator<Item = (&Name, ScopeEntryRef<'_>)> {
        self.names
            .iter()
            .map(|(name, entry)| (name, entry.as_ref(&self.additional_namespace_bindings)))
    }

    /// Retain a resolved `use Trait as _` route without adding a path-visible name.
    pub fn insert_unnamed_trait_binding(&mut self, binding: ScopeBinding) -> bool {
        self.unnamed_trait_bindings.push(binding)
    }

    /// Restrict public routes except for bindings explicitly allowed by the caller.
    ///
    /// Proc-macro targets use this after each import-resolution step: their implementation items
    /// remain visible throughout the defining crate, while only direct proc-macro exports can be
    /// named by downstream crates.
    pub(crate) fn censor_public_bindings(
        &mut self,
        visible_from: ModuleRef,
        mut should_remain_public: impl FnMut(Namespace, &ScopeBinding) -> bool,
    ) {
        for entry in self.names.values_mut() {
            entry.for_each_binding_mut(
                &mut self.additional_namespace_bindings,
                |namespace, binding| {
                    if !should_remain_public(namespace, binding) {
                        binding.restrict_public_routes_to(visible_from);
                    }
                },
            );
        }
        let unnamed_trait_bindings = std::mem::take(&mut self.unnamed_trait_bindings);
        for mut binding in unnamed_trait_bindings {
            if !should_remain_public(Namespace::Types, &binding) {
                binding.restrict_public_routes_to(visible_from);
            }
            self.unnamed_trait_bindings.push(binding);
        }
    }

    /// Turn a settled build scope into the compact representation retained by queries.
    ///
    /// Every name drains the extra namespace nodes it owns. Names are then sorted so lookup,
    /// equality, and serialization are independent of hash-map and import insertion order.
    pub fn freeze(self) -> ModuleScope {
        let mut additional_namespace_bindings = self.additional_namespace_bindings;
        let mut entries = self
            .names
            .into_iter()
            .map(|(name, entry)| ScopeNameEntry {
                name,
                entry: entry.freeze(&mut additional_namespace_bindings),
            })
            .collect::<Vec<_>>();
        debug_assert!(
            additional_namespace_bindings.is_drained(),
            "every additional namespace binding should belong to one frozen scope entry"
        );
        entries.sort_by(|left, right| left.name.cmp(&right.name));

        ModuleScope {
            entries: entries.into_boxed_slice(),
            unnamed_trait_bindings: self.unnamed_trait_bindings.into_vec().into_boxed_slice(),
        }
    }
}

/// Frozen namespace selections for one textual name.
///
/// Empty namespaces occupy no retained slot. `resolution` reconstructs an empty result when the
/// requested type, value, or macro slot is absent, so query code does not need to know about the
/// sparse representation.
#[derive(Debug, Clone, PartialEq, Eq, Default, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct ScopeEntry {
    bindings: FrozenScopeBindings,
}

impl ScopeEntry {
    pub fn resolution(&self, namespace: Namespace) -> ScopeResolutionRef<'_> {
        self.bindings.resolution(namespace)
    }

    pub fn bindings(&self, namespace: Namespace) -> &[ScopeBinding] {
        self.resolution(namespace).bindings()
    }

    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }

    pub fn as_ref(&self) -> ScopeEntryRef<'_> {
        ScopeEntryRef {
            bindings: PerNs::new(
                self.resolution(Namespace::Types),
                self.resolution(Namespace::Values),
                self.resolution(Namespace::Macros),
            ),
        }
    }
}

/// Borrowed view over either a mutable-build or frozen scope entry.
///
/// Path resolution reads this common shape while finalization is still changing scopes and after
/// those scopes have been frozen. Keeping the storage distinction behind this view prevents the
/// resolver from having separate mutable and retained lookup paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScopeEntryRef<'a> {
    bindings: PerNs<ScopeResolutionRef<'a>>,
}

impl<'a> ScopeEntryRef<'a> {
    pub fn resolution(self, namespace: Namespace) -> ScopeResolutionRef<'a> {
        *self.bindings.get(namespace)
    }

    pub fn bindings(self, namespace: Namespace) -> &'a [ScopeBinding] {
        self.resolution(namespace).bindings()
    }
}

#[cfg(test)]
mod tests {
    use rg_ir_model::CrateId;
    use rg_ir_model::{CrateRef, DefId, DefMapRef, ImportId, ImportRef, ModuleId, ModuleRef};
    use rg_text::Name;
    use rg_workspace::PackageSlot;

    use super::{
        ModuleScopeBuilder, Namespace, ScopeBinding, ScopeBindingProvenance, ScopeResolutionRef,
        Visibility,
    };

    #[test]
    fn builder_rejects_duplicate_routes_to_one_definition() {
        let mut scope = ModuleScopeBuilder::default();
        let name = Name::new("User");
        let binding = direct_binding(0);

        assert!(scope.insert_binding(&name, Namespace::Types, binding.clone()));
        assert!(!scope.insert_binding(&name, Namespace::Types, binding));

        let entry = scope.entry("User").expect("entry should exist");
        assert!(matches!(
            entry.resolution(Namespace::Types),
            ScopeResolutionRef::Resolved(_)
        ));
    }

    #[test]
    fn explicit_binding_replaces_glob_regardless_of_insertion_order() {
        for glob_first in [false, true] {
            let mut scope = ModuleScopeBuilder::default();
            let name = Name::new("User");
            let explicit = direct_binding(0);
            let glob = glob_binding(1);

            if glob_first {
                scope.insert_binding(&name, Namespace::Types, glob);
                scope.insert_binding(&name, Namespace::Types, explicit.clone());
            } else {
                scope.insert_binding(&name, Namespace::Types, explicit.clone());
                scope.insert_binding(&name, Namespace::Types, glob);
            }

            let entry = scope.entry("User").expect("entry should exist");
            assert_eq!(entry.bindings(Namespace::Types), [explicit]);
        }
    }

    #[test]
    fn distinct_equal_priority_bindings_are_explicitly_ambiguous() {
        let mut scope = ModuleScopeBuilder::default();
        let name = Name::new("User");
        scope.insert_binding(&name, Namespace::Types, direct_binding(0));
        scope.insert_binding(&name, Namespace::Types, direct_binding(1));

        let entry = scope.entry("User").expect("entry should exist");
        assert!(matches!(
            entry.resolution(Namespace::Types),
            ScopeResolutionRef::Ambiguous(bindings) if bindings.len() == 2
        ));
    }

    #[test]
    fn equal_priority_routes_to_one_definition_are_merged() {
        let mut scope = ModuleScopeBuilder::default();
        let name = Name::new("User");
        let def = DefId::Module(owner(0));
        scope.insert_binding(
            &name,
            Namespace::Types,
            ScopeBinding::new(def, Visibility::Public, ScopeBindingProvenance::Direct),
        );
        scope.insert_binding(
            &name,
            Namespace::Types,
            ScopeBinding::new(def, Visibility::Public, ScopeBindingProvenance::MacroExport),
        );

        let entry = scope.entry("User").expect("entry should exist");
        let [binding] = entry.bindings(Namespace::Types) else {
            panic!("same definition should remain one selected binding");
        };
        assert_eq!(binding.routes().len(), 2);
    }

    #[test]
    fn frozen_scope_looks_up_sorted_entries() {
        let mut scope = ModuleScopeBuilder::default();
        scope.insert_binding(&Name::new("zeta"), Namespace::Types, direct_binding(0));
        scope.insert_binding(&Name::new("alpha"), Namespace::Values, direct_binding(1));

        let frozen = scope.freeze();
        let names = frozen
            .entries()
            .map(|(name, _)| name.to_string())
            .collect::<Vec<_>>();

        assert_eq!(names, ["alpha", "zeta"]);
        assert_eq!(
            frozen
                .entry("alpha")
                .expect("entry should exist")
                .bindings(Namespace::Values)
                .len(),
            1
        );
        assert!(frozen.entry("missing").is_none());
    }

    #[test]
    fn frozen_scope_preserves_occupied_namespace_combinations() {
        let cases: [(&str, &[Namespace]); 4] = [
            ("type only", &[Namespace::Types]),
            ("value only", &[Namespace::Values]),
            ("type and value", &[Namespace::Types, Namespace::Values]),
            ("all", &Namespace::ALL),
        ];

        for (case, occupied) in cases {
            let mut scope = ModuleScopeBuilder::default();
            let name = Name::new("Item");
            for (index, namespace) in occupied.iter().copied().enumerate() {
                scope.insert_binding(&name, namespace, direct_binding(index));
            }

            let frozen = scope.freeze();
            let entry = frozen.entry("Item").expect("inserted name should freeze");
            for namespace in Namespace::ALL {
                assert_eq!(
                    !entry.bindings(namespace).is_empty(),
                    occupied.contains(&namespace),
                    "{case}: unexpected {namespace:?} occupancy",
                );
            }
        }
    }

    #[test]
    fn scope_equality_ignores_sparse_namespace_allocation_order() {
        let alpha = Name::new("Alpha");
        let beta = Name::new("Beta");

        let mut left = ModuleScopeBuilder::default();
        left.insert_binding(&alpha, Namespace::Types, direct_binding(0));
        left.insert_binding(&alpha, Namespace::Values, direct_binding(1));
        left.insert_binding(&beta, Namespace::Types, direct_binding(2));
        left.insert_binding(&beta, Namespace::Macros, direct_binding(3));

        // Reverse both name and namespace insertion. The sparse arena now assigns different IDs
        // and each entry chooses a different inline slot, but its visible scope is unchanged.
        let mut right = ModuleScopeBuilder::default();
        right.insert_binding(&beta, Namespace::Macros, direct_binding(3));
        right.insert_binding(&beta, Namespace::Types, direct_binding(2));
        right.insert_binding(&alpha, Namespace::Values, direct_binding(1));
        right.insert_binding(&alpha, Namespace::Types, direct_binding(0));

        assert_eq!(left, right);
        assert_eq!(left.freeze(), right.freeze());
    }

    fn direct_binding(module: usize) -> ScopeBinding {
        ScopeBinding::new(
            DefId::Module(owner(module)),
            Visibility::Public,
            ScopeBindingProvenance::Direct,
        )
    }

    fn glob_binding(module: usize) -> ScopeBinding {
        ScopeBinding::new(
            DefId::Module(owner(module)),
            Visibility::Public,
            ScopeBindingProvenance::GlobImport(ImportRef {
                origin: DefMapRef::Crate(crate_ref()),
                import: ImportId(0),
            }),
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
