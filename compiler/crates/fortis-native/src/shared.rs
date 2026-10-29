//! Shared ownership and synchronisation of the synchronous subset (7.8, 12.7): `Shared[T]` is a reference-counted
//! cell whose duplication is an explicit `retain`, `Weak[T]` its non-owning handle; `Mutex` and atomics are
//! single-threaded here (parallel pools are DEL-39's).

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use crate::show::Show;

#[derive(Debug)]
pub struct Shared<T>(pub Rc<RefCell<T>>);

impl<T> Shared<T> {
    pub fn create(v: T) -> Shared<T> {
        Shared(Rc::new(RefCell::new(v)))
    }
    /// `s.retain()`: a second strong handle (the only way to duplicate one)
    pub fn retain(&self) -> Shared<T> {
        Shared(Rc::clone(&self.0))
    }
    pub fn downgrade(&self) -> WeakRef<T> {
        WeakRef(Rc::downgrade(&self.0))
    }
    pub fn read(&self) -> std::cell::Ref<'_, T> {
        self.0.borrow()
    }
    pub fn write(&self) -> std::cell::RefMut<'_, T> {
        self.0.borrow_mut()
    }
}

impl<T: Show> Show for Shared<T> {
    fn show(&self) -> String {
        self.0.borrow().show()
    }
}
impl<T: PartialEq> PartialEq for Shared<T> {
    fn eq(&self, o: &Self) -> bool {
        *self.0.borrow() == *o.0.borrow()
    }
}

#[derive(Debug)]
pub struct WeakRef<T>(pub Weak<RefCell<T>>);

impl<T> WeakRef<T> {
    pub fn upgrade(&self) -> Option<Shared<T>> {
        self.0.upgrade().map(Shared)
    }
    pub fn retain(&self) -> WeakRef<T> {
        WeakRef(Weak::clone(&self.0))
    }
}
impl<T> Show for WeakRef<T> {
    fn show(&self) -> String {
        "Weak".into()
    }
}

#[derive(Debug)]
pub struct Mutex<T> {
    value: RefCell<T>,
    held: Cell<bool>,
}
impl<T> Mutex<T> {
    pub fn create(v: T) -> Mutex<T> {
        Mutex { value: RefCell::new(v), held: Cell::new(false) }
    }
    /// `m.lockBlocking()`: the guard, or `mutex.deadlock` when this task already holds the lock.
    pub fn lock_blocking(&self) -> crate::Outcome<Guard<'_, T>> {
        if self.held.replace(true) {
            return Err(crate::Failure::new("mutex.deadlock", ""));
        }
        Ok(Guard { mutex: self })
    }
}

/// A `MutexGuard[T]`: the exclusive access its lock granted, until it is dropped (12.7). Reads and writes go through
/// `&self`, as the guard is a loan of the value, not a mutable binding (D-23).
#[derive(Debug)]
pub struct Guard<'a, T> {
    mutex: &'a Mutex<T>,
}
impl<T> Guard<'_, T> {
    /// `guard.get()`: a copy of the protected value.
    pub fn get(&self) -> T
    where
        T: Clone,
    {
        self.mutex.value.borrow().clone()
    }
    /// `guard.set(value)`: the old value is dropped.
    pub fn set(&self, v: T) {
        let old = self.mutex.value.replace(v);
        drop(old);
    }
    /// `guard.replace(value)`: the old value, now the caller's.
    pub fn replace(&self, v: T) -> T {
        self.mutex.value.replace(v)
    }
}
impl<T> Drop for Guard<'_, T> {
    fn drop(&mut self) {
        self.mutex.held.set(false);
    }
}

#[derive(Debug, Default)]
pub struct AtomicInt(Cell<i64>);
impl AtomicInt {
    pub fn create(v: i64) -> AtomicInt {
        AtomicInt(Cell::new(v))
    }
    pub fn load(&self) -> i64 {
        self.0.get()
    }
    pub fn store(&self, v: i64) {
        self.0.set(v)
    }
    pub fn fetch_add(&self, d: i64) -> i64 {
        let old = self.0.get();
        self.0.set(old.wrapping_add(d));
        old
    }
}
#[derive(Debug, Default)]
pub struct AtomicBool(Cell<bool>);
impl AtomicBool {
    pub fn create(v: bool) -> AtomicBool {
        AtomicBool(Cell::new(v))
    }
    pub fn load(&self) -> bool {
        self.0.get()
    }
    pub fn store(&self, v: bool) {
        self.0.set(v)
    }
}

/// A value that is either borrowed or owned, read through one reference: `p ?? fallback` over an optional read
/// parameter of a type that cannot be copied borrows the present value, or owns the fallback, which is evaluated only
/// when the parameter is null (D-2).
pub enum MaybeOwned<'a, T> {
    Borrowed(&'a T),
    Owned(T),
}
impl<'a, T> std::ops::Deref for MaybeOwned<'a, T> {
    type Target = T;
    fn deref(&self) -> &T {
        match self {
            MaybeOwned::Borrowed(v) => v,
            MaybeOwned::Owned(v) => v,
        }
    }
}
impl<'a, T: Show> Show for MaybeOwned<'a, T> {
    fn show(&self) -> String {
        (**self).show()
    }
}
/// A tuple holds a lent `??` as its `MaybeOwned`, so it owns the fallback the `??` gave and destroys it in element
/// order (7.9; D-367). A copy of the tuple copies what it owns and holds the same loan, as a copy of a loan is that
/// loan.
impl<'a, T: Clone> Clone for MaybeOwned<'a, T> {
    fn clone(&self) -> Self {
        match self {
            MaybeOwned::Borrowed(v) => MaybeOwned::Borrowed(v),
            MaybeOwned::Owned(v) => MaybeOwned::Owned(v.clone()),
        }
    }
}

/// The present value of an optional, borrowed or owned, as a `MaybeOwned`: a borrowed `p ?? fallback` reads an
/// optional that is a loan (`xs.get(i)`) or a value (`m.get(k)`, which copies), and the fallback is evaluated only
/// in the `None` arm (5.7; D-246).
pub trait IntoMaybeOwned<'a, T> {
    fn into_maybe_owned(self) -> MaybeOwned<'a, T>;
}
impl<'a, T> IntoMaybeOwned<'a, T> for &'a T {
    fn into_maybe_owned(self) -> MaybeOwned<'a, T> {
        MaybeOwned::Borrowed(self)
    }
}
impl<'a, T> IntoMaybeOwned<'a, T> for T {
    fn into_maybe_owned(self) -> MaybeOwned<'a, T> {
        MaybeOwned::Owned(self)
    }
}

// a class's derived `Show` names every field: a Mutex has no display form of its own (it is not Displayable), so it
// shows as its type, as the VM names it; without this a class holding a Mutex does not build (D-153)
impl<T> Show for Mutex<T> {
    fn show(&self) -> String {
        "Mutex".into()
    }
}

// an atomic shows as the interpreter displays it: its current value
impl Show for AtomicInt {
    fn show(&self) -> String {
        self.0.get().to_string()
    }
}
impl Show for AtomicBool {
    fn show(&self) -> String {
        self.0.get().to_string()
    }
}
