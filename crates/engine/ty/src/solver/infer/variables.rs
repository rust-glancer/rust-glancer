//! Undoable variable storage. All relations use these tables, including solver responses.

use std::{fmt::Debug, marker::PhantomData};

use ena::unify::{NoError, UnifyKey, UnifyValue};
use rustc_type_ir::UniverseIndex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Value<T> {
    Unknown(UniverseIndex),
    Known(T),
}

impl<T: Copy + Eq + Debug> UnifyValue for Value<T> {
    type Error = NoError;
    fn unify_values(a: &Self, b: &Self) -> Result<Self, NoError> {
        Ok(match (*a, *b) {
            // A shared variable must obey both visibility limits. The older universe cannot
            // name placeholders introduced in the newer one, so it is the limit we retain.
            (Self::Unknown(a), Self::Unknown(b)) => Self::Unknown(a.min(b)),
            (Self::Known(a), Self::Known(b)) => {
                // Known values must be related before their classes are joined.
                assert_eq!(a, b);
                Self::Known(a)
            }
            (known @ Self::Known(_), Self::Unknown(_))
            | (Self::Unknown(_), known @ Self::Known(_)) => known,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Key<T> {
    index: u32,
    marker: PhantomData<T>,
}

impl<T> Key<T> {
    pub fn new(index: u32) -> Self {
        Self {
            index,
            marker: PhantomData,
        }
    }
}

impl<T: Copy + Eq + Debug> UnifyKey for Key<T> {
    type Value = Value<T>;
    fn index(&self) -> u32 {
        self.index
    }

    fn from_index(index: u32) -> Self {
        Self::new(index)
    }

    fn tag() -> &'static str {
        "solver variable"
    }
}
