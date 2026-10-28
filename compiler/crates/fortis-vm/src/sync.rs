//! Thread-safe stand-ins for `Rc`, `RefCell` and `Cell`, so VM values can move between task threads.
//!
//! The VM runs each Fortis task on its own OS thread and passes one baton between them, as the definitional
//! interpreter's executor does (interpreter/executor.py). Only the thread holding the baton runs, so these
//! locks are never contended. `RefCell` keeps `std::cell::RefCell`'s contract: a conflicting borrow panics
//! instead of blocking, so a bug shows up exactly where it would single-threaded and never deadlocks.

use std::fmt;
use std::sync::{Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard, TryLockError};

pub use std::sync::Arc as Rc;

pub struct RefCell<T: ?Sized>(RwLock<T>);

impl<T> RefCell<T> {
    pub const fn new(v: T) -> RefCell<T> {
        RefCell(RwLock::new(v))
    }
}

impl<T: ?Sized> RefCell<T> {
    pub fn borrow(&self) -> RwLockReadGuard<'_, T> {
        match self.0.try_read() {
            Ok(g) => g,
            Err(TryLockError::Poisoned(p)) => p.into_inner(),
            Err(TryLockError::WouldBlock) => panic!("already mutably borrowed"),
        }
    }
    pub fn borrow_mut(&self) -> RwLockWriteGuard<'_, T> {
        match self.0.try_write() {
            Ok(g) => g,
            Err(TryLockError::Poisoned(p)) => p.into_inner(),
            Err(TryLockError::WouldBlock) => panic!("already borrowed"),
        }
    }
}

impl<T: fmt::Debug + ?Sized> fmt::Debug for RefCell<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0.try_read() {
            Ok(g) => f.debug_tuple("RefCell").field(&&*g).finish(),
            Err(_) => f.write_str("RefCell(<borrowed>)"),
        }
    }
}

pub struct Cell<T: Copy>(Mutex<T>);

impl<T: Copy> Cell<T> {
    pub const fn new(v: T) -> Cell<T> {
        Cell(Mutex::new(v))
    }
    pub fn get(&self) -> T {
        *self.0.lock().unwrap_or_else(|p| p.into_inner())
    }
    pub fn set(&self, v: T) {
        *self.0.lock().unwrap_or_else(|p| p.into_inner()) = v;
    }
}

impl<T: Copy + fmt::Debug> fmt::Debug for Cell<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Cell").field(&self.get()).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_reads_coexist() {
        let c = RefCell::new(vec![1, 2]);
        let a = c.borrow();
        let b = c.borrow();
        assert_eq!(a.len() + b.len(), 4);
    }

    #[test]
    #[should_panic(expected = "already borrowed")]
    fn a_write_during_a_read_panics_as_refcell_does() {
        let c = RefCell::new(1);
        let _r = c.borrow();
        let _w = c.borrow_mut();
    }

    #[test]
    fn values_cross_threads() {
        let v = Rc::new(RefCell::new(String::from("a")));
        let w = v.clone();
        std::thread::spawn(move || w.borrow_mut().push('b')).join().unwrap();
        assert_eq!(&*v.borrow(), "ab");
        let n = Cell::new(1i64);
        n.set(n.get() + 1);
        assert_eq!(n.get(), 2);
    }
}
