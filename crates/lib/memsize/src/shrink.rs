/// Releases spare heap capacity retained inside a value.
///
/// This is intentionally separate from `MemorySize`: some generic data models need to ask their
/// embedded storage-specific values to compact themselves without also knowing how those values
/// report memory usage.
pub trait Shrink {
    fn shrink_to_fit(&mut self);
}
