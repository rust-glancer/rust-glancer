use std::{ffi::CStr, ptr};

#[cfg(feature = "jemalloc-stats")]
use std::mem;

use rg_lsp_engine::AllocatorStats;

pub(super) const NAME: &str = "jemalloc";

#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

pub(super) fn capture_stats() -> Option<AllocatorStats> {
    #[cfg(feature = "jemalloc-stats")]
    {
        advance_epoch()?;

        Some(AllocatorStats {
            allocated_bytes: Some(read_usize(c"stats.allocated")?),
            active_bytes: Some(read_usize(c"stats.active")?),
            resident_bytes: Some(read_usize(c"stats.resident")?),
            mapped_bytes: Some(read_usize(c"stats.mapped")?),
            retained_bytes: Some(read_usize(c"stats.retained")?),
        })
    }

    #[cfg(not(feature = "jemalloc-stats"))]
    {
        None
    }
}

pub(super) fn try_purge() -> bool {
    // The engine builds analysis on a dedicated thread, so flushing this thread's tcache first
    // makes recently freed indexing allocations visible to the arena purge below. A missing
    // thread cache does not prevent the process-wide arena purge from doing useful work.
    mallctl_void(c"thread.tcache.flush");

    // 4096 is jemalloc's documented MALLCTL_ARENAS_ALL constant. It lets one mallctl target all
    // arenas instead of discovering and iterating arena indexes manually.
    mallctl_void(c"arena.4096.purge")
}

#[cfg(feature = "jemalloc-stats")]
fn advance_epoch() -> Option<()> {
    let mut epoch = 1_u64;
    let rc = unsafe {
        tikv_jemalloc_sys::mallctl(
            c"epoch".as_ptr(),
            ptr::null_mut(),
            ptr::null_mut(),
            (&mut epoch as *mut u64).cast(),
            mem::size_of_val(&epoch),
        )
    };
    (rc == 0).then_some(())
}

#[cfg(feature = "jemalloc-stats")]
fn read_usize(name: &'static CStr) -> Option<usize> {
    let mut value = 0_usize;
    let mut value_len = mem::size_of_val(&value);
    let rc = unsafe {
        tikv_jemalloc_sys::mallctl(
            name.as_ptr(),
            (&mut value as *mut usize).cast(),
            &mut value_len,
            ptr::null_mut(),
            0,
        )
    };
    (rc == 0).then_some(value)
}

fn mallctl_void(name: &'static CStr) -> bool {
    let rc = unsafe {
        tikv_jemalloc_sys::mallctl(
            name.as_ptr(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            0,
        )
    };
    rc == 0
}
