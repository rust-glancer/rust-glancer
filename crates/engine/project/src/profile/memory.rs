/// Process allocator counters sampled by the executable during a profiled build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuildProcessMemory {
    pub allocated_bytes: Option<usize>,
    pub active_bytes: Option<usize>,
    pub resident_bytes: Option<usize>,
    pub mapped_bytes: Option<usize>,
}

pub type ProcessMemorySampler = Box<dyn FnMut() -> Option<BuildProcessMemory>>;
