//! Memory for syntax trees: tree-sitter allocates from mimalloc, and freed
//! tree memory goes back to the OS when a highlight pass completes.
//!
//! tree-sitter's C core calls libc `malloc` unless told otherwise. glibc keeps
//! the pages of a freed tree, so dropping a 130 MB tree saves nothing
//! (measured in brief 0009). [`install`] points tree-sitter at mimalloc
//! (MIT) instead, on a heap of its own so tree nodes do not share pages
//! with longer-lived objects, whatever global allocator the binary uses, and
//! [`release_free_memory`] asks mimalloc to return free pages, which it
//! otherwise does only when the owning thread allocates again.
//!
//! The rest of the process stays on the system allocator: brief 0011
//! measured mimalloc as the global allocator and it cost 18 to 24 MB more
//! at idle in the viewer (40 MB in the `eludite` shell) for no speed gain.
//!
//! Process-wide policy stays with the binaries: `eludite` and the example
//! viewer call [`disable_transparent_huge_pages`] from their `main`; this
//! library never changes process settings on its own.
//!
//! [`live_bytes`] counts the bytes tree-sitter holds, which measures tree
//! memory independently of RSS and of the allocator's caching.
//!
//! tree-sitter requires the allocator to be installed before its first
//! allocation and never changed while its objects live, so [`install`] runs
//! once per process, from every constructor in this crate that creates a
//! tree-sitter object ([`super::Language::new`], [`super::Highlighter::new`]).
//! No other crate in the workspace uses tree-sitter.

// The C allocator interface is raw pointers by definition.
#![allow(unsafe_code)]

use std::ffi::c_void;
use std::sync::Once;
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};

use libmimalloc_sys as mi;

static LIVE: AtomicUsize = AtomicUsize::new(0);

/// tree-sitter's own mimalloc heap, so tree nodes never share a page with
/// longer-lived objects; set once by [`install`].
static HEAP: AtomicPtr<mi::mi_heap_t> = AtomicPtr::new(std::ptr::null_mut());

fn heap() -> *mut mi::mi_heap_t {
    HEAP.load(Ordering::Relaxed)
}

/// Bytes currently allocated by tree-sitter (usable size of its blocks).
pub fn live_bytes() -> usize {
    LIVE.load(Ordering::Relaxed)
}

/// Return the calling thread's free mimalloc pages, and free pages of
/// mimalloc's shared arenas, to the OS. Costs a few milliseconds after a
/// large tree is dropped; [`super::Highlighter`] calls it on the syntax
/// thread when a highlight pass completes.
pub fn release_free_memory() {
    // SAFETY: `HEAP` is null or a heap that is never deleted.
    unsafe {
        if !heap().is_null() {
            mi::mi_heap_collect(heap(), true);
        }
        mi::mi_collect(true);
    }
}

/// Install the allocator. Idempotent; call before creating any tree-sitter
/// object.
pub(crate) fn install() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // SAFETY: creating a heap has no preconditions; it is never deleted.
        let heap = unsafe { mi::mi_heap_new() };
        assert!(!heap.is_null(), "mimalloc heap for tree-sitter");
        HEAP.store(heap, Ordering::Relaxed);
        // SAFETY: the four functions are mimalloc's malloc family on one
        // heap (mimalloc 3 heaps allocate from any thread), which
        // aligns like libc `malloc`; they abort instead of returning null for
        // a non-zero size. `Once` makes this the only call, made before this
        // crate creates any tree-sitter object, and no other crate uses
        // tree-sitter.
        unsafe {
            tree_sitter::set_allocator(Some(tree_sitter::Allocator {
                malloc: ts_malloc,
                calloc: ts_calloc,
                realloc: ts_realloc,
                free: ts_free,
            }));
        }
    });
}

/// Turn off transparent huge pages for this process, as mimalloc's own
/// `MIMALLOC_ALLOW_THP=0` does.
///
/// Where the kernel's THP mode is `always` (CachyOS and other distributions
/// default to it), every 2 MiB-aligned range of mimalloc's arenas that is
/// touched becomes a huge page, so a tree heap with free gaps keeps far
/// more resident than it uses: brief 0011 measured 14 MB more at idle for
/// the 100k-line C# file (112 MB instead of 98 MB). Under the more common
/// `madvise` mode this changes nothing. It affects pages faulted in after
/// the call, so calling it first thing in `main` is the right place. This is
/// process-wide policy, so only binaries call it; the library never does.
pub fn disable_transparent_huge_pages() {
    #[cfg(target_os = "linux")]
    {
        use std::ffi::{c_int, c_ulong};
        unsafe extern "C" {
            fn prctl(option: c_int, ...) -> c_int;
        }
        const PR_SET_THP_DISABLE: c_int = 41;
        // SAFETY: PR_SET_THP_DISABLE takes four unsigned long arguments and
        // only sets a flag on this process; failure (old kernels) is
        // harmless and ignored.
        unsafe {
            prctl(
                PR_SET_THP_DISABLE,
                1 as c_ulong,
                0 as c_ulong,
                0 as c_ulong,
                0 as c_ulong,
            )
        };
    }
}

/// Count a fresh block, or abort if the allocation failed.
fn counted(ptr: *mut c_void, size: usize) -> *mut c_void {
    if ptr.is_null() {
        if size == 0 {
            return ptr;
        }
        let layout = std::alloc::Layout::from_size_align(size, 16)
            .unwrap_or(std::alloc::Layout::new::<[u8; 16]>());
        std::alloc::handle_alloc_error(layout);
    }
    // SAFETY: `ptr` is a live mimalloc block.
    LIVE.fetch_add(unsafe { mi::mi_usable_size(ptr) }, Ordering::Relaxed);
    ptr
}

unsafe extern "C" fn ts_malloc(size: usize) -> *mut c_void {
    // SAFETY: plain allocation.
    counted(unsafe { mi::mi_heap_malloc(heap(), size) }, size)
}

unsafe extern "C" fn ts_calloc(count: usize, size: usize) -> *mut c_void {
    // SAFETY: plain allocation; mimalloc checks the multiplication.
    counted(
        unsafe { mi::mi_heap_calloc(heap(), count, size) },
        count.saturating_mul(size),
    )
}

unsafe extern "C" fn ts_realloc(ptr: *mut c_void, size: usize) -> *mut c_void {
    if !ptr.is_null() {
        // SAFETY: tree-sitter passes blocks it got from this family.
        LIVE.fetch_sub(unsafe { mi::mi_usable_size(ptr) }, Ordering::Relaxed);
    }
    // SAFETY: `ptr` is null or a live block of this family. On failure the
    // old block is left alone and `counted` aborts.
    counted(unsafe { mi::mi_heap_realloc(heap(), ptr, size) }, size)
}

unsafe extern "C" fn ts_free(ptr: *mut c_void) {
    if ptr.is_null() {
        return;
    }
    // SAFETY: tree-sitter passes blocks it got from this family.
    unsafe {
        LIVE.fetch_sub(mi::mi_usable_size(ptr), Ordering::Relaxed);
        mi::mi_free(ptr);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_round_trip() {
        install();
        // Other tests allocate concurrently, so check the blocks themselves
        // rather than the global counter.
        unsafe {
            let p = ts_calloc(4, 8).cast::<u8>();
            assert_eq!(p as usize % 16, 0, "malloc alignment");
            assert!(mi::mi_usable_size(p.cast()) >= 32);
            assert!((0..32).all(|i| *p.add(i) == 0), "calloc zeroes");
            p.write_bytes(7, 32);
            let q = ts_realloc(p.cast(), 4096).cast::<u8>();
            assert!(mi::mi_usable_size(q.cast()) >= 4096);
            assert!((0..32).all(|i| *q.add(i) == 7), "realloc keeps contents");
            ts_free(q.cast());
            ts_free(std::ptr::null_mut());
            let r = ts_realloc(std::ptr::null_mut(), 8);
            assert!(!r.is_null());
            ts_free(r);
        }
        release_free_memory();
    }
}
