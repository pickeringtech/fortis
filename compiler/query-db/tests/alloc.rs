//! The daemon's allocator (fortis-query-db `alloc`, X-141) as this test binary's global allocator: every layout
//! path, its alignment and zeroing, `realloc` across classes and paths, frees on another thread, and the counters
//! returning exactly to where they were. The counters are exact only while one thread allocates, so the checks run
//! in order inside one test.

use std::alloc::{GlobalAlloc, Layout};

use fortis_query_db::alloc::{stats, Classes};

#[global_allocator]
static A: Classes = Classes;

fn counts() -> (i64, i64, i64) {
    let s = stats();
    (s.live_bytes, s.live_allocs, s.in_use)
}

fn every_path_aligns_zeroes_and_balances() {
    let before = counts();
    let layouts = [(1, 1), (16, 8), (17, 16), (255, 8), (256, 16), (257, 8), (4000, 8), (32 << 10, 16), (256 << 10, 16), ((256 << 10) + 1, 8), (3 << 20, 8), (64, 64), (100, 4096), (8192, 8192), (20000, 1 << 16)];
    let mut held = vec![];
    for &(size, align) in &layouts {
        let l = Layout::from_size_align(size, align).unwrap();
        unsafe {
            let p = A.alloc_zeroed(l);
            assert!(!p.is_null(), "{size}/{align}");
            assert_eq!(p as usize % align, 0, "{size}/{align} misaligned");
            assert!(std::slice::from_raw_parts(p, size).iter().all(|&b| b == 0), "{size}/{align} not zeroed");
            std::ptr::write_bytes(p, 0xA5, size);
            held.push((p, l));
        }
    }
    // reused blocks are zeroed too
    for (p, l) in held.drain(..) {
        unsafe { A.dealloc(p, l) };
    }
    drop(held);
    for &(size, align) in &layouts {
        let l = Layout::from_size_align(size, align).unwrap();
        unsafe {
            let p = A.alloc_zeroed(l);
            assert!(std::slice::from_raw_parts(p, size).iter().all(|&b| b == 0), "{size}/{align} reused, not zeroed");
            A.dealloc(p, l);
        }
    }
    assert_eq!(counts(), before);
}

fn realloc_keeps_contents_across_classes_and_paths() {
    let before = counts();
    for align in [1usize, 8, 16, 64, 8192] {
        let mut l = Layout::from_size_align(8, align).unwrap();
        let mut p = unsafe { A.alloc(l) };
        unsafe { std::ptr::write_bytes(p, 7, 8) };
        // grow through small classes, into a mapping, through mremap, and back down
        for n in [12usize, 300, 5000, 300 << 10, 2 << 20, 9 << 20, 600, 9] {
            let q = unsafe { A.realloc(p, l, n) };
            assert!(!q.is_null());
            assert_eq!(q as usize % align, 0, "realloc to {n} at align {align}");
            assert!(unsafe { std::slice::from_raw_parts(q, 8.min(n)) }.iter().all(|&b| b == 7), "contents lost at {n}");
            p = q;
            l = Layout::from_size_align(n, align).unwrap();
        }
        unsafe { A.dealloc(p, l) };
    }
    assert_eq!(counts(), before);
}

fn a_block_freed_on_another_thread_is_still_counted() {
    let before = counts();
    let v: Vec<Vec<u8>> = (0..1000).map(|i| vec![i as u8; i % 700 + 1]).collect();
    std::thread::spawn(move || drop(v)).join().unwrap();
    // the thread's own bookkeeping (its stack and name) is freed when it is joined
    let after = counts();
    assert_eq!((after.0, after.1), (before.0, before.1));
}

fn collections_use_it() {
    let mut m = std::collections::HashMap::new();
    for i in 0..100_000u64 {
        m.insert(i, i.to_string());
    }
    assert_eq!(m.len(), 100_000);
    assert!(stats().live_allocs >= 100_000);
}

#[test]
fn allocator() {
    every_path_aligns_zeroes_and_balances();
    realloc_keeps_contents_across_classes_and_paths();
    a_block_freed_on_another_thread_is_still_counted();
    collections_use_it();
}
