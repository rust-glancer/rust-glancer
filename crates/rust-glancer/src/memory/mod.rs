//! Process memory controls for the CLI/LSP binary.
//!
//! Allocator choice stays at the executable boundary. The analysis engine only sees normalized,
//! optional counters, while each allocator owns its setup, statistics, and cleanup behavior in a
//! separate backend module.

#[cfg(all(feature = "jemalloc", feature = "mimalloc"))]
compile_error!(
    "the `jemalloc` and `mimalloc` allocator features are mutually exclusive; disable default \
     features before selecting jemalloc"
);

use std::sync::Arc;

use rg_lsp_engine::{AllocatorStats, MemoryControl};
use rg_project::{ProjectMemoryHooks, ProjectMemoryPurgePoint};

#[cfg(all(
    not(feature = "mimalloc"),
    feature = "jemalloc",
    not(any(target_env = "msvc", target_os = "openbsd"))
))]
mod jemalloc;
#[cfg(feature = "mimalloc")]
mod mimalloc;
#[cfg(all(
    not(feature = "mimalloc"),
    any(not(feature = "jemalloc"), target_env = "msvc", target_os = "openbsd")
))]
mod system;

#[cfg(all(
    not(feature = "mimalloc"),
    feature = "jemalloc",
    not(any(target_env = "msvc", target_os = "openbsd"))
))]
use self::jemalloc as allocator;
#[cfg(feature = "mimalloc")]
use self::mimalloc as allocator;
#[cfg(all(
    not(feature = "mimalloc"),
    any(not(feature = "jemalloc"), target_env = "msvc", target_os = "openbsd")
))]
use self::system as allocator;

#[derive(Debug, Clone, Copy)]
pub(crate) struct ProcessMemoryControl;

impl MemoryControl for ProcessMemoryControl {
    fn allocator_name(&self) -> &'static str {
        Self::allocator_name()
    }

    fn allocator_stats(&self) -> Option<AllocatorStats> {
        Self::allocator_stats()
    }

    fn try_purge_allocator(&self) -> bool {
        Self::try_purge_allocator()
    }
}

pub(crate) fn memory_control() -> ProcessMemoryControl {
    ProcessMemoryControl
}

pub(crate) fn project_memory_hooks() -> Arc<dyn ProjectMemoryHooks> {
    Arc::new(ProjectProcessMemoryHooks {
        memory_control: memory_control(),
    })
}

#[derive(Debug, Clone, Copy)]
struct ProjectProcessMemoryHooks {
    memory_control: ProcessMemoryControl,
}

impl ProjectMemoryHooks for ProjectProcessMemoryHooks {
    fn purge(&self, _point: ProjectMemoryPurgePoint) {
        self.memory_control.try_purge_allocator();
    }
}

impl ProcessMemoryControl {
    pub(crate) fn allocator_name() -> &'static str {
        allocator::NAME
    }

    pub(crate) fn allocator_stats() -> Option<AllocatorStats> {
        allocator::capture_stats()
    }

    pub(crate) fn try_purge_allocator() -> bool {
        allocator::try_purge()
    }
}

#[cfg(test)]
mod tests {
    use super::ProcessMemoryControl;

    #[cfg(all(
        not(feature = "mimalloc"),
        feature = "jemalloc",
        not(any(target_env = "msvc", target_os = "openbsd"))
    ))]
    #[test]
    fn jemalloc_is_the_selected_allocator() {
        assert_eq!(ProcessMemoryControl::allocator_name(), "jemalloc");

        #[cfg(feature = "jemalloc-stats")]
        {
            let stats =
                ProcessMemoryControl::allocator_stats().expect("jemalloc stats should be enabled");
            assert!(stats.allocated_bytes.is_some());
            assert!(stats.active_bytes.is_some());
            assert!(stats.resident_bytes.is_some());
            assert!(stats.mapped_bytes.is_some());
            assert!(stats.retained_bytes.is_some());
        }

        #[cfg(not(feature = "jemalloc-stats"))]
        assert!(ProcessMemoryControl::allocator_stats().is_none());

        assert!(ProcessMemoryControl::try_purge_allocator());
    }

    #[cfg(feature = "mimalloc")]
    #[test]
    fn mimalloc_is_the_selected_allocator() {
        assert_eq!(ProcessMemoryControl::allocator_name(), "mimalloc");
    }

    #[cfg(all(
        not(feature = "mimalloc"),
        any(not(feature = "jemalloc"), target_env = "msvc", target_os = "openbsd")
    ))]
    #[test]
    fn system_allocator_has_no_allocator_controls() {
        assert_eq!(ProcessMemoryControl::allocator_name(), "system");
        assert!(ProcessMemoryControl::allocator_stats().is_none());
        assert!(!ProcessMemoryControl::try_purge_allocator());
    }
}
