use std::{
    alloc::{GlobalAlloc, Layout},
    sync::Once,
};

#[cfg(feature = "mimalloc-stats")]
use std::ptr;

use rg_lsp_engine::AllocatorStats;

pub(super) const NAME: &str = "mimalloc";

// `mi_option_disallow_arena_alloc` is an experimental option and is deliberately absent from the
// Rust bindings. The dependency is pinned because this index belongs to mimalloc v3.3.2's option
// enum. A version check below makes a future upgrade fail loudly in tests instead of silently
// changing allocator behavior.
#[cfg(test)]
const MIMALLOC_VERSION: i32 = 30_302;
const DISALLOW_ARENA_ALLOC_OPTION: libmimalloc_sys::mi_option_t = 26;

struct ArenaFreeMiMalloc;

impl ArenaFreeMiMalloc {
    #[inline]
    fn configure() {
        static CONFIGURE_ONCE: Once = Once::new();

        // The option API is not thread-safe, so all allocator entry points pass through this once
        // barrier. The winning thread sets the process policy before the first global allocation
        // reaches mimalloc, and the other threads cannot allocate through mimalloc until that write
        // is complete. Keep this closure allocation-free: it executes from `GlobalAlloc`, where
        // re-entry would wait on the same barrier.
        CONFIGURE_ONCE.call_once(|| unsafe {
            libmimalloc_sys::mi_option_set_enabled(DISALLOW_ARENA_ALLOC_OPTION, true);
        });
    }
}

unsafe impl GlobalAlloc for ArenaFreeMiMalloc {
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        Self::configure();
        unsafe { GlobalAlloc::alloc(&::mimalloc::MiMalloc, layout) }
    }

    #[inline]
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        Self::configure();
        unsafe { GlobalAlloc::alloc_zeroed(&::mimalloc::MiMalloc, layout) }
    }

    #[inline]
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        Self::configure();
        unsafe { GlobalAlloc::dealloc(&::mimalloc::MiMalloc, ptr, layout) };
    }

    #[inline]
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        Self::configure();
        unsafe { GlobalAlloc::realloc(&::mimalloc::MiMalloc, ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: ArenaFreeMiMalloc = ArenaFreeMiMalloc;

pub(super) fn capture_stats() -> Option<AllocatorStats> {
    #[cfg(feature = "mimalloc-stats")]
    {
        let mut current_rss = 0_usize;
        let mut current_commit = 0_usize;
        unsafe {
            libmimalloc_sys::mi_process_info(
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                &mut current_rss,
                ptr::null_mut(),
                &mut current_commit,
                ptr::null_mut(),
                ptr::null_mut(),
            );
        }

        // Mimalloc maintains these process counters in optimized builds without enabling its full
        // debug instrumentation. `current_commit` is the closest counterpart to jemalloc's active
        // pages, while RSS is process-wide on platforms where mimalloc can ask the OS directly.
        //
        // TODO: Populate live allocation and mapped-address counters if mimalloc's Rust bindings
        // expose normal `MI_STAT` instrumentation separately from the expensive debug mode.
        Some(AllocatorStats {
            allocated_bytes: None,
            active_bytes: Some(current_commit),
            resident_bytes: Some(current_rss),
            mapped_bytes: None,
            retained_bytes: None,
        })
    }

    #[cfg(not(feature = "mimalloc-stats"))]
    {
        None
    }
}

pub(super) fn try_purge() -> bool {
    // Arena-free mimalloc releases unused OS allocations as its pages become empty. Its forced
    // collection API only collects the calling thread's heap, and measurements showed no idle-RSS
    // benefit from invoking it at project checkpoints.
    false
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "mimalloc-stats")]
    use super::capture_stats;
    use super::{ArenaFreeMiMalloc, DISALLOW_ARENA_ALLOC_OPTION, MIMALLOC_VERSION, try_purge};

    #[test]
    fn allocator_disables_arenas_before_allocating() {
        ArenaFreeMiMalloc::configure();

        let version = unsafe { libmimalloc_sys::mi_version() };
        assert_eq!(
            version, MIMALLOC_VERSION,
            "review the disallow-arena option index before upgrading mimalloc"
        );
        assert!(unsafe { libmimalloc_sys::mi_option_is_enabled(DISALLOW_ARENA_ALLOC_OPTION) });
    }

    #[test]
    fn explicit_purge_is_unavailable_for_mimalloc() {
        assert!(!try_purge());
    }

    #[cfg(feature = "mimalloc-stats")]
    #[test]
    fn optimized_stats_expose_only_counters_mimalloc_maintains_cheaply() {
        let stats = capture_stats().expect("mimalloc stats should be enabled");

        assert_eq!(stats.allocated_bytes, None);
        assert!(stats.active_bytes.is_some());
        assert!(stats.resident_bytes.is_some());
        assert_eq!(stats.mapped_bytes, None);
        assert_eq!(stats.retained_bytes, None);
    }
}
