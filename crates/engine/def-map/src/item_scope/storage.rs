//! Storage for the namespace bindings belonging to one spelling.
//!
//! `NameBindings` owns the frozen answers; `NameBindingsView` is the read-only shape shared by
//! mutable and frozen tables. The remaining types pack those answers: one occupied namespace
//! stays inline, extra mutable slots share an arena, and extra frozen slots use a boxed slice.
//! Storage slots are implementation details, not additional kinds of name resolution.

use std::num::NonZeroU32;

use rg_ir_model::ImportId;
use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

use crate::{NameBinding, Namespace, PerNs};

/// Selected bindings for one spelling, with at most one answer in each namespace.
///
/// The spelling itself is the key in `ItemScope`. This value keeps its type, value, and macro
/// answers. Empty namespaces occupy no retained slot; `binding` returns `None` for them without
/// exposing the sparse representation to query code.
#[derive(Debug, Clone, PartialEq, Eq, Default, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct NameBindings {
    bindings: BindingsStorage,
}

impl NameBindings {
    pub fn binding(&self, namespace: Namespace) -> Option<&NameBinding> {
        self.bindings.binding(namespace)
    }

    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }

    pub fn as_view(&self) -> NameBindingsView<'_> {
        NameBindingsView {
            bindings: PerNs::new(
                self.binding(Namespace::Types),
                self.binding(Namespace::Values),
                self.binding(Namespace::Macros),
            ),
        }
    }
}

/// Read-only namespace bindings for one spelling in a mutable or frozen item scope.
///
/// Path resolution reads this common shape while construction is still changing scopes and after
/// those scopes have been frozen. Keeping the storage distinction behind this view prevents the
/// resolver from having separate mutable and retained lookup paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NameBindingsView<'a> {
    bindings: PerNs<Option<&'a NameBinding>>,
}

impl<'a> NameBindingsView<'a> {
    pub fn binding(self, namespace: Namespace) -> Option<&'a NameBinding> {
        *self.bindings.get(namespace)
    }
}

/// Retained namespace slots for one textual name after scope resolution has settled.
///
/// Most names occupy exactly one namespace. This includes the very large number of bindings
/// copied through glob imports: a Vulkan bindings crate, for example, can expose thousands of
/// types into hundreds of modules. Keeping three inline [`NameBinding`] values would make
/// every one of those entries pay for two empty slots. Store the common first slot inline and
/// allocate only the additional occupied slots instead. A missing namespace is represented by the
/// absence of a [`BindingSlot`], and lookup returns `None` for it.
#[derive(Debug, Clone, PartialEq, Eq, Default, SchemaRead, SchemaWrite, MemorySize, Shrink)]
enum BindingsStorage {
    #[default]
    Empty,
    One(BindingSlot),
    Multiple {
        first: BindingSlot,
        rest: Box<[BindingSlot]>,
    },
}

impl BindingsStorage {
    fn binding(&self, namespace: Namespace) -> Option<&NameBinding> {
        match self {
            Self::Empty => None,
            Self::One(binding) => (binding.namespace == namespace).then_some(&binding.binding),
            Self::Multiple { first, rest } => std::iter::once(first)
                .chain(rest.iter())
                .find_map(|binding| (binding.namespace == namespace).then_some(&binding.binding)),
        }
    }

    fn is_empty(&self) -> bool {
        matches!(self, Self::Empty)
    }
}

/// One occupied namespace and its settled resolution inside the sparse frozen representation.
///
/// The namespace tag is needed because sparse slots are stored consecutively instead of in three
/// fixed type/value/macro fields.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
struct BindingSlot {
    namespace: Namespace,
    binding: NameBinding,
}

/// Mutable namespace state for one name in `ItemScopeBuilder::names`.
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
pub(super) struct NameBindingsBuilder {
    first: Option<BuildSlot>,
    additional: [Option<SlotId>; 2],
}

impl NameBindingsBuilder {
    /// Store a selected binding, allocating an extra slot only for a newly occupied namespace.
    ///
    /// `import` identifies the winning route, not the source of presentation attributes. A changed
    /// binding replaces that route; an unchanged write with no import ID preserves it. Declaration
    /// collection needs the latter when a new declaration loses to an existing explicit import.
    pub(super) fn set_binding(
        &mut self,
        namespace: Namespace,
        binding: NameBinding,
        import: Option<ImportId>,
        arena: &mut SlotArena,
    ) {
        let Some(first) = &mut self.first else {
            self.first = Some(BuildSlot {
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
        *extra = Some(arena.push(BuildSlot {
            namespace,
            binding,
            import,
        }));
    }

    pub(super) fn binding_import(
        &self,
        namespace: Namespace,
        arena: &SlotArena,
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
        arena: &'a SlotArena,
    ) -> Option<&'a NameBinding> {
        self.first
            .iter()
            .chain(self.additional.iter().flatten().map(|id| arena.get(*id)))
            .find(|binding| binding.namespace == namespace)
            .map(|binding| &binding.binding)
    }

    /// Reconstruct the dense borrowed view expected by scope consumers without allocating.
    ///
    /// The view hides which namespace is inline and which ones live in the shared arena.
    pub(super) fn as_view<'a>(&'a self, arena: &'a SlotArena) -> NameBindingsView<'a> {
        NameBindingsView {
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
        arena: &mut SlotArena,
        mut apply: impl FnMut(Namespace, &mut NameBinding),
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
    /// `ItemScopeBuilder` checks that every arena slot was claimed after all names are frozen.
    pub(super) fn freeze(self, arena: &mut SlotArena) -> NameBindings {
        let mut sorted = [None, None, None];
        for binding in self.first.into_iter().chain(
            self.additional
                .into_iter()
                .flatten()
                .map(|id| arena.take(id)),
        ) {
            let rank = usize::from(binding.namespace.sort_rank());
            let previous = sorted[rank].replace(BindingSlot {
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
            return NameBindings {
                bindings: BindingsStorage::Empty,
            };
        };
        let bindings = match (bindings.next(), bindings.next()) {
            (None, None) => BindingsStorage::One(first),
            (Some(second), None) => BindingsStorage::Multiple {
                first,
                rest: Box::new([second]),
            },
            (Some(second), Some(third)) => BindingsStorage::Multiple {
                first,
                rest: Box::new([second, third]),
            },
            (None, Some(_)) => unreachable!("namespace bindings should not contain gaps"),
        };
        NameBindings { bindings }
    }
}

/// One occupied mutable namespace slot.
///
/// Empty namespaces have no slot. Occupied ones carry their namespace tag, selected binding,
/// and the import route allowed to refresh that binding while construction continues.
#[derive(Debug, Clone)]
struct BuildSlot {
    namespace: Namespace,
    binding: NameBinding,
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
struct SlotId(NonZeroU32);

impl SlotId {
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
/// Every slot belongs to exactly one `NameBindingsBuilder`. Freezing names can happen in hash-map
/// iteration order, so slots are wrapped in `Option`: an entry takes its bindings without moving the
/// ids still held by other entries. The arena has no reuse path because scope construction only
/// appends slots and the complete arena is discarded after freezing.
#[derive(Debug, Clone, Default)]
pub(super) struct SlotArena {
    bindings: Vec<Option<BuildSlot>>,
}

impl SlotArena {
    pub(super) fn is_drained(&self) -> bool {
        self.bindings.iter().all(Option::is_none)
    }

    fn push(&mut self, binding: BuildSlot) -> SlotId {
        let id = SlotId::from_index(self.bindings.len());
        self.bindings.push(Some(binding));
        id
    }

    fn get(&self, id: SlotId) -> &BuildSlot {
        self.bindings[id.index()]
            .as_ref()
            .expect("live namespace binding id should resolve")
    }

    fn get_mut(&mut self, id: SlotId) -> &mut BuildSlot {
        self.bindings[id.index()]
            .as_mut()
            .expect("live namespace binding id should resolve")
    }

    fn take(&mut self, id: SlotId) -> BuildSlot {
        self.bindings[id.index()]
            .take()
            .expect("frozen namespace binding should be live")
    }
}
