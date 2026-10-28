//! The daemon's allocator (X-141): size classes, so that what it holds for the live allocations is a function of
//! their sizes, whatever order of requests made them.
//!
//! - A request of at most 256 KiB, aligned to at most 16 bytes, gets a block of its size class: 16-byte steps up to
//!   256 bytes, then four classes to each doubling. A class carves its blocks from runs of at least 64 KiB, cut
//!   from 4 MiB chunks mapped from the system, and keeps the blocks it gets back on a free list. Runs and chunks are
//!   never returned, so the footprint grows only when a class's live blocks exceed every earlier peak.
//! - A larger request, or one aligned to more than 16 bytes, is mapped on its own, rounded up to the system page; one
//!   aligned to more than a page is mapped with room to align it, and the room is unmapped again.
//!
//! glibc instead hands out chunks whose size depends on the heap's history (SP-12: identical requests took more
//! bytes in a fragmented heap). Here the bytes in use are the sum of the live requests' classes, exactly.
//!
//! Each thread has its own free lists and runs, reached through a thread-local with a constant initialiser and no
//! destructor, so the allocator takes no lock. A block freed by another thread joins that thread's lists. The
//! counters are read and written without a locked read-modify-write: exact in the single-threaded daemon,
//! approximate (never undefined) in a process with several allocating threads.

use std::alloc::{GlobalAlloc, Layout};
use std::cell::UnsafeCell;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, AtomicI64};
use std::sync::atomic::Ordering::Relaxed;

const SMALL: usize = 256 << 10;
const RUN: usize = 64 << 10;
const CHUNK: usize = 4 << 20;
const OS_PAGE: usize = 4096;
/// 16 classes to 256 bytes, then four to each doubling up to 256 KiB (10 doublings)
const NCLASS: usize = 16 + 10 * 4;

extern "C" {
    fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
    fn munmap(addr: *mut u8, len: usize) -> i32;
    fn mremap(old: *mut u8, old_len: usize, new_len: usize, flags: i32, ...) -> *mut u8;
}
const PROT_READ_WRITE: i32 = 0x1 | 0x2;
const MAP_PRIVATE_ANONYMOUS: i32 = 0x02 | 0x20;
const MREMAP_MAYMOVE: i32 = 1;
const MAP_FAILED: *mut u8 = !0usize as *mut u8;

/// The class of a small request (1 ≤ size ≤ SMALL).
fn class_of(size: usize) -> usize {
    if size <= 256 {
        return (size + 15) / 16 - 1;
    }
    // 2^p < size ≤ 2^(p+1), p ≥ 8; four classes in each such range
    let p = (usize::BITS - 1 - (size - 1).leading_zeros()) as usize;
    16 + (p - 8) * 4 + (((size - 1) >> (p - 2)) & 3)
}

/// The block size of a class.
fn class_size(c: usize) -> usize {
    if c < 16 {
        return (c + 1) * 16;
    }
    let (p, sub) = (8 + (c - 16) / 4, (c - 16) % 4);
    (1 << p) + (sub + 1) * (1 << (p - 2))
}

fn round_up(n: usize, to: usize) -> usize {
    (n + to - 1) / to * to
}

struct Heap {
    free: [*mut u8; NCLASS],
    cursor: [*mut u8; NCLASS],
    end: [*mut u8; NCLASS],
    chunk: *mut u8,
    chunk_end: *mut u8,
    dropped: u64,
}

thread_local! {
    static HEAP: UnsafeCell<Heap> = const {
        UnsafeCell::new(Heap { free: [null_mut(); NCLASS], cursor: [null_mut(); NCLASS], end: [null_mut(); NCLASS], chunk: null_mut(), chunk_end: null_mut(), dropped: 0 })
    };
}

static LIVE_BYTES: AtomicI64 = AtomicI64::new(0);
static LIVE_ALLOCS: AtomicI64 = AtomicI64::new(0);
static IN_USE: AtomicI64 = AtomicI64::new(0);
static CARVED: AtomicI64 = AtomicI64::new(0);
static MAPPED: AtomicI64 = AtomicI64::new(0);
/// Fault support (SP-12b's `free-leak`): set once at start by a fault build, never by a default one.
static FREE_LEAK: AtomicBool = AtomicBool::new(false);

/// Fault support: from here on, one returned block in 64 is never reused, so the footprint grows.
pub fn leak_frees() {
    FREE_LEAK.store(true, Relaxed);
}

/// Fault support (SP-12b's `system-mix`): set once at start by a fault build, never by a default one.
static SYSTEM_MIX: AtomicBool = AtomicBool::new(false);

/// Fault support: from here on, a small request whose class has no free block goes to the system allocator and is
/// charged what that allocator gives it (`malloc_usable_size`), which depends on its heap's history: SP-12's glibc,
/// for the requests a class cannot serve from its free list.
pub fn mix_system() {
    SYSTEM_MIX.store(true, Relaxed);
}

/// The chunks this allocator mapped (the first 2^14, 64 GiB), so that under `system-mix` a freed small block can be
/// told from a system one.
const MAX_CHUNKS: usize = 1 << 14;
static CHUNKS: [AtomicI64; MAX_CHUNKS] = [const { AtomicI64::new(0) }; MAX_CHUNKS];
static NCHUNKS: AtomicI64 = AtomicI64::new(0);

fn ours(p: *mut u8) -> bool {
    let n = NCHUNKS.load(Relaxed) as usize;
    CHUNKS[..n.min(MAX_CHUNKS)].iter().any(|c| {
        let base = c.load(Relaxed) as usize;
        base <= p as usize && (p as usize) < base + CHUNK
    })
}

extern "C" {
    fn malloc_usable_size(p: *mut u8) -> usize;
}

fn add(c: &AtomicI64, n: i64) {
    c.store(c.load(Relaxed) + n, Relaxed);
}

/// What the allocator holds:
/// - the live bytes requested, and the live allocations;
/// - in use: the bytes their blocks take (the sum of their classes, and of the mapped requests' pages);
/// - the footprint: every block ever carved from a run, free or not, and every mapped request; it grows only when a
///   class needs a block its free list does not have, so it is each class's peak;
/// - mapped: every chunk and mapped request taken from the system (the footprint plus the chunks' uncarved rest).
pub fn stats() -> Stats {
    Stats { live_bytes: LIVE_BYTES.load(Relaxed), live_allocs: LIVE_ALLOCS.load(Relaxed), in_use: IN_USE.load(Relaxed), footprint: CARVED.load(Relaxed), mapped: MAPPED.load(Relaxed) }
}

pub struct Stats {
    pub live_bytes: i64,
    pub live_allocs: i64,
    pub in_use: i64,
    pub footprint: i64,
    pub mapped: i64,
}

/// A mapping of `len` bytes (a multiple of the page) aligned to `align`: a page-aligned one is what the system
/// gives; a larger alignment maps `align` more and unmaps the head and the tail around the aligned region.
unsafe fn map_aligned(len: usize, align: usize) -> *mut u8 {
    if align <= OS_PAGE {
        return map(len);
    }
    let p = mmap(null_mut(), len + align, PROT_READ_WRITE, MAP_PRIVATE_ANONYMOUS, -1, 0);
    if p == MAP_FAILED {
        return null_mut();
    }
    let start = round_up(p as usize, align);
    let (head, tail) = (start - p as usize, p as usize + len + align - (start + len));
    if head > 0 {
        munmap(p, head);
    }
    if tail > 0 {
        munmap((start + len) as *mut u8, tail);
    }
    add(&MAPPED, len as i64);
    start as *mut u8
}

unsafe fn map(len: usize) -> *mut u8 {
    let p = mmap(null_mut(), len, PROT_READ_WRITE, MAP_PRIVATE_ANONYMOUS, -1, 0);
    if p == MAP_FAILED {
        return null_mut();
    }
    add(&MAPPED, len as i64);
    p
}

impl Heap {
    /// A block of class `c`: from its free list, else from its run, else from a new run.
    unsafe fn take(&mut self, c: usize) -> *mut u8 {
        let b = self.free[c];
        if !b.is_null() {
            self.free[c] = *(b as *mut *mut u8);
            return b;
        }
        let size = class_size(c);
        if self.cursor[c].is_null() || (self.end[c] as usize) - (self.cursor[c] as usize) < size {
            let run = RUN.max(4 * size);
            if self.chunk.is_null() || (self.chunk_end as usize) - (self.chunk as usize) < run {
                // the rest of the old chunk stays unused: it is in the footprint
                let len = CHUNK.max(run);
                let p = map(len);
                if p.is_null() {
                    return null_mut();
                }
                // every chunk is recorded, from the start, so that `system-mix` can tell a block's origin
                let n = NCHUNKS.load(Relaxed) as usize;
                if n < MAX_CHUNKS {
                    CHUNKS[n].store(p as i64, Relaxed);
                    NCHUNKS.store(n as i64 + 1, Relaxed);
                }
                self.chunk = p;
                self.chunk_end = p.add(len);
            }
            self.cursor[c] = self.chunk;
            self.end[c] = self.chunk.add(run);
            self.chunk = self.chunk.add(run);
        }
        let b = self.cursor[c];
        self.cursor[c] = b.add(size);
        add(&CARVED, size as i64);
        b
    }

    unsafe fn give(&mut self, c: usize, b: *mut u8) {
        // fault `free-leak` (SP-12b): one returned block in 64 is never reused
        if FREE_LEAK.load(Relaxed) {
            self.dropped += 1;
            if self.dropped % 64 == 0 {
                return;
            }
        }
        *(b as *mut *mut u8) = self.free[c];
        self.free[c] = b;
    }
}

/// Fault support (SP-12b's `leak-uncounted`): a block of `size` bytes taken from this thread's heap and never given
/// back, outside every count but the footprint, which grows as the block's class fills its runs.
pub fn leak_uncounted(size: usize) {
    // SAFETY: the block is taken like any other and simply never returned
    let p = HEAP.with(|h| unsafe { (*h.get()).take(class_of(size.clamp(1, SMALL))) });
    std::hint::black_box(p);
}

fn small(l: &Layout) -> bool {
    l.size() <= SMALL && l.align() <= 16
}

/// The global allocator of `fortisd`.
pub struct Classes;

// SAFETY: a small block is handed to one caller until it is given back; its class is derived from the layout the
// caller must pass back (GlobalAlloc's contract), so a block returns to the list it came from. Mapped requests are
// unmapped with the length they were mapped with, which the same layout gives; one aligned to more than a page keeps
// exactly that length after its head and tail are unmapped.
unsafe impl GlobalAlloc for Classes {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let size = l.size().max(1);
        let p = if small(&l) {
            let c = class_of(size);
            if SYSTEM_MIX.load(Relaxed) && HEAP.with(|h| (*h.get()).free[c].is_null()) {
                let p = std::alloc::System.alloc(l);
                if !p.is_null() {
                    add(&IN_USE, malloc_usable_size(p) as i64);
                    add(&LIVE_BYTES, l.size() as i64);
                    add(&LIVE_ALLOCS, 1);
                }
                return p;
            }
            let p = HEAP.with(|h| (*h.get()).take(c));
            if !p.is_null() {
                add(&IN_USE, class_size(c) as i64);
            }
            p
        } else {
            let len = round_up(size, OS_PAGE);
            let p = map_aligned(len, l.align());
            if !p.is_null() {
                add(&IN_USE, len as i64);
                add(&CARVED, len as i64);
            }
            p
        };
        if !p.is_null() {
            add(&LIVE_BYTES, l.size() as i64);
            add(&LIVE_ALLOCS, 1);
        }
        p
    }

    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        let p = self.alloc(l);
        // a fresh mapping is zero already; a reused block is not
        if !p.is_null() && small(&l) {
            std::ptr::write_bytes(p, 0, l.size());
        }
        p
    }

    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        let size = l.size().max(1);
        if small(&l) && SYSTEM_MIX.load(Relaxed) && !ours(p) {
            add(&IN_USE, -(malloc_usable_size(p) as i64));
            std::alloc::System.dealloc(p, l);
        } else if small(&l) {
            let c = class_of(size);
            HEAP.with(|h| (*h.get()).give(c, p));
            add(&IN_USE, -(class_size(c) as i64));
        } else {
            let len = round_up(size, OS_PAGE);
            munmap(p, len);
            add(&IN_USE, -(len as i64));
            add(&CARVED, -(len as i64));
            add(&MAPPED, -(len as i64));
        }
        add(&LIVE_BYTES, -(l.size() as i64));
        add(&LIVE_ALLOCS, -1);
    }

    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        let new = Layout::from_size_align_unchecked(n, l.align());
        let (old_small, new_small) = (small(&l), small(&new));
        // the same class: the block already fits
        if old_small && new_small && class_of(l.size().max(1)) == class_of(n.max(1)) && !(SYSTEM_MIX.load(Relaxed) && !ours(p)) {
            add(&LIVE_BYTES, n as i64 - l.size() as i64);
            return p;
        }
        // mapped to mapped: the kernel moves the pages (a move keeps page alignment only, so not for larger ones)
        if !old_small && !new_small && l.align() <= OS_PAGE {
            let (a, b) = (round_up(l.size().max(1), OS_PAGE), round_up(n.max(1), OS_PAGE));
            let q = mremap(p, a, b, MREMAP_MAYMOVE);
            if q == MAP_FAILED {
                return null_mut();
            }
            add(&IN_USE, b as i64 - a as i64);
            add(&CARVED, b as i64 - a as i64);
            add(&MAPPED, b as i64 - a as i64);
            add(&LIVE_BYTES, n as i64 - l.size() as i64);
            return q;
        }
        let q = self.alloc(new);
        if !q.is_null() {
            std::ptr::copy_nonoverlapping(p, q, l.size().min(n));
            self.dealloc(p, l);
        }
        q
    }
}

#[cfg(test)]
mod tests {
    use super::{class_of, class_size, NCLASS, SMALL};

    #[test]
    fn every_small_size_has_the_smallest_class_that_holds_it() {
        let mut last = 0;
        for size in 1..=SMALL {
            let c = class_of(size);
            assert!(c < NCLASS, "{size}");
            assert!(class_size(c) >= size, "{size} in {}", class_size(c));
            assert!(c == 0 || class_size(c - 1) < size, "{size}: class {} would hold it", class_size(c - 1));
            assert!(c >= last);
            last = c;
            assert_eq!(class_size(c) % 16, 0);
        }
        assert_eq!(class_size(class_of(SMALL)), SMALL);
    }
}
