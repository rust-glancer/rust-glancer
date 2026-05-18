//! Process memory controls for the CLI/LSP binary.
//!
//! Allocator choice is intentionally kept at the executable boundary. The analysis engine remains
//! allocator-agnostic, while release builds can still compare jemalloc and the platform allocator
//! by toggling the `jemalloc` Cargo feature.

use std::sync::Arc;

use rg_lsp_engine::{AllocatorPurgeResult, AllocatorStats, MemoryControl};
use rg_project::{ProjectMemoryHooks, ProjectMemoryPurgePoint};

const JEMALLOC_PURGE_AFTER_BUILD_ENV: &str = "RUST_GLANCER_PURGE_MEMORY_AFTER_BUILD";

#[cfg(all(feature = "jemalloc", not(target_env = "msvc")))]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[derive(Debug, Clone, Copy)]
pub(crate) struct ProcessMemoryControl;

impl MemoryControl for ProcessMemoryControl {
    fn allocator_name(&self) -> &'static str {
        Self::allocator_name()
    }

    fn allocator_purge_enabled(&self) -> bool {
        Self::allocator_purge_enabled()
    }

    fn allocator_stats(&self) -> Option<AllocatorStats> {
        Self::allocator_stats()
    }

    fn try_purge_allocator(&self) -> Option<AllocatorPurgeResult> {
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
        let _ = self.memory_control.try_purge_allocator();
    }
}

impl ProcessMemoryControl {
    pub(crate) fn allocator_name() -> &'static str {
        if cfg!(all(feature = "jemalloc", not(target_env = "msvc"))) {
            "jemalloc"
        } else {
            "system"
        }
    }

    pub(crate) fn allocator_stats() -> Option<AllocatorStats> {
        #[cfg(all(feature = "jemalloc-stats", not(target_env = "msvc")))]
        {
            jemalloc_stats::capture()
        }

        #[cfg(not(all(feature = "jemalloc-stats", not(target_env = "msvc"))))]
        {
            None
        }
    }

    fn allocator_purge_enabled() -> bool {
        std::env::var(JEMALLOC_PURGE_AFTER_BUILD_ENV)
            .ok()
            .map(|value| matches!(value.as_str(), "1" | "true" | "yes" | "on"))
            .unwrap_or(true) // Enabled by default
    }

    pub(crate) fn try_purge_allocator() -> Option<AllocatorPurgeResult> {
        if !Self::allocator_purge_enabled() {
            return None;
        }

        #[cfg(all(feature = "jemalloc-stats", not(target_env = "msvc")))]
        {
            Some(jemalloc_stats::purge())
        }

        #[cfg(not(all(feature = "jemalloc-stats", not(target_env = "msvc"))))]
        {
            None
        }
    }
}

#[cfg(all(feature = "jemalloc-stats", not(target_env = "msvc")))]
mod jemalloc_stats {
    use std::{ffi::CString, mem, ptr};

    use rg_lsp_engine::AllocatorStats;

    pub(super) fn capture() -> Option<AllocatorStats> {
        advance_epoch()?;

        Some(AllocatorStats {
            allocated_bytes: read_usize("stats.allocated")?,
            active_bytes: read_usize("stats.active")?,
            resident_bytes: read_usize("stats.resident")?,
            mapped_bytes: read_usize("stats.mapped")?,
            retained_bytes: read_usize("stats.retained")?,
        })
    }

    fn advance_epoch() -> Option<()> {
        let mut epoch = 1_u64;
        let rc = unsafe {
            tikv_jemalloc_sys::mallctl(
                c_name("epoch").as_ptr(),
                ptr::null_mut(),
                ptr::null_mut(),
                (&mut epoch as *mut u64).cast(),
                mem::size_of_val(&epoch),
            )
        };
        (rc == 0).then_some(())
    }

    fn read_usize(name: &'static str) -> Option<usize> {
        let mut value = 0_usize;
        let mut value_len = mem::size_of_val(&value);
        let rc = unsafe {
            tikv_jemalloc_sys::mallctl(
                c_name(name).as_ptr(),
                (&mut value as *mut usize).cast(),
                &mut value_len,
                ptr::null_mut(),
                0,
            )
        };
        (rc == 0).then_some(value)
    }

    fn c_name(name: &'static str) -> CString {
        CString::new(name).expect("jemalloc mallctl name should not contain NUL")
    }

    pub(super) fn purge() -> rg_lsp_engine::AllocatorPurgeResult {
        // The engine builds analysis on a dedicated thread, so flushing this thread's tcache first
        // makes recently freed indexing allocations visible to the arena purge below.
        let tcache_flushed = mallctl_void("thread.tcache.flush");

        // 4096 is jemalloc's documented MALLCTL_ARENAS_ALL constant. It lets one mallctl target all
        // arenas instead of discovering and iterating arena indexes manually.
        let arenas_purged = mallctl_void("arena.4096.purge");

        rg_lsp_engine::AllocatorPurgeResult {
            tcache_flushed,
            arenas_purged,
        }
    }

    fn mallctl_void(name: &'static str) -> bool {
        let rc = unsafe {
            tikv_jemalloc_sys::mallctl(
                c_name(name).as_ptr(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                0,
            )
        };
        rc == 0
    }
}
