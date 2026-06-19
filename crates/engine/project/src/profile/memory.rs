use rg_std::MemorySize;

/// Process allocator counters sampled by the executable during a profiled build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuildProcessMemory {
    pub allocated_bytes: usize,
    pub active_bytes: usize,
    pub resident_bytes: usize,
    pub mapped_bytes: usize,
}

pub type ProcessMemorySampler = Box<dyn FnMut() -> Option<BuildProcessMemory>>;

pub(crate) enum BuildMemorySampler {
    Disabled,
    Retained {
        process_memory: Option<ProcessMemorySampler>,
    },
}

impl BuildMemorySampler {
    pub(crate) fn disabled() -> Self {
        Self::Disabled
    }

    pub(crate) fn retained(process_memory: Option<ProcessMemorySampler>) -> Self {
        Self::Retained { process_memory }
    }

    pub(crate) fn with_retained_memory(self, enabled: bool) -> Self {
        if !enabled {
            return Self::Disabled;
        }

        match self {
            Self::Disabled => Self::retained(None),
            Self::Retained { process_memory } => Self::retained(process_memory),
        }
    }

    pub(crate) fn with_process_memory(self, process_memory: ProcessMemorySampler) -> Self {
        match self {
            Self::Disabled | Self::Retained { .. } => Self::retained(Some(process_memory)),
        }
    }

    pub(crate) fn measure_retained<T>(&self, value: &T) -> Option<usize>
    where
        T: MemorySize,
    {
        match self {
            Self::Disabled => None,
            Self::Retained { .. } => Some(value.memory_size()),
        }
    }

    pub(crate) fn sum_retained(&self, values: &[Option<usize>]) -> Option<usize> {
        match self {
            Self::Disabled => None,
            Self::Retained { .. } => Some(values.iter().flatten().copied().sum()),
        }
    }

    pub(crate) fn sample_process_memory(&mut self) -> Option<BuildProcessMemory> {
        match self {
            Self::Disabled => None,
            Self::Retained { process_memory } => {
                process_memory.as_mut().and_then(|sampler| sampler())
            }
        }
    }
}
