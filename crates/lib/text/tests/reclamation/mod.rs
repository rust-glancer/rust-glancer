//! Check that dropping Name values frees the memory used for their strings.
//!
//! This test counts bytes allocated through Rust's global allocator. It lives in a separate test
//! binary so allocations made by other tests cannot affect the before/after counts.

use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::atomic::{AtomicUsize, Ordering},
};

use rg_text::Name;

static LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);

struct CountingAllocator;

// Let System do the actual allocation and freeing, and track how many bytes remain allocated.
// Only count an allocation or resize after it succeeds; a failed resize leaves the old block live.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            LIVE_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        LIVE_BYTES.fetch_sub(layout.size(), Ordering::Relaxed);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let result = unsafe { System.realloc(ptr, layout, size) };
        if !result.is_null() {
            LIVE_BYTES.fetch_add(size, Ordering::Relaxed);
            LIVE_BYTES.fetch_sub(layout.size(), Ordering::Relaxed);
        }
        result
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

// The global name pool must release strings once no `Name` owns them, rather than retaining
// every spelling seen by earlier queries. These names are long enough to need heap storage.
// We create one set of handles from strings and decode another set from bytes, then drop both.
//
// The input strings and encoded bytes stay alive throughout the measurement. Returning to the
// allocation count from before either set existed means the vectors and their name storage were
// freed. We check live allocated bytes here, not whether the allocator returns pages to the OS.
#[test]
fn dropping_constructed_and_decoded_values_reclaims_dynamic_names() {
    let spellings = (0..128)
        .map(|index| format!("ReclaimableQueryName_{index:04}"))
        .collect::<Vec<_>>();
    let bytes = wincode::serialize(&spellings).expect("name spellings should serialize");
    // Create and drop one long name first so the pool is initialized before taking the baseline.
    // Any allocations needed to set up the pool are then already included in that baseline.
    drop(Name::new(&spellings[0]));
    let before = LIVE_BYTES.load(Ordering::Relaxed);
    let constructed = spellings.iter().map(Name::new).collect::<Vec<_>>();
    let decoded =
        wincode::deserialize_exact::<Vec<Name>>(&bytes).expect("name values should deserialize");
    let while_alive = LIVE_BYTES.load(Ordering::Relaxed);
    drop((constructed, decoded));
    let after = LIVE_BYTES.load(Ordering::Relaxed);

    assert!(while_alive >= before + spellings.iter().map(String::len).sum::<usize>());
    assert_eq!(
        after, before,
        "unused dynamic name storage should be released"
    );
}
