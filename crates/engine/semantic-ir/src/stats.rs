/// Coarse semantic IR counts used by CLI/status reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SemanticIrStats {
    pub target_count: usize,
    pub struct_count: usize,
    pub union_count: usize,
    pub enum_count: usize,
    pub trait_count: usize,
    pub impl_count: usize,
    pub function_count: usize,
    pub type_alias_count: usize,
    pub const_count: usize,
    pub static_count: usize,
}
