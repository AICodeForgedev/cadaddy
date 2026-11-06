//! A counting allocator for tests that must tell memory held from memory the allocator keeps.
//!
//! A process's resident size follows the allocator's free lists as much as what the program
//! holds, so it can't say whether a closed drawing was let go. A test binary that installs
//! [`Counting`] as its global allocator reads [`Counting::live`] instead: the bytes allocated and
//! not yet freed, exactly.
//!
//! ```ignore
//! #[global_allocator]
//! static ALLOC: cadaddy_memcount::Counting = cadaddy_memcount::Counting::new();
//! ```

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

/// The system allocator, counting the bytes it has handed out and not got back.
pub struct Counting {
    live: AtomicUsize,
}

impl Counting {
    pub const fn new() -> Self {
        Counting { live: AtomicUsize::new(0) }
    }

    /// Bytes allocated and not yet freed, by every thread of the program.
    pub fn live(&self) -> usize {
        self.live.load(Ordering::Relaxed)
    }
}

impl Default for Counting {
    fn default() -> Self {
        Self::new()
    }
}

// SAFETY: every call is forwarded unchanged to `System`, which upholds the `GlobalAlloc`
// contract; the counter is only read and written atomically and never affects the pointers.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller's contract for `alloc` is `System`'s.
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            self.live.fetch_add(layout.size(), Ordering::Relaxed);
        }
        p
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller's contract for `alloc_zeroed` is `System`'s.
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() {
            self.live.fetch_add(layout.size(), Ordering::Relaxed);
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        self.live.fetch_sub(layout.size(), Ordering::Relaxed);
        // SAFETY: `ptr` and `layout` are the caller's, from an earlier `alloc` of this allocator.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: `ptr` and `layout` are the caller's, from an earlier `alloc` of this allocator.
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if !p.is_null() {
            if new_size >= layout.size() {
                self.live.fetch_add(new_size - layout.size(), Ordering::Relaxed);
            } else {
                self.live.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
            }
        }
        p
    }
}
