//! `Map` and `Set` (6.7): first-insertion order with constant-time lookup; list helpers with the contracts of
//! library/fortis/prelude.ft.

use std::collections::HashMap;

use crate::hash::FastState;
use std::hash::Hash;

use crate::failure::{Dyn, Failure, Outcome};
use crate::show::Show;

#[derive(Clone, Debug)]
pub struct OrderedMap<K: Hash + Eq + Clone, V> {
    index: HashMap<K, usize, FastState>,
    entries: Vec<(K, V)>,
}

impl<K: Hash + Eq + Clone, V> Default for OrderedMap<K, V> {
    fn default() -> Self {
        OrderedMap { index: HashMap::default(), entries: Vec::new() }
    }
}

impl<K: Hash + Eq + Clone, V> OrderedMap<K, V> {
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }
    /// Lookup by any borrowed form of the key (`map.get("apple")` for a `String` key).
    #[inline]
    pub fn get<Q: ?Sized + Hash + Eq>(&self, k: &Q) -> Option<&V>
    where
        K: std::borrow::Borrow<Q>,
    {
        self.index.get(k).map(|i| &self.entries[*i].1)
    }
    /// Insert or replace; replacing keeps the first insertion's position (the old value is dropped).
    #[inline]
    pub fn set(&mut self, k: K, v: V) {
        use std::collections::hash_map::Entry;
        let n = self.entries.len();
        match self.index.entry(k) {
            Entry::Occupied(e) => self.entries[*e.get()].1 = v,
            Entry::Vacant(e) => {
                self.entries.push((e.key().clone(), v));
                e.insert(n);
            }
        }
    }
    /// `containsKey`: whether the key has an entry, by any borrowed form of the key.
    #[inline]
    pub fn contains_key<Q: ?Sized + Hash + Eq>(&self, k: &Q) -> bool
    where
        K: std::borrow::Borrow<Q>,
    {
        self.index.contains_key(k)
    }
    /// `remove`: the value of the entry, now the caller's, or null when there is none. The entries after it keep
    /// their order and move up one place, so the cost is O(n) in them (6.7).
    pub fn remove<Q: ?Sized + Hash + Eq>(&mut self, k: &Q) -> Option<V>
    where
        K: std::borrow::Borrow<Q>,
    {
        let at = self.index.remove(k)?;
        let (_, v) = self.entries.remove(at);
        for (later, (key, _)) in self.entries.iter().enumerate().skip(at) {
            if let Some(slot) = self.index.get_mut::<K>(key) {
                *slot = later;
            }
        }
        Some(v)
    }
    #[inline]
    pub fn count(&self) -> i64 {
        self.entries.len() as i64
    }
    #[inline]
    pub fn iter(&self) -> impl Iterator<Item = &(K, V)> {
        self.entries.iter()
    }
    #[inline]
    pub fn into_entries(self) -> Vec<(K, V)> {
        self.entries
    }
}

impl<K: Hash + Eq + Clone + PartialEq, V: PartialEq> PartialEq for OrderedMap<K, V> {
    fn eq(&self, o: &Self) -> bool {
        self.entries == o.entries
    }
}

impl<K: Hash + Eq + Clone + Show, V: Show> Show for OrderedMap<K, V> {
    fn show(&self) -> String {
        format!("{{{}}}", self.entries.iter().map(|(k, v)| format!("{}: {}", k.show(), v.show())).collect::<Vec<_>>().join(", "))
    }
}

#[derive(Clone, Debug)]
pub struct OrderedSet<T: Hash + Eq + Clone> {
    map: OrderedMap<T, ()>,
}

impl<T: Hash + Eq + Clone> Default for OrderedSet<T> {
    fn default() -> Self {
        OrderedSet { map: OrderedMap::default() }
    }
}

impl<T: Hash + Eq + Clone> OrderedSet<T> {
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }
    #[inline]
    pub fn add(&mut self, v: T) -> bool {
        if self.map.get::<T>(&v).is_some() {
            return false;
        }
        self.map.set(v, ());
        true
    }
    #[inline]
    pub fn contains<Q: ?Sized + Hash + Eq>(&self, v: &Q) -> bool
    where
        T: std::borrow::Borrow<Q>,
    {
        self.map.get(v).is_some()
    }
    /// `remove`: true when the item was present. The items after it keep their order (6.7).
    #[inline]
    pub fn remove<Q: ?Sized + Hash + Eq>(&mut self, v: &Q) -> bool
    where
        T: std::borrow::Borrow<Q>,
    {
        self.map.remove(v).is_some()
    }
    #[inline]
    pub fn count(&self) -> i64 {
        self.map.count()
    }
    #[inline]
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.map.iter().map(|(k, _)| k)
    }
    #[inline]
    pub fn into_items(self) -> Vec<T> {
        self.map.into_entries().into_iter().map(|(k, _)| k).collect()
    }
}

impl<T: Hash + Eq + Clone + PartialEq> PartialEq for OrderedSet<T> {
    fn eq(&self, o: &Self) -> bool {
        self.map == o.map
    }
}

impl<T: Hash + Eq + Clone + Show> Show for OrderedSet<T> {
    fn show(&self) -> String {
        format!("{{{}}}", self.iter().map(|x| x.show()).collect::<Vec<_>>().join(", "))
    }
}

/// `xs.get(i)` for `(alias T from self)?`: a loan of the item, or null when out of range.
#[inline]
pub fn list_get_ref<T>(xs: &[T], i: i64) -> Option<&T> {
    if i < 0 {
        None
    } else {
        xs.get(i as usize)
    }
}

/// `xs.get(i)`: the item, or null when out of range.
#[inline]
pub fn list_get<T: Clone>(xs: &[T], i: i64) -> Option<T> {
    if i < 0 {
        None
    } else {
        xs.get(i as usize).cloned()
    }
}
/// `xs.at(i)`: the item, or `collection.index`.
#[inline]
pub fn list_at<T: Clone>(xs: &[T], i: i64) -> Outcome<T> {
    if i >= 0 && (i as usize) < xs.len() {
        Ok(xs[i as usize].clone())
    } else {
        Err(Failure::new("collection.index", "").with("index", Dyn::Int(i as i128)))
    }
}
/// `xs.set(i, v)`: replaces the item (dropping the old one), or `collection.index`.
#[inline]
pub fn list_set<T>(xs: &mut [T], i: i64, v: T) -> Outcome<()> {
    if i >= 0 && (i as usize) < xs.len() {
        xs[i as usize] = v;
        Ok(())
    } else {
        Err(Failure::new("collection.index", "").with("index", Dyn::Int(i as i128)))
    }
}
/// `count()` as an `Int` (D-160): a collection of a zero-sized element type can be longer than `Int.MAX` without using
/// memory, so the conversion is checked; such a collection is past what an `Int` count can describe, which is out of
/// memory (10.8), never a negative count.
#[inline]
pub fn count<T>(xs: &[T]) -> i64 {
    i64::try_from(xs.len()).unwrap_or_else(|_| crate::defect("out_of_memory"))
}

/// The length of `a` and `b` joined, checked against `Int.MAX` before anything is copied (D-160).
#[inline]
pub fn joined_len<T>(a: &[T], b: &[T]) -> usize {
    match a.len().checked_add(b.len()) {
        Some(n) if n <= i64::MAX as usize => n,
        _ => crate::defect("out_of_memory"),
    }
}

/// `xs[i]` with a proved index: a defect when the proof was wrong.
#[inline]
pub fn index<T: Clone>(xs: &[T], i: i64) -> T {
    match xs.get(i as usize) {
        Some(x) if i >= 0 => x.clone(),
        _ => crate::defect("index_out_of_range"),
    }
}

/// `List.iterate()`: a cursor over read loans of the items (`ListCursor[T] from self`). It copies nothing: a copy
/// of an item would be a second owner, and its drop would run the item's `cleanup` again (12.2; D-338).
pub struct Cursor<'a, T> {
    /// the items walked first, last to first when `reversed`
    first: &'a [T],
    reversed: bool,
    /// the items walked after `first`, first to last
    second: &'a [T],
    pos: usize,
}
impl<'a, T> Cursor<'a, T> {
    #[inline]
    pub fn new(items: &'a [T]) -> Cursor<'a, T> {
        Cursor { first: items, reversed: false, second: &[], pos: 0 }
    }
    /// `Deque.iterate()` (X-576): the items of `front` from the last to the first, then those of `back`.
    #[inline]
    pub fn joined(front: &'a [T], back: &'a [T]) -> Cursor<'a, T> {
        Cursor { first: front, reversed: true, second: back, pos: 0 }
    }
    /// `next()`: a read loan of the next item, `(alias T from self)?`.
    #[inline]
    pub fn next(&mut self) -> Option<&'a T> {
        let n = self.first.len();
        let v = if self.pos < n { self.first.get(if self.reversed { n - 1 - self.pos } else { self.pos }) } else { self.second.get(self.pos - n) };
        if v.is_some() {
            self.pos += 1;
        }
        v
    }
}

/// A `for` over a class that implements `Iterable` walks the cursor its `iterate()` gives (5.3; D-545).
impl<'a, T> Iterator for Cursor<'a, T> {
    type Item = &'a T;
    #[inline]
    fn next(&mut self) -> Option<&'a T> {
        Cursor::next(self)
    }
}

/// `Outcome.collect(xs)`: every success, or the first failure.
#[inline]
pub fn collect<T>(items: Vec<Outcome<T>>) -> Outcome<Vec<T>> {
    let mut out = Vec::with_capacity(items.len());
    for it in items {
        out.push(it?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    /// D-160: a zero-sized collection longer than `Int.MAX` has no `Int` count. `count` stops the program with the
    /// out-of-memory defect (exit 101 and a `defect` record) instead of returning a negative count. The defect exits the
    /// process, so the check runs in a child process.
    #[test]
    fn count_past_int_max_is_a_defect() {
        if std::env::var("FORTIS_D160_CHILD").is_ok() {
            // 63 doublings, as the generated `concat` does them (a slice copy of zero-sized elements copies nothing), reach
            // a length of 2^63 = Int.MAX + 1 at once and without memory
            let mut v: Vec<()> = vec![()];
            for _ in 0..63 {
                let w = v.clone();
                v.extend_from_slice(&w);
            }
            let n = super::count(&v);
            println!("CHILD-RETURNED {n}");
            std::process::exit(0);
        }
        let exe = std::env::current_exe().expect("test binary");
        let out = std::process::Command::new(exe)
            .args(["--exact", "collections::tests::count_past_int_max_is_a_defect", "--nocapture", "--test-threads=1"])
            .env("FORTIS_D160_CHILD", "1")
            .output()
            .expect("child");
        let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        assert!(!text.contains("CHILD-RETURNED"), "count returned for a length past Int.MAX: {text}");
        assert!(text.contains("out_of_memory"), "no out-of-memory defect record: {text}");
        assert_eq!(out.status.code(), Some(101), "{text}");
    }

    /// D-160: the joined length of two collections is checked against `Int.MAX` before anything is built.
    #[test]
    fn joined_len_within_int_max() {
        let a: Vec<()> = vec![(); 3];
        assert_eq!(super::joined_len(&a, &a), 6);
    }
}
