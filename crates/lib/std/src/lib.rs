//! Small project-wide foundations for rust-glancer.
//!
//! This crate keeps low-level utilities that are useful across engine layers without belonging to
//! one of those layers. Root re-exports cover the common ergonomic imports; modules keep the actual
//! ownership visible for more deliberate call sites.
//
// Note:
// The goal of this crate is to keep _highly reusable_ bits that are useful throughout the whole
// workspace. It should not be treated as "utils" crate to put stuff you don't know where to put.
// Putting things here should have good enough justification.

pub mod memsize;
pub mod shrink;
pub mod unique;

pub use self::{
    memsize::{MemoryRecord, MemoryRecordKind, MemoryRecorder, MemoryRecorderMode, MemorySize},
    shrink::Shrink,
    unique::UniqueVec,
};
