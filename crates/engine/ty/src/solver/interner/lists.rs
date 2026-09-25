//! Reuse equal sequences before copying their elements into the operation's arena.
//!
//! Interning a type happens after its argument list has been built. Without separate slice
//! interning, repeated `Vec<u8>` construction would keep allocating `[u8]` even when the type
//! itself is already present. The tables here borrow their slices from the same arena as types.

use std::{cell::RefCell, hash::Hash};

use rustc_type_ir::{self as ir, data_structures::HashSet};

use super::SolverInterner;
use crate::{
    profile::metric,
    solver::{
        profile::SolverProfile,
        types::{Clause, Const, DefId, GenericArg, Pattern, Ty},
    },
};

/// Element types supported by the solver's slice storage. Each implementation selects a typed
/// table, so list construction can stay generic without erasing element types or lifetimes.
pub trait ListElement<'s>: Copy + Eq + Hash {
    fn intern(cx: SolverInterner<'s>, values: &[Self]) -> &'s [Self];
}

struct SliceInterner<'s, T> {
    values: HashSet<&'s [T]>,
    requests: u64,
    bytes: u64,
}

impl<T> Default for SliceInterner<'_, T> {
    fn default() -> Self {
        Self {
            values: HashSet::default(),
            requests: 0,
            bytes: 0,
        }
    }
}

impl<'s, T: Copy + Eq + Hash> SliceInterner<'s, T> {
    fn intern(&mut self, arena: &'s bumpalo::Bump, values: &[T]) -> &'s [T] {
        self.requests += 1;
        if let Some(&interned) = self.values.get(values) {
            return interned;
        }
        // Borrowed slice keys compare the sequence contents, including order and repetition.
        // Only a miss consumes arena space; the table retains a reference to that same copy.
        let interned = arena.alloc_slice_copy(values);
        self.values.insert(interned);
        self.bytes += std::mem::size_of_val(values) as u64;
        interned
    }

    fn record_profile(&self, name: &'static str, profile: &mut SolverProfile) {
        if self.requests == 0 {
            return;
        }
        profile.slice_requests += self.requests;
        profile.slice_bytes += self.bytes;
        profile.slice_entries += self.values.len() as u64;
        profile.slice_capacity += self.values.capacity() as u64;
        profile.slice_capacity_bytes +=
            (self.values.capacity() * std::mem::size_of::<&[T]>()) as u64;
        metric::SOLVER_SLICE_REQUESTS_BY_KIND.add(name, self.requests);
        metric::SOLVER_SLICE_HITS_BY_KIND.add(name, self.requests - self.values.len() as u64);
        metric::SOLVER_SLICE_BYTES_BY_KIND.add(name, self.bytes);
    }
}

// Keep the finite set of compiler list types in one place. The macro supplies only table
// selection and reporting; allocation and lookup remain the same for every element type.
macro_rules! list_interners {
    ($($name:ident: $element:ty),+ $(,)?) => {
        #[derive(Default)]
        pub(crate) struct ListInterners<'s> {
            $($name: RefCell<SliceInterner<'s, $element>>),+
        }

        impl ListInterners<'_> {
            pub(crate) fn record_profile(&self, profile: &mut SolverProfile) {
                $(self.$name.borrow().record_profile(stringify!($name), profile);)+
            }
        }

        $(impl<'s> ListElement<'s> for $element {
            fn intern(cx: SolverInterner<'s>, values: &[Self]) -> &'s [Self] {
                if values.is_empty() {
                    return &[];
                }
                cx.0.lists.$name.borrow_mut().intern(&cx.0.arena, values)
            }
        })+
    };
}

list_interners! {
    types: Ty<'s>,
    consts: Const<'s>,
    arguments: GenericArg<'s>,
    clauses: Clause<'s>,
    bound_vars: ir::BoundVariableKind<SolverInterner<'s>>,
    canonical_vars: ir::CanonicalVarKind<SolverInterner<'s>>,
    existential_predicates: ir::Binder<SolverInterner<'s>, ir::ExistentialPredicate<SolverInterner<'s>>>,
    predefined_opaques: (ir::OpaqueTypeKey<SolverInterner<'s>>, Ty<'s>),
    definitions: DefId,
    region_assumptions: ir::OutlivesPredicate<SolverInterner<'s>, GenericArg<'s>>,
    variances: ir::Variance,
    patterns: Pattern<'s>,
}
