use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

/// Identifies a recorded piece of syntax without storing its byte range in declaration meaning.
///
/// For example, `fn load(value: User) -> User` has separate occurrences for the two `User` paths.
/// The lowered signature keeps their IDs. A source table keeps their positions. Adding a comment
/// before the function moves those positions without changing the signature or its IDs.
///
/// Both numbers are table indices, never text offsets. `owner` selects a group of occurrences;
/// `local` selects an entry within that group. Declaration lowering starts a group for each item,
/// while body lowering also starts groups for individual type-lowering operations. The owner
/// index is separate from item IDs and semantic declaration identities.
///
/// The containing file, expansion, or body supplies the table. An ID carries neither that table's
/// identity nor a source revision: it must travel with its source context. The same numbers in
/// another table can name unrelated syntax. IDs stay reusable while lowering records the same
/// structure in the same order; they are not stable identities across arbitrary source edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite, MemorySize, Shrink)]
#[memsize(leaf)]
#[shrink(leaf)]
pub struct SourceId {
    /// Group allocated before recording its syntax or descending into nested declarations.
    pub owner: u32,
    /// Recording order within that group, independent of how many entries its children have.
    pub local: u32,
}
