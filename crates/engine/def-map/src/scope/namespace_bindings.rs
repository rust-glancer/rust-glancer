//! Sparse storage for the namespace slots owned by one scope name.
//!
//! A [`ScopeEntryBuilder`] belongs to one spelling in `ModuleScopeBuilder`, such as `Widget`. That
//! spelling can have a type, value, and macro meaning, but most names occupy only one of those
//! slots. The first occupied slot therefore stays inside the entry. A second or third slot is kept
//! in one arena shared by the complete module scope, which avoids a small allocation for every
//! multi-namespace name.
//!
//! Freezing reverses that split representation. Each entry takes its additional arena nodes and
//! writes a self-contained [`FrozenScopeBindings`] value in type/value/macro order. Frozen lookup
//! can then borrow a normal `ScopeEntryRef` without exposing either the sparse layout or its arena
//! ids. The surrounding `scope` module owns binding precedence and visibility semantics; this
//! module only owns the mutable and retained storage shapes used to preserve those decisions.

use std::{cmp::Ordering, num::NonZeroUsize};

use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

use super::{
    Namespace, PerNs, ScopeBinding, ScopeEntry, ScopeEntryRef, ScopeResolution, ScopeResolutionRef,
};

/// Retained namespace slots for one textual name after scope resolution has settled.
///
/// Most names occupy exactly one namespace. This includes the very large number of bindings
/// copied through glob imports: a Vulkan bindings crate, for example, can expose thousands of
/// types into hundreds of modules. Keeping three inline [`ScopeResolution`] values would make
/// every one of those entries pay for two empty slots. Store the common first slot inline and
/// allocate only the additional occupied slots instead. A missing namespace is represented by the
/// absence of a [`FrozenNamespaceBinding`], and lookup exposes it as `ScopeResolutionRef::Empty`.
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
    pub(super) fn resolution(&self, namespace: Namespace) -> ScopeResolutionRef<'_> {
        let resolution = match self {
            Self::Empty => None,
            Self::One(binding) => (binding.namespace == namespace).then_some(&binding.resolution),
            Self::Multiple { first, rest } => {
                std::iter::once(first)
                    .chain(rest.iter())
                    .find_map(|binding| {
                        (binding.namespace == namespace).then_some(&binding.resolution)
                    })
            }
        };
        resolution
            .map(ScopeResolution::as_ref)
            .unwrap_or(ScopeResolutionRef::Empty)
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
    resolution: ScopeResolution,
}

/// Mutable namespace state for one name in `ModuleScopeBuilder::names`.
///
/// The entry owns its first occupied namespace. Additional namespaces belong to the scope-wide
/// [`MutableNamespaceBindingArena`], so every operation that may follow those slots receives the
/// arena beside the entry.
#[derive(Debug, Clone, Default)]
pub(super) struct ScopeEntryBuilder {
    bindings: MutableScopeBindings,
}

impl ScopeEntryBuilder {
    /// Insert one route into this name's namespace and report whether its selected result changed.
    pub(super) fn insert_binding(
        &mut self,
        namespace: Namespace,
        binding: ScopeBinding,
        additional: &mut MutableNamespaceBindingArena,
    ) -> bool {
        self.bindings.insert_binding(namespace, binding, additional)
    }

    /// Reconstruct the dense borrowed view expected by scope consumers without allocating.
    ///
    /// The view hides which namespace is inline and which ones live in the shared arena.
    pub(super) fn as_ref<'a>(
        &'a self,
        additional: &'a MutableNamespaceBindingArena,
    ) -> ScopeEntryRef<'a> {
        let resolution = |namespace| {
            self.bindings
                .resolution(namespace, additional)
                .map(ScopeResolutionBuilder::as_ref)
                .unwrap_or(ScopeResolutionRef::Empty)
        };
        ScopeEntryRef {
            bindings: PerNs::new(
                resolution(Namespace::Types),
                resolution(Namespace::Values),
                resolution(Namespace::Macros),
            ),
        }
    }

    /// Compare the selected namespace contents used by import fixed-point convergence.
    ///
    /// Arena ids only record insertion order. Two import passes can build the same visible scope
    /// while assigning different ids, so equality must follow namespace slots and compare their
    /// resolutions instead.
    pub(super) fn has_same_bindings(
        &self,
        additional: &MutableNamespaceBindingArena,
        other: &Self,
        other_additional: &MutableNamespaceBindingArena,
    ) -> bool {
        Namespace::ALL.into_iter().all(|namespace| {
            self.bindings.resolution(namespace, additional)
                == other.bindings.resolution(namespace, other_additional)
        })
    }

    /// Visit every selected candidate, including each candidate in an ambiguous namespace.
    pub(super) fn for_each_binding_mut(
        &mut self,
        additional: &mut MutableNamespaceBindingArena,
        mut apply: impl FnMut(Namespace, &mut ScopeBinding),
    ) {
        self.bindings
            .for_each_resolution_mut(additional, |namespace, resolution| {
                resolution.for_each_binding_mut(|binding| apply(namespace, binding));
            });
    }

    /// Take this name's arena nodes and produce one self-contained frozen entry.
    ///
    /// `ModuleScopeBuilder` checks that every arena node was claimed after all names are frozen.
    pub(super) fn freeze(self, additional: &mut MutableNamespaceBindingArena) -> ScopeEntry {
        ScopeEntry {
            bindings: self.bindings.freeze(additional),
        }
    }
}

/// Sparse namespace slots used while one scope is still changing.
///
/// Glob expansion creates millions of entries that occupy only one namespace. Keeping three inline
/// [`ScopeResolutionBuilder`] values made those entries pay for two empty values throughout the
/// fixed point, exactly when mutable and frozen scopes overlap at the memory peak. Additional
/// occupied namespaces live in one module-owned arena so common type/value pairs do not create a
/// separate allocator object for every name.
///
/// ```text
/// Widget => first: Types(Resolved(...))
///           additional: id -> Values(Resolved(...)) -> Macros(Ambiguous(...))
/// ```
///
/// The inline slot is whichever namespace was inserted first. That choice is only storage; lookup,
/// equality, and freezing all use the namespace tag rather than the slot's position.
#[derive(Debug, Clone, Default)]
enum MutableScopeBindings {
    #[default]
    Empty,
    Occupied {
        first: MutableNamespaceBinding,
        additional: Option<MutableNamespaceBindingId>,
    },
}

impl MutableScopeBindings {
    /// Find the namespace once, then either update its selection or append a new occupied slot.
    ///
    /// A scope entry has at most three occupied namespaces, so the shared-arena chain has at most
    /// two nodes. Keeping this as one walk matters because every declaration and imported binding
    /// passes through it during the fixed point.
    fn insert_binding(
        &mut self,
        namespace: Namespace,
        binding: ScopeBinding,
        arena: &mut MutableNamespaceBindingArena,
    ) -> bool {
        match self {
            Self::Empty => {
                *self = Self::Occupied {
                    first: MutableNamespaceBinding {
                        namespace,
                        resolution: ScopeResolutionBuilder::Resolved(binding),
                    },
                    additional: None,
                };
                true
            }
            Self::Occupied { first, additional } => {
                if first.namespace == namespace {
                    return first.resolution.insert(binding);
                }

                // The list contains at most two nodes, but it is still important to walk it once:
                // insertion is on the hot path for every direct and imported scope binding.
                let mut next = *additional;
                while let Some(binding_id) = next {
                    let (matches, following) = {
                        let node = arena.get(binding_id);
                        (node.binding.namespace == namespace, node.next)
                    };
                    if matches {
                        return arena.get_mut(binding_id).binding.resolution.insert(binding);
                    }
                    next = following;
                }

                *additional = Some(arena.push(
                    MutableNamespaceBinding {
                        namespace,
                        resolution: ScopeResolutionBuilder::Resolved(binding),
                    },
                    *additional,
                ));
                true
            }
        }
    }

    fn resolution<'a>(
        &'a self,
        namespace: Namespace,
        arena: &'a MutableNamespaceBindingArena,
    ) -> Option<&'a ScopeResolutionBuilder> {
        match self {
            Self::Empty => None,
            Self::Occupied { first, additional } => {
                if first.namespace == namespace {
                    return Some(&first.resolution);
                }
                let mut next = *additional;
                while let Some(binding_id) = next {
                    let node = arena.get(binding_id);
                    if node.binding.namespace == namespace {
                        return Some(&node.binding.resolution);
                    }
                    next = node.next;
                }
                None
            }
        }
    }

    fn for_each_resolution_mut(
        &mut self,
        arena: &mut MutableNamespaceBindingArena,
        mut apply: impl FnMut(Namespace, &mut ScopeResolutionBuilder),
    ) {
        match self {
            Self::Empty => {}
            Self::Occupied { first, additional } => {
                apply(first.namespace, &mut first.resolution);
                let mut next = *additional;
                while let Some(binding_id) = next {
                    let node = arena.get_mut(binding_id);
                    next = node.next;
                    apply(node.binding.namespace, &mut node.binding.resolution);
                }
            }
        }
    }

    /// Drain this name's arena nodes into a deterministic, self-contained frozen value.
    fn freeze(self, arena: &mut MutableNamespaceBindingArena) -> FrozenScopeBindings {
        let mut sorted = [None, None, None];
        let mut retain = |binding: MutableNamespaceBinding| {
            let rank = usize::from(binding.namespace.sort_rank());
            let previous = sorted[rank].replace(FrozenNamespaceBinding {
                namespace: binding.namespace,
                resolution: binding.resolution.freeze(),
            });
            debug_assert!(
                previous.is_none(),
                "each namespace should have one occupied slot"
            );
        };

        match self {
            Self::Empty => return FrozenScopeBindings::Empty,
            Self::Occupied { first, additional } => {
                retain(first);
                let mut next = additional;
                while let Some(binding_id) = next {
                    let node = arena.take(binding_id);
                    next = node.next;
                    retain(node.binding);
                }
            }
        }

        // Namespace order is part of frozen equality and serialization, so make it independent of
        // the order in which declarations and imports happened to populate the sparse slots.
        let mut bindings = sorted.into_iter().flatten();
        let first = bindings
            .next()
            .expect("an occupied namespace set should retain at least one binding");
        let frozen = match (bindings.next(), bindings.next()) {
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
        debug_assert!(bindings.next().is_none());
        frozen
    }
}

/// One occupied mutable namespace slot.
///
/// Empty namespaces are absent from `MutableScopeBindings`, so this value only needs the namespace
/// tag and the resolution being selected for it.
#[derive(Debug, Clone)]
struct MutableNamespaceBinding {
    namespace: Namespace,
    resolution: ScopeResolutionBuilder,
}

/// Arena node for a name's second or third occupied namespace.
#[derive(Debug, Clone)]
struct MutableNamespaceBindingNode {
    binding: MutableNamespaceBinding,
    next: Option<MutableNamespaceBindingId>,
}

/// One-based arena index used by the linked sparse slots.
///
/// `NonZeroUsize` lets `Option<MutableNamespaceBindingId>` use the same space as the id itself.
#[derive(Debug, Clone, Copy)]
struct MutableNamespaceBindingId(NonZeroUsize);

impl MutableNamespaceBindingId {
    fn from_index(index: usize) -> Self {
        let one_based = index
            .checked_add(1)
            .expect("namespace binding index should fit usize");
        Self(NonZeroUsize::new(one_based).expect("one-based namespace binding index is nonzero"))
    }

    fn index(self) -> usize {
        self.0.get() - 1
    }
}

/// Scope-owned storage for namespace slots that do not fit inline in their name entry.
///
/// Every node belongs to exactly one `ScopeEntryBuilder`. Freezing names can happen in hash-map
/// iteration order, so nodes are wrapped in `Option`: an entry takes its nodes without moving the
/// ids still held by other entries. The arena has no reuse path because scope construction only
/// appends slots and the complete arena is discarded after freezing.
#[derive(Debug, Clone, Default)]
pub(super) struct MutableNamespaceBindingArena {
    nodes: Vec<Option<MutableNamespaceBindingNode>>,
}

impl MutableNamespaceBindingArena {
    pub(super) fn is_drained(&self) -> bool {
        self.nodes.iter().all(Option::is_none)
    }

    fn push(
        &mut self,
        binding: MutableNamespaceBinding,
        next: Option<MutableNamespaceBindingId>,
    ) -> MutableNamespaceBindingId {
        let id = MutableNamespaceBindingId::from_index(self.nodes.len());
        self.nodes
            .push(Some(MutableNamespaceBindingNode { binding, next }));
        id
    }

    fn get(&self, id: MutableNamespaceBindingId) -> &MutableNamespaceBindingNode {
        self.nodes[id.index()]
            .as_ref()
            .expect("live namespace binding id should resolve")
    }

    fn get_mut(&mut self, id: MutableNamespaceBindingId) -> &mut MutableNamespaceBindingNode {
        self.nodes[id.index()]
            .as_mut()
            .expect("live namespace binding id should resolve")
    }

    fn take(&mut self, id: MutableNamespaceBindingId) -> MutableNamespaceBindingNode {
        self.nodes[id.index()]
            .take()
            .expect("frozen namespace binding should be live")
    }
}

/// Selection state for an occupied namespace while declarations and imports are still arriving.
///
/// Absence lives in `MutableScopeBindings`, so there is no `Empty` variant here. A local or named
/// binding replaces a glob, two equal-priority routes to the same definition merge, and two
/// equal-priority definitions remain explicit candidates in `Ambiguous`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ScopeResolutionBuilder {
    Resolved(ScopeBinding),
    Ambiguous(Vec<ScopeBinding>),
}

impl ScopeResolutionBuilder {
    /// Apply binding precedence and route merging to one occupied namespace slot.
    ///
    /// For example, a local `struct Item` replaces an earlier `use other::*` candidate. Two named
    /// imports of different `Item` definitions instead leave the slot ambiguous.
    fn insert(&mut self, binding: ScopeBinding) -> bool {
        match self {
            Self::Resolved(existing) => match binding.priority().cmp(&existing.priority()) {
                Ordering::Less => false,
                Ordering::Greater => {
                    *self = Self::Resolved(binding);
                    true
                }
                Ordering::Equal if binding.def == existing.def => existing.merge_routes(binding),
                Ordering::Equal => {
                    let existing = existing.clone();
                    *self = Self::Ambiguous(vec![existing, binding]);
                    true
                }
            },
            Self::Ambiguous(existing) => {
                let priority = existing
                    .first()
                    .expect("ambiguous scope slot should contain bindings")
                    .priority();
                match binding.priority().cmp(&priority) {
                    Ordering::Less => false,
                    Ordering::Greater => {
                        *self = Self::Resolved(binding);
                        true
                    }
                    Ordering::Equal => {
                        if let Some(same_def) = existing
                            .iter_mut()
                            .find(|candidate| candidate.def == binding.def)
                        {
                            same_def.merge_routes(binding)
                        } else {
                            existing.push(binding);
                            true
                        }
                    }
                }
            }
        }
    }

    fn as_ref(&self) -> ScopeResolutionRef<'_> {
        match self {
            Self::Resolved(binding) => ScopeResolutionRef::Resolved(binding),
            Self::Ambiguous(bindings) => ScopeResolutionRef::Ambiguous(bindings),
        }
    }

    fn for_each_binding_mut(&mut self, mut apply: impl FnMut(&mut ScopeBinding)) {
        match self {
            Self::Resolved(binding) => apply(binding),
            Self::Ambiguous(bindings) => {
                for binding in bindings {
                    apply(binding);
                }
            }
        }
    }

    fn freeze(self) -> ScopeResolution {
        match self {
            Self::Resolved(binding) => ScopeResolution::Resolved(binding),
            Self::Ambiguous(bindings) => ScopeResolution::Ambiguous(bindings.into_boxed_slice()),
        }
    }
}
