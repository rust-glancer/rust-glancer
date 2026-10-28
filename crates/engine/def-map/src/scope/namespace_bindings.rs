//! Sparse storage for the namespace slots owned by one scope name.
//!
//! A [`ScopeEntryBuilder`] belongs to one spelling in `ModuleScopeBuilder`, such as `Widget`. That
//! spelling can have a type, value, and macro meaning, but most names occupy only one of those
//! slots. The first occupied slot therefore stays inside the entry. A second or third slot is kept
//! in one arena shared by the complete module scope, which avoids a small allocation for every
//! multi-namespace name.
//!
//! Freezing reverses that split representation. Each entry takes its additional arena bindings and
//! writes a self-contained [`FrozenScopeBindings`] value in type/value/macro order. Frozen lookup
//! can then borrow a normal `ScopeEntryRef` without exposing either the sparse layout or its arena
//! ids. The surrounding `scope` module owns binding precedence and visibility semantics; this
//! module only owns the mutable and retained storage shapes used to preserve those decisions.

use std::num::NonZeroU32;

use rg_ir_model::ImportId;
use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

use super::{Namespace, PerNs, ScopeBinding, ScopeEntry, ScopeEntryRef};

/// Retained namespace slots for one textual name after scope resolution has settled.
///
/// Most names occupy exactly one namespace. This includes the very large number of bindings
/// copied through glob imports: a Vulkan bindings crate, for example, can expose thousands of
/// types into hundreds of modules. Keeping three inline [`ScopeBinding`] values would make
/// every one of those entries pay for two empty slots. Store the common first slot inline and
/// allocate only the additional occupied slots instead. A missing namespace is represented by the
/// absence of a [`FrozenNamespaceBinding`], and lookup returns `None` for it.
#[derive(Debug, Clone, PartialEq, Eq, Default, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub(super) enum FrozenScopeBindings {
    #[default]
    Empty,
    One(FrozenNamespaceBinding),
    Multiple {
        first: FrozenNamespaceBinding,
        rest: Box<[FrozenNamespaceBinding]>,
    },
}

impl FrozenScopeBindings {
    pub(super) fn binding(&self, namespace: Namespace) -> Option<&ScopeBinding> {
        match self {
            Self::Empty => None,
            Self::One(binding) => (binding.namespace == namespace).then_some(&binding.binding),
            Self::Multiple { first, rest } => std::iter::once(first)
                .chain(rest.iter())
                .find_map(|binding| (binding.namespace == namespace).then_some(&binding.binding)),
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        matches!(self, Self::Empty)
    }
}

/// One occupied namespace and its settled resolution inside the sparse frozen representation.
///
/// The namespace tag is needed because sparse slots are stored consecutively instead of in three
/// fixed type/value/macro fields.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub(super) struct FrozenNamespaceBinding {
    namespace: Namespace,
    binding: ScopeBinding,
}

/// Mutable namespace state for one name in `ModuleScopeBuilder::names`.
///
/// Glob imports copy large numbers of names that occupy only one namespace. Three inline
/// bindings would make every such name pay for two empty slots throughout construction.
///
/// The first binding therefore stays inline. There can only be two more namespaces: their IDs fit
/// together in eight bytes and point into the scope-owned arena. The arena stores only occupied
/// slots, without a separate allocation for each name.
///
/// ```text
/// Widget => first: Types(binding)
///           additional: [Values slot id, Macros slot id]
/// ```
///
/// The first slot follows insertion order. Namespace tags determine lookup and frozen ordering.
#[derive(Debug, Clone, Default)]
pub(super) struct ScopeEntryBuilder {
    first: Option<MutableNamespaceBinding>,
    additional: [Option<MutableNamespaceBindingId>; 2],
}

impl ScopeEntryBuilder {
    /// Store a selected binding, allocating an extra slot only for a newly occupied namespace.
    ///
    /// `import` identifies the winning route, not the source of presentation attributes. A changed
    /// binding replaces that route; an unchanged write with no import ID preserves it. Declaration
    /// collection needs the latter when a new declaration loses to an existing explicit import.
    pub(super) fn set_binding(
        &mut self,
        namespace: Namespace,
        binding: ScopeBinding,
        import: Option<ImportId>,
        arena: &mut MutableNamespaceBindingArena,
    ) {
        let Some(first) = &mut self.first else {
            self.first = Some(MutableNamespaceBinding {
                namespace,
                binding,
                import,
            });
            return;
        };
        if first.namespace == namespace {
            // A direct declaration can lose to an existing explicit import. The collector then
            // writes the unchanged winner back; keep its producer so later import updates work.
            if first.binding != binding || import.is_some() {
                first.import = import;
            }
            first.binding = binding;
            return;
        }
        for id in self.additional.iter().flatten() {
            let slot = arena.get_mut(*id);
            if slot.namespace == namespace {
                if slot.binding != binding || import.is_some() {
                    slot.import = import;
                }
                slot.binding = binding;
                return;
            }
        }
        let extra = self
            .additional
            .iter_mut()
            .find(|slot| slot.is_none())
            .expect("a name has at most three namespaces");
        *extra = Some(arena.push(MutableNamespaceBinding {
            namespace,
            binding,
            import,
        }));
    }

    pub(super) fn binding_import(
        &self,
        namespace: Namespace,
        arena: &MutableNamespaceBindingArena,
    ) -> Option<ImportId> {
        self.first
            .iter()
            .chain(self.additional.iter().flatten().map(|id| arena.get(*id)))
            .find(|slot| slot.namespace == namespace)
            .and_then(|slot| slot.import)
    }

    fn binding<'a>(
        &'a self,
        namespace: Namespace,
        arena: &'a MutableNamespaceBindingArena,
    ) -> Option<&'a ScopeBinding> {
        self.first
            .iter()
            .chain(self.additional.iter().flatten().map(|id| arena.get(*id)))
            .find(|binding| binding.namespace == namespace)
            .map(|binding| &binding.binding)
    }

    /// Reconstruct the dense borrowed view expected by scope consumers without allocating.
    ///
    /// The view hides which namespace is inline and which ones live in the shared arena.
    pub(super) fn as_ref<'a>(
        &'a self,
        arena: &'a MutableNamespaceBindingArena,
    ) -> ScopeEntryRef<'a> {
        ScopeEntryRef {
            bindings: PerNs::new(
                self.binding(Namespace::Types, arena),
                self.binding(Namespace::Values, arena),
                self.binding(Namespace::Macros, arena),
            ),
        }
    }

    /// Visit the selected binding in each occupied namespace.
    pub(super) fn for_each_binding_mut(
        &mut self,
        arena: &mut MutableNamespaceBindingArena,
        mut apply: impl FnMut(Namespace, &mut ScopeBinding),
    ) {
        if let Some(first) = &mut self.first {
            apply(first.namespace, &mut first.binding);
        }
        for id in self.additional.iter().flatten() {
            let slot = arena.get_mut(*id);
            apply(slot.namespace, &mut slot.binding);
        }
    }

    /// Take this name's extra slots and retain occupied namespaces in type/value/macro order.
    ///
    /// `ModuleScopeBuilder` checks that every arena slot was claimed after all names are frozen.
    pub(super) fn freeze(self, arena: &mut MutableNamespaceBindingArena) -> ScopeEntry {
        let mut sorted = [None, None, None];
        for binding in self.first.into_iter().chain(
            self.additional
                .into_iter()
                .flatten()
                .map(|id| arena.take(id)),
        ) {
            let rank = usize::from(binding.namespace.sort_rank());
            let previous = sorted[rank].replace(FrozenNamespaceBinding {
                namespace: binding.namespace,
                binding: binding.binding,
            });
            debug_assert!(
                previous.is_none(),
                "each namespace should have one occupied slot"
            );
        }
        let mut bindings = sorted.into_iter().flatten();
        let Some(first) = bindings.next() else {
            return ScopeEntry {
                bindings: FrozenScopeBindings::Empty,
            };
        };
        let bindings = match (bindings.next(), bindings.next()) {
            (None, None) => FrozenScopeBindings::One(first),
            (Some(second), None) => FrozenScopeBindings::Multiple {
                first,
                rest: Box::new([second]),
            },
            (Some(second), Some(third)) => FrozenScopeBindings::Multiple {
                first,
                rest: Box::new([second, third]),
            },
            (None, Some(_)) => unreachable!("namespace bindings should not contain gaps"),
        };
        ScopeEntry { bindings }
    }
}

/// One occupied mutable namespace slot.
///
/// Empty namespaces have no slot, so this value only needs the namespace tag and its resolution.
#[derive(Debug, Clone)]
struct MutableNamespaceBinding {
    namespace: Namespace,
    binding: ScopeBinding,
    // Winning import route: only this import may replace the selected target at equal priority.
    // Another route to the same definition may supply presentation attributes without winning.
    // All imports here belong to this scope's DefMap, so only its local ID is needed.
    // This field is discarded when the slot is frozen.
    import: Option<ImportId>,
}

/// One-based index into a module scope's additional namespace slots.
///
/// Two optional 32-bit IDs occupy the same eight bytes as one pointer-sized link on a 64-bit host.
/// The index bounds one module's extra slots, not the number of bindings in the whole project.
#[derive(Debug, Clone, Copy)]
struct MutableNamespaceBindingId(NonZeroU32);

impl MutableNamespaceBindingId {
    fn from_index(index: usize) -> Self {
        let index = u32::try_from(index).expect("namespace binding index should fit u32");
        let one_based = index
            .checked_add(1)
            .expect("namespace binding id should fit u32");
        Self(NonZeroU32::new(one_based).expect("one-based namespace binding index is nonzero"))
    }

    fn index(self) -> usize {
        (self.0.get() - 1) as usize
    }
}

/// Scope-owned storage for namespace slots that do not fit inline in their name entry.
///
/// Every slot belongs to exactly one `ScopeEntryBuilder`. Freezing names can happen in hash-map
/// iteration order, so slots are wrapped in `Option`: an entry takes its bindings without moving the
/// ids still held by other entries. The arena has no reuse path because scope construction only
/// appends slots and the complete arena is discarded after freezing.
#[derive(Debug, Clone, Default)]
pub(super) struct MutableNamespaceBindingArena {
    bindings: Vec<Option<MutableNamespaceBinding>>,
}

impl MutableNamespaceBindingArena {
    pub(super) fn is_drained(&self) -> bool {
        self.bindings.iter().all(Option::is_none)
    }

    fn push(&mut self, binding: MutableNamespaceBinding) -> MutableNamespaceBindingId {
        let id = MutableNamespaceBindingId::from_index(self.bindings.len());
        self.bindings.push(Some(binding));
        id
    }

    fn get(&self, id: MutableNamespaceBindingId) -> &MutableNamespaceBinding {
        self.bindings[id.index()]
            .as_ref()
            .expect("live namespace binding id should resolve")
    }

    fn get_mut(&mut self, id: MutableNamespaceBindingId) -> &mut MutableNamespaceBinding {
        self.bindings[id.index()]
            .as_mut()
            .expect("live namespace binding id should resolve")
    }

    fn take(&mut self, id: MutableNamespaceBindingId) -> MutableNamespaceBinding {
        self.bindings[id.index()]
            .take()
            .expect("frozen namespace binding should be live")
    }
}
