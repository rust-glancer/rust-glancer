use std::{ptr, sync::Arc};

use bumpalo::Bump;

/// Owns the bump allocation backing one immutable green tree.
///
/// The arena is intentionally private: after construction, rowan only hands out
/// immutable green handles. That lets us use `Bump` for storage while keeping
/// the public `GreenNode` and `GreenToken` handles `Send + Sync`.
#[derive(Debug)]
pub(crate) struct GreenArena {
    bump: Bump,
}

// `Bump` is not `Sync` because allocation mutates its bump pointer through
// interior state. This wrapper is only mutated while a tree is being built,
// before any handle can be shared with other threads. Once published, the arena
// is read-only and the private API never allocates into an existing tree.
unsafe impl Send for GreenArena {}
unsafe impl Sync for GreenArena {}

impl GreenArena {
    pub(crate) fn new() -> Arc<GreenArena> {
        Arc::new(GreenArena { bump: Bump::new() })
    }

    pub(crate) fn raw(arena: &Arc<GreenArena>) -> ptr::NonNull<GreenArena> {
        ptr::NonNull::from(Arc::as_ref(arena))
    }

    pub(crate) unsafe fn clone_from_raw(raw: ptr::NonNull<GreenArena>) -> Arc<GreenArena> {
        let raw = raw.as_ptr();
        Arc::increment_strong_count(raw);
        Arc::from_raw(raw)
    }

    pub(crate) fn alloc<T>(&self, value: T) -> ptr::NonNull<T> {
        ptr::NonNull::from(self.bump.alloc(value))
    }

    pub(crate) fn alloc_slice_copy<T: Copy>(&self, src: &[T]) -> &'static [T] {
        let slice = self.bump.alloc_slice_copy(src);
        unsafe { std::mem::transmute::<&[T], &'static [T]>(&*slice) }
    }

    pub(crate) fn alloc_str(&self, src: &str) -> &'static str {
        let text = self.bump.alloc_str(src);
        unsafe { std::mem::transmute::<&str, &'static str>(&*text) }
    }
}
