//! Typed dense arenas for phase-local rust-glancer ids.
//!
//! The crate intentionally models only the simple arena shape used throughout the engine:
//! builders allocate values into a dense `Arena`, and retained project snapshots compact that
//! storage through the workspace-wide `Shrink` hierarchy.

use std::{
    marker::PhantomData,
    ops::{Index, IndexMut},
    slice,
};
use wincode::{SchemaRead, SchemaWrite};

/// Stable typed index into an arena.
///
/// Implementations should be tiny newtypes around `usize`. The conversion methods are deliberately
/// boring so arena storage can remain a plain dense array while callers avoid mixing unrelated ids.
pub trait ArenaId: Copy + Eq {
    fn from_index(index: usize) -> Self;
    fn index(self) -> usize;
}

/// Declares a typed arena id and implements `ArenaId` for it.
///
/// The one-argument form follows ordinary Rust item visibility and declares a private id:
///
/// ```
/// rg_arena::arena_id!(ExprId);
/// ```
///
/// Specify visibility explicitly when the id should cross a module or crate boundary:
///
/// ```
/// rg_arena::arena_id!(pub Id);
/// rg_arena::arena_id!(pub(crate) LocalId);
/// ```
#[macro_export]
macro_rules! arena_id {
    ($(#[$attr:meta])* $name:ident) => {
        $(#[$attr])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        struct $name(usize);

        impl $crate::ArenaId for $name {
            fn from_index(index: usize) -> Self {
                Self(index)
            }

            fn index(self) -> usize {
                self.0
            }
        }
    };
    ($(#[$attr:meta])* $vis:vis $name:ident) => {
        $(#[$attr])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        $vis struct $name(usize);

        impl $crate::ArenaId for $name {
            fn from_index(index: usize) -> Self {
                Self(index)
            }

            fn index(self) -> usize {
                self.0
            }
        }
    };
}

/// Mutable dense arena used while lowering/building a phase.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct Arena<Id, T> {
    items: Vec<T>,
    _marker: PhantomData<fn(Id) -> Id>,
}

impl<Id, T> Default for Arena<Id, T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<Id, T> Arena<Id, T> {
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            _marker: PhantomData,
        }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            items: Vec::with_capacity(capacity),
            _marker: PhantomData,
        }
    }

    pub fn from_vec(items: Vec<T>) -> Self {
        Self {
            items,
            _marker: PhantomData,
        }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn capacity(&self) -> usize {
        self.items.capacity()
    }

    pub fn as_slice(&self) -> &[T] {
        &self.items
    }

    pub fn as_mut_slice(&mut self) -> &mut [T] {
        &mut self.items
    }

    pub fn iter(&self) -> slice::Iter<'_, T> {
        self.items.iter()
    }

    pub fn iter_mut(&mut self) -> slice::IterMut<'_, T> {
        self.items.iter_mut()
    }

    pub fn shrink_to_fit(&mut self) {
        self.items.shrink_to_fit();
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }

    pub fn resize_with(&mut self, new_len: usize, f: impl FnMut() -> T) {
        self.items.resize_with(new_len, f);
    }
}

impl<Id, T> Arena<Id, T>
where
    Id: ArenaId,
{
    pub fn alloc(&mut self, item: T) -> Id {
        let id = Id::from_index(self.items.len());
        self.items.push(item);
        id
    }

    pub fn get(&self, id: Id) -> Option<&T> {
        self.items.get(id.index())
    }

    pub fn get_mut(&mut self, id: Id) -> Option<&mut T> {
        self.items.get_mut(id.index())
    }

    pub fn next_id(&self) -> Id {
        Id::from_index(self.items.len())
    }

    pub fn iter_with_ids(&self) -> impl Iterator<Item = (Id, &T)> {
        self.items
            .iter()
            .enumerate()
            .map(|(index, item)| (Id::from_index(index), item))
    }

    pub fn iter_mut_with_ids(&mut self) -> impl Iterator<Item = (Id, &mut T)> {
        self.items
            .iter_mut()
            .enumerate()
            .map(|(index, item)| (Id::from_index(index), item))
    }
}

impl<Id, T> Index<Id> for Arena<Id, T>
where
    Id: ArenaId,
{
    type Output = T;

    fn index(&self, id: Id) -> &Self::Output {
        &self.items[id.index()]
    }
}

impl<Id, T> IndexMut<Id> for Arena<Id, T>
where
    Id: ArenaId,
{
    fn index_mut(&mut self, id: Id) -> &mut Self::Output {
        &mut self.items[id.index()]
    }
}

impl<'a, Id, T> IntoIterator for &'a Arena<Id, T> {
    type Item = &'a T;
    type IntoIter = slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.iter()
    }
}

impl<'a, Id, T> IntoIterator for &'a mut Arena<Id, T> {
    type Item = &'a mut T;
    type IntoIter = slice::IterMut<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.iter_mut()
    }
}

mod memsize {
    use rg_std::{MemoryRecorder, MemorySize};

    use crate::Arena;

    impl<Id, T> MemorySize for Arena<Id, T>
    where
        T: MemorySize,
    {
        fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
            self.items.record_memory_children(recorder);
        }
    }
}

mod shrink {
    use rg_std::Shrink;

    use crate::Arena;

    impl<Id, T> Shrink for Arena<Id, T>
    where
        T: Shrink,
    {
        fn shrink_to_fit(&mut self) {
            Shrink::shrink_to_fit(&mut self.items);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{Arena, ArenaId};

    crate::arena_id!(ExprId);
    crate::arena_id!(pub(crate) LocalId);

    #[test]
    fn macro_declares_ids_and_implements_arena_id() {
        let expr = ExprId::from_index(7);
        let local = LocalId::from_index(3);

        assert_eq!(expr.index(), 7);
        assert_eq!(local.index(), 3);
    }

    #[test]
    fn allocates_dense_typed_ids() {
        let mut arena = Arena::<ExprId, &'static str>::new();

        let first = arena.alloc("first");
        let second = arena.alloc("second");

        assert_eq!(first.index(), 0);
        assert_eq!(second.index(), 1);
        assert_eq!(arena.get(first), Some(&"first"));
        assert_eq!(arena[second], "second");
    }

    #[test]
    fn iterates_with_ids() {
        let mut arena = Arena::<ExprId, &'static str>::new();
        arena.alloc("left");
        arena.alloc("right");

        let items = arena.iter_with_ids().collect::<Vec<_>>();

        let items = items
            .into_iter()
            .map(|(id, item)| (id.index(), item))
            .collect::<Vec<_>>();
        assert_eq!(items, vec![(0, &"left"), (1, &"right")]);
    }

    #[test]
    fn wraps_existing_vectors_without_losing_iteration_shape() {
        let arena = Arena::<ExprId, _>::from_vec(vec!["left", "right"]);

        assert_eq!(arena.get(ExprId::from_index(1)), Some(&"right"));
        assert_eq!(
            (&arena).into_iter().copied().collect::<Vec<_>>(),
            vec!["left", "right"]
        );
    }

    #[test]
    fn records_arena_memory_without_losing_container_shape() {
        use std::mem;

        use rg_std::{MemoryRecordKind, MemoryRecorder, MemorySize};

        let mut arena = Arena::<ExprId, String>::with_capacity(2);
        arena.alloc("user".to_string());

        let mut recorder = MemoryRecorder::new("arena");
        arena.record_memory_size(&mut recorder);
        let totals = recorder.totals_by_kind();

        assert_eq!(
            totals.get(&MemoryRecordKind::Shallow),
            Some(&mem::size_of::<Arena<ExprId, String>>())
        );
        assert!(
            totals
                .get(&MemoryRecordKind::Heap)
                .is_some_and(|bytes| *bytes > 0)
        );
        assert!(
            totals
                .get(&MemoryRecordKind::SpareCapacity)
                .is_some_and(|bytes| *bytes >= mem::size_of::<String>())
        );

        let paths = recorder
            .records()
            .into_iter()
            .map(|record| record.path)
            .collect::<Vec<_>>();
        assert!(paths.iter().any(|path| path == "arena.items"));
        assert!(!paths.iter().any(|path| path.contains("items.items")));
    }

    #[test]
    fn shrinks_arena_storage_and_children() {
        use rg_std::Shrink;

        let mut arena = Arena::<ExprId, String>::with_capacity(8);
        let id = arena.alloc(String::with_capacity(8));

        Shrink::shrink_to_fit(&mut arena);

        assert_eq!(arena.capacity(), arena.len());
        assert_eq!(arena[id].capacity(), arena[id].len());
    }
}
