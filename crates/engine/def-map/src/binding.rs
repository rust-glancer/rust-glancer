//! Selected definitions for item names, with namespace, visibility, and precedence rules.
//!
//! A spelling can have a type, value, and macro meaning. Each occupied namespace selects one
//! binding: its target and the facts needed for lookup, imports, and presentation. Item scopes
//! store these answers; this module decides how competing answers are selected or combined.

use std::cmp::Ordering;

use rg_ir_model::{DefId, ImportRef, ModuleRef};
use rg_item_tree::FieldList;
use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

use crate::ImportData;

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
}

impl FromIterator<Namespace> for NamespaceSet {
    fn from_iter<T: IntoIterator<Item = Namespace>>(namespaces: T) -> Self {
        let mut set = Self::EMPTY;
        for namespace in namespaces {
            set.0 |= match namespace {
                Namespace::Types => Self::TYPES.0,
                Namespace::Values => Self::VALUES.0,
                Namespace::Macros => Self::MACROS.0,
            };
        }
        set
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

/// Resolved visibility of a name binding.
///
/// Source declarations retain `VisibilityLevel` for rendering. Scope lookup only needs the module
/// from which a name is visible, so it stores that identity directly and never reparses source text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[memsize(leaf)]
#[shrink(leaf)]
pub enum Visibility {
    /// No module visibility ceiling once the binding is reachable.
    Public,
    /// Visible from this module and its descendants inside the same DefMap origin.
    Module(ModuleRef),
    /// Source visibility could not be lowered to a valid module region.
    Invisible,
}

/// How the selected source entered its module scope.
///
/// Construction lowers this source into precedence, declaration flags, and an optional import for
/// presentation. Repeated imports of the same definition still select one source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameBindingProvenance {
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

impl NameBindingProvenance {
    fn priority(self) -> BindingPriority {
        match self {
            Self::GlobImport(_) => BindingPriority::Glob,
            Self::Direct
            | Self::DirectMacroRules
            | Self::DirectMacroExport
            | Self::NamedImport(_)
            | Self::ExternCrate
            | Self::MacroExport => BindingPriority::Explicit,
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
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, SchemaRead, SchemaWrite, MemorySize, Shrink,
)]
#[memsize(leaf)]
#[shrink(leaf)]
enum BindingPriority {
    Glob,
    Explicit,
}

/// One selected definition for a name and namespace.
///
/// Explicit bindings beat globs. Equal-priority bindings to the same definition merge visibility;
/// conflicting definitions keep the first result so incomplete code still has a useful target.
/// The selected source supplies presentation attributes. Small summary flags preserve declaration
/// and re-export rules even when another import becomes the selected source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct NameBinding {
    pub def: DefId,
    pub visibility: Visibility,
    attribute_import: Option<ImportRef>,
    priority: BindingPriority,
    // A same-name macro import removes textual-order filtering without erasing the fact that a
    // direct declaration exists. Proc-macro export censoring still needs the latter.
    direct_only: bool,
    has_direct_declaration: bool,
    // These declaration ceilings survive selecting an import for its presentation attributes.
    direct_macro_rules: bool,
    exported_macro: bool,
    has_reexport: bool,
    preferred_import: bool,
}

impl NameBinding {
    pub fn new(def: DefId, visibility: Visibility, provenance: NameBindingProvenance) -> Self {
        Self {
            def,
            visibility,
            attribute_import: match provenance {
                NameBindingProvenance::NamedImport(import)
                | NameBindingProvenance::GlobImport(import) => Some(import),
                _ => None,
            },
            priority: provenance.priority(),
            direct_only: provenance.is_direct(),
            has_direct_declaration: provenance.is_direct(),
            direct_macro_rules: matches!(
                provenance,
                NameBindingProvenance::DirectMacroRules | NameBindingProvenance::DirectMacroExport
            ),
            exported_macro: matches!(
                provenance,
                NameBindingProvenance::DirectMacroExport | NameBindingProvenance::MacroExport
            ),
            has_reexport: false,
            preferred_import: false,
        }
    }

    pub(crate) fn from_import(
        def: DefId,
        visibility: Visibility,
        provenance: NameBindingProvenance,
        import: &ImportData,
    ) -> Self {
        Self {
            has_reexport: import.is_reexport,
            preferred_import: !import.user_facing_attrs.is_doc_hidden()
                && !import.user_facing_attrs.is_unstable(),
            ..Self::new(def, visibility, provenance)
        }
    }

    /// Direct-only macro bindings are subject to textual source-order filtering.
    pub fn is_direct_only(self) -> bool {
        self.direct_only
    }

    pub(crate) fn is_explicit(self) -> bool {
        self.priority == BindingPriority::Explicit
    }

    pub fn has_reexport(self) -> bool {
        self.has_reexport
    }

    pub(crate) fn has_direct_declaration(self) -> bool {
        self.has_direct_declaration
    }

    pub(crate) fn has_direct_macro_rules(self) -> bool {
        self.direct_macro_rules
    }

    pub(crate) fn is_exported_macro(self) -> bool {
        self.exported_macro
    }

    pub fn attribute_import(self) -> Option<ImportRef> {
        self.attribute_import
    }

    /// Select one binding while letting the storage owner compare module visibility regions.
    ///
    /// Visibility lookup can load an offloaded module, so the comparator may fail. Different
    /// definitions never combine visibility: the earlier definition wins an equal-priority tie.
    pub(crate) fn select<E>(
        mut self,
        incoming: Self,
        broadest_visibility: impl FnOnce(Visibility, Visibility) -> Result<Visibility, E>,
    ) -> Result<Self, E> {
        match incoming.priority.cmp(&self.priority) {
            Ordering::Greater => return Ok(incoming),
            Ordering::Less => return Ok(self),
            Ordering::Equal if self.def != incoming.def => return Ok(self),
            Ordering::Equal => {}
        }

        if self.visibility != incoming.visibility {
            self.visibility = broadest_visibility(self.visibility, incoming.visibility)?;
        }
        self.direct_only &= incoming.direct_only;
        self.has_direct_declaration |= incoming.has_direct_declaration;
        self.direct_macro_rules |= incoming.direct_macro_rules;
        self.exported_macro |= incoming.exported_macro;
        self.has_reexport |= incoming.has_reexport;

        // In `macro_rules! m { ... } pub(crate) use m;`, the import supplies attributes while the
        // declaration flags above preserve textual-order and re-export rules. Between imports,
        // prefer an ordinary API source; stable ties keep the first source.
        // TODO: Mixed public/private routes may need different attribute sources for callers in
        // different visibility regions; this keeps one source for every caller.
        if incoming.attribute_import().is_some()
            && (self.attribute_import().is_none()
                || incoming.preferred_import && !self.preferred_import)
        {
            self.attribute_import = incoming.attribute_import;
            self.preferred_import = incoming.preferred_import;
        }
        Ok(self)
    }

    /// Keep a binding usable inside its crate while restricting external proc-macro exports.
    pub(crate) fn restrict_public_visibility_to(&mut self, visible_from: ModuleRef) {
        if self.visibility == Visibility::Public {
            self.visibility = Visibility::Module(visible_from);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;

    use rg_ir_model::{
        CrateId, CrateRef, DefId, DefMapRef, ImportId, ImportRef, ModuleId, ModuleRef, PackageSlot,
    };

    use super::{NameBinding, NameBindingProvenance, Visibility};

    #[test]
    fn explicit_binding_replaces_glob_regardless_of_insertion_order() {
        let explicit = direct_binding(0);
        let glob = glob_binding(1);
        for (first, second) in [(explicit, glob), (glob, explicit)] {
            let selected = first
                .select(second, |left, _| Ok::<_, Infallible>(left))
                .expect("test visibility comparison is infallible");
            assert_eq!(selected, explicit);
        }
    }

    #[test]
    fn equal_priority_conflicting_definitions_keep_the_first() {
        let first = direct_binding(0);
        let selected = first
            .select(direct_binding(1), |left, _| Ok::<_, Infallible>(left))
            .expect("test visibility comparison is infallible");
        assert_eq!(selected, first);
    }

    #[test]
    fn repeated_definition_uses_the_broadest_visibility() {
        let public = glob_binding(0);
        let mut private = public;
        private.visibility = Visibility::Module(owner(0));
        for (first, second) in [(private, public), (public, private)] {
            let selected = first
                .select(second, |_, _| Ok::<_, Infallible>(Visibility::Public))
                .expect("test visibility comparison is infallible");
            assert_eq!(selected.visibility, Visibility::Public);
            assert_eq!(selected.def, public.def);
        }
    }

    fn direct_binding(module: usize) -> NameBinding {
        NameBinding::new(
            DefId::Module(owner(module)),
            Visibility::Public,
            NameBindingProvenance::Direct,
        )
    }

    fn glob_binding(module: usize) -> NameBinding {
        NameBinding::new(
            DefId::Module(owner(module)),
            Visibility::Public,
            NameBindingProvenance::GlobImport(ImportRef {
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
