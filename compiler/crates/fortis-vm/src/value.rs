//! Runtime values of the VM: tagged, with aggregates behind reference counts so that a loan shares its owner's
//! value exactly as the definitional interpreter's object references do. Display, equality, deep copy and the
//! structural keys of `Map` and `Set` follow interpreter/values.py and interpreter/intrinsics.py.

use crate::sync::{Cell, RefCell};
use std::collections::HashMap;
use crate::sync::Rc;

use fortis_runtime::Decimal;

pub type Shared<T> = Rc<RefCell<T>>;

pub fn shared<T>(v: T) -> Shared<T> {
    Rc::new(RefCell::new(v))
}

#[derive(Clone, Debug)]
pub enum Value {
    Void,
    Null,
    /// the content of a local whose value was moved out (7.4): never read, never destroyed
    Moved,
    Bool(bool),
    Int(i128),
    Float(f64),
    F32(f32),
    Dec(Decimal),
    Dur(i64),
    Time(i64),
    Str(Rc<str>),
    Scalar(Rc<str>),
    Bytes(Rc<Vec<u8>>),
    List(Shared<Vec<Value>>),
    Array(Shared<Vec<Value>>),
    Map(Shared<OrderedMap>),
    Set(Shared<OrderedMap>),
    Tuple(Rc<TupleVal>),
    Record(Shared<RecordVal>),
    Obj(Shared<Obj>),
    Enum(Rc<EnumVal>),
    Failure(Shared<FailureVal>),
    Report(Rc<str>),
    Closure(Rc<ClosureVal>),
    Func(usize),
    Intrinsic(Rc<str>),
    Builder(Shared<String>),
    SharedRef(Rc<SharedBox>),
    WeakRef(Rc<SharedBox>),
    Mutex(Shared<MutexVal>),
    Guard(Shared<MutexVal>),
    Atomic(Rc<str>, Shared<Value>),
    Cursor(Shared<(Vec<Value>, usize)>),
    Cap(Rc<CapVal>),
    Path(Rc<str>),
    Secret(Rc<Value>),
    File(Shared<FileVal>),
    /// the storage of a local that a closure captured by reference
    Cell(Shared<Value>),
    /// an async call that has not run: its arguments bound, its body pending (12.3)
    Future(Rc<FutureVal>),
    /// a launched future (12.5)
    Task(Rc<TaskRef>),
    /// a `scope` (12.5)
    Scope(Rc<ScopeVal>),
    /// `join(a, b)` not yet awaited
    Join(Rc<Vec<Value>>),
    /// `clock.sleep(d)` not yet awaited: virtual nanoseconds
    Sleep(i64),
    /// an async intrinsic that completed without suspending
    Ready(Rc<Value>),
}

/// Which parts of an aggregate are loans: one flag per element or field, recorded by the tuple or record literal
/// that built it from what each part's expression gave as the program ran. A part whose type is a loan can hold a
/// value of its own (a `??` whose fallback is owned, an `if` that moved a value in, a binding that owns one), and
/// the aggregate owns and destroys that value; a loan in it is its owner's (7.9; D-349, D-366). An aggregate made
/// elsewhere (a library result, a decoded value) records none, and its owner decides by the declared type.
pub type LentParts = Option<Rc<[bool]>>;

/// A tuple's elements and which of them it borrows.
#[derive(Clone, Debug, Default)]
pub struct TupleVal {
    pub items: Vec<Value>,
    pub lent: LentParts,
}

impl TupleVal {
    pub fn new(items: Vec<Value>, lent: LentParts) -> TupleVal {
        TupleVal { items, lent }
    }
}

impl From<Vec<Value>> for TupleVal {
    fn from(items: Vec<Value>) -> TupleVal {
        TupleVal { items, lent: None }
    }
}

impl std::ops::Deref for TupleVal {
    type Target = Vec<Value>;
    fn deref(&self) -> &Vec<Value> {
        &self.items
    }
}

impl std::ops::DerefMut for TupleVal {
    fn deref_mut(&mut self) -> &mut Vec<Value> {
        &mut self.items
    }
}

/// A record literal's fields, in written order, and which of them it borrows. An assignment to a field records
/// what the new value is, a loan or a value of its own.
#[derive(Clone, Debug, Default)]
pub struct RecordVal {
    pub fields: Vec<(Rc<str>, Value)>,
    pub lent: LentParts,
}

impl RecordVal {
    pub fn new(fields: Vec<(Rc<str>, Value)>, lent: LentParts) -> RecordVal {
        RecordVal { fields, lent }
    }
}

impl From<Vec<(Rc<str>, Value)>> for RecordVal {
    fn from(fields: Vec<(Rc<str>, Value)>) -> RecordVal {
        RecordVal { fields, lent: None }
    }
}

impl std::ops::Deref for RecordVal {
    type Target = Vec<(Rc<str>, Value)>;
    fn deref(&self) -> &Vec<(Rc<str>, Value)> {
        &self.fields
    }
}

impl std::ops::DerefMut for RecordVal {
    fn deref_mut(&mut self) -> &mut Vec<(Rc<str>, Value)> {
        &mut self.fields
    }
}

/// Whether part `i` of an aggregate is a loan, as recorded when it was built; `None` when nothing was recorded.
pub fn lent_part(lent: &LentParts, i: usize) -> Option<bool> {
    lent.as_ref().and_then(|m| m.get(i).copied())
}

#[derive(Debug)]
pub struct FutureVal {
    pub fname: String,
    pub work: RefCell<Option<crate::executor::Work>>,
    /// the task it became when launched
    pub task: Cell<Option<usize>>,
}

impl FutureVal {
    pub fn new(fname: &str, work: crate::executor::Work) -> FutureVal {
        FutureVal { fname: fname.to_string(), work: RefCell::new(Some(work)), task: Cell::new(None) }
    }
}

#[derive(Debug)]
pub struct TaskRef {
    pub id: usize,
    pub name: String,
}

#[derive(Debug)]
pub struct ScopeVal {
    pub name: String,
    /// launched, not yet joined or detached, in launch order
    pub tasks: RefCell<Vec<usize>>,
}

#[derive(Debug)]
pub struct Obj {
    pub ty: Rc<str>,
    pub fields: Vec<(Rc<str>, Value)>,
    /// its owner has destroyed it: its `cleanup` ran and its fields went. A second destruction, which only a
    /// program the checker should have refused can reach (two owners of one value, D-352), does nothing, so
    /// `cleanup` runs exactly once (7.9, 12.2; D-348)
    pub destroyed: bool,
}

impl Obj {
    pub fn new(ty: Rc<str>, fields: Vec<(Rc<str>, Value)>) -> Obj {
        Obj { ty, fields, destroyed: false }
    }
    pub fn get(&self, f: &str) -> Option<&Value> {
        self.fields.iter().find(|(n, _)| &**n == f).map(|(_, v)| v)
    }
    pub fn set(&mut self, f: &str, v: Value) -> Option<Value> {
        match self.fields.iter_mut().find(|(n, _)| &**n == f) {
            Some((_, slot)) => Some(std::mem::replace(slot, v)),
            None => {
                self.fields.push((Rc::from(f), v));
                None
            }
        }
    }
}

#[derive(Debug)]
pub struct EnumVal {
    pub ty: Rc<str>,
    pub variant: Rc<str>,
    pub fields: Vec<(Rc<str>, Value)>,
}

#[derive(Debug)]
pub struct FailureVal {
    pub code: Rc<str>,
    pub payload: Vec<(Rc<str>, Value)>,
    pub origin: Rc<str>,
    pub message: Rc<str>,
    pub cause: Option<Shared<FailureVal>>,
    pub annotations: Vec<Value>,
}

#[derive(Debug)]
pub struct ClosureVal {
    pub func: usize,
    /// one per capture, in capture-list order: a by-reference capture shares its owner's cell; a by-value one
    /// (`copy`, `move`, bare) has its own cell, which every call's frame shares, so a `fn mut` body's assignment
    /// lasts to the next call and a `fn once` body's move out of it is seen when the closure is destroyed
    pub captures: Vec<Value>,
    /// which captures the closure owns (the by-value ones): destroying it destroys their values (7.9, X-250; D-30)
    pub owned: Vec<bool>,
    /// in a development session, the identity a frame of another generation resolves it by (SP-07 design 5)
    pub code: Option<Rc<crate::generation::CodeId>>,
}

#[derive(Debug)]
pub struct SharedBox {
    pub value: RefCell<Value>,
    pub strong: Cell<i64>,
}

#[derive(Debug)]
pub struct MutexVal {
    pub value: Value,
    pub held: bool,
}

#[derive(Debug)]
pub struct CapVal {
    pub kind: Rc<str>,
    pub state: RefCell<HashMap<String, Value>>,
}

#[derive(Debug)]
pub struct FileVal {
    pub path: String,
    pub mode: char,
    pub closed: bool,
    pub pos: usize,
}

/// A structural key: equal keys for values equal under derived Equatable/Hashable (6.10).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    Null,
    Bool(bool),
    Int(i128),
    Str(String),
    List(Vec<Key>),
    Tuple(Vec<Key>),
    Enum(String, String, Vec<(String, Key)>),
    Inst(String, Vec<(String, Key)>),
    Dec(bool, u128, u32),
    Dur(i64),
    Scalar(String),
}

/// `Map` and `Set` storage: first-insertion order, constant-time lookup by structural key.
#[derive(Debug, Default)]
pub struct OrderedMap {
    pub index: HashMap<Key, usize>,
    pub entries: Vec<(Value, Value)>,
}

impl OrderedMap {
    pub fn get(&self, k: &Key) -> Option<&(Value, Value)> {
        self.index.get(k).map(|i| &self.entries[*i])
    }
    /// Insert or replace; replacing keeps the first insertion's position and returns the old value.
    pub fn insert(&mut self, k: Key, key: Value, v: Value) -> Option<Value> {
        match self.index.get(&k) {
            Some(i) => Some(std::mem::replace(&mut self.entries[*i].1, v)),
            None => {
                self.index.insert(k, self.entries.len());
                self.entries.push((key, v));
                None
            }
        }
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    /// Remove the entry at position `i`, keeping the others in order: the structural index of each later entry
    /// moves up one place (6.7: `remove` costs O(n) in the entries after it).
    pub fn remove_at(&mut self, i: usize) -> (Value, Value) {
        let entry = self.entries.remove(i);
        self.index.retain(|_, pos| *pos != i);
        for pos in self.index.values_mut() {
            if *pos > i {
                *pos -= 1;
            }
        }
        entry
    }
}

impl Value {
    pub fn str(s: &str) -> Value {
        Value::Str(Rc::from(s))
    }
    pub fn list(items: Vec<Value>) -> Value {
        Value::List(shared(items))
    }
    pub fn is_failure(&self) -> bool {
        matches!(self, Value::Failure(_))
    }
    /// The value behind a by-reference capture cell.
    pub fn deref(&self) -> Value {
        match self {
            Value::Cell(c) => c.borrow().deref(),
            v => v.clone(),
        }
    }
    pub fn as_int(&self) -> Option<i128> {
        match self {
            Value::Int(n) => Some(*n),
            Value::Cell(c) => c.borrow().as_int(),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<Rc<str>> {
        match self {
            Value::Str(s) => Some(s.clone()),
            Value::Cell(c) => c.borrow().as_str(),
            _ => None,
        }
    }
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Void => "Void",
            Value::Null => "Null",
            Value::Moved => "Moved",
            Value::Bool(_) => "Bool",
            Value::Int(_) => "Int",
            Value::Float(_) => "Float",
            Value::F32(_) => "Float32",
            Value::Dec(_) => "Decimal",
            Value::Dur(_) => "Duration",
            Value::Time(_) => "Timestamp",
            Value::Str(_) => "String",
            Value::Scalar(_) => "Scalar",
            Value::Bytes(_) => "Bytes",
            Value::List(_) => "List",
            Value::Array(_) => "Array",
            Value::Map(_) => "Map",
            Value::Set(_) => "Set",
            Value::Tuple(_) => "Tuple",
            Value::Record(_) => "Record",
            Value::Obj(_) => "Object",
            Value::Enum(_) => "Enum",
            Value::Failure(_) => "Failure",
            Value::Report(_) => "Report",
            Value::Closure(_) | Value::Func(_) | Value::Intrinsic(_) => "Function",
            Value::Builder(_) => "Builder",
            Value::SharedRef(_) => "Shared",
            Value::WeakRef(_) => "Weak",
            Value::Mutex(_) => "Mutex",
            Value::Guard(_) => "Guard",
            Value::Atomic(..) => "Atomic",
            Value::Cursor(_) => "ListCursor",
            Value::Cap(_) => "Capability",
            Value::Path(_) => "Path",
            Value::Secret(_) => "Secret",
            Value::File(_) => "File",
            Value::Cell(_) => "Cell",
            Value::Future(_) => "Future",
            Value::Task(_) => "Task",
            Value::Scope(_) => "Scope",
            Value::Join(_) => "Join",
            Value::Sleep(_) => "Sleep",
            Value::Ready(_) => "Ready",
        }
    }
}

/// Python's `repr` of a value inside a tuple or record display (the interpreter prints those with `repr`).
fn py_repr(v: &Value) -> String {
    match v {
        Value::Str(s) => {
            if s.contains('\'') && !s.contains('"') {
                format!("\"{s}\"")
            } else {
                format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
            }
        }
        Value::Bool(b) => if *b { "True".into() } else { "False".into() },
        Value::Null => "None".into(),
        Value::Int(n) => n.to_string(),
        Value::Tuple(items) => tuple_repr(items),
        other => display(other),
    }
}

fn tuple_repr(items: &[Value]) -> String {
    if items.len() == 1 {
        format!("({},)", py_repr(&items[0]))
    } else {
        format!("({})", items.iter().map(py_repr).collect::<Vec<_>>().join(", "))
    }
}

/// The display form (6.10), as the interpreter's `display`.
pub fn display(v: &Value) -> String {
    match v {
        Value::Null => "null".into(),
        Value::Void => "Void".into(),
        Value::Moved => "<moved>".into(),
        Value::Bool(b) => if *b { "true".into() } else { "false".into() },
        Value::Int(n) => n.to_string(),
        Value::Float(x) => fortis_runtime::float::display(*x),
        Value::F32(x) => fortis_runtime::float::display32(*x),
        Value::Dec(d) => d.to_string(),
        Value::Dur(ns) => fortis_runtime::time::display_duration(*ns),
        Value::Time(ns) => fortis_runtime::time::display_timestamp(*ns),
        Value::Str(s) | Value::Scalar(s) | Value::Path(s) => s.to_string(),
        Value::Report(c) => format!("Report(code='{c}')"),
        Value::Obj(o) => {
            let o = o.borrow();
            format!("{}({})", o.ty, o.fields.iter().map(|(k, v)| format!("{k}: {}", display(v))).collect::<Vec<_>>().join(", "))
        }
        Value::Enum(e) => {
            if e.fields.is_empty() {
                e.variant.to_string()
            } else {
                format!("{}({})", e.variant, e.fields.iter().map(|(k, v)| format!("{k}: {}", display(v))).collect::<Vec<_>>().join(", "))
            }
        }
        Value::List(items) | Value::Array(items) => format!("[{}]", items.borrow().iter().map(display).collect::<Vec<_>>().join(", ")),
        Value::Failure(f) => format!("Failure({})", f.borrow().code),
        Value::SharedRef(b) => display(&b.value.borrow()),
        Value::WeakRef(_) => "Weak".into(),
        Value::Cap(c) => {
            let mut s = c.kind.to_string();
            if let Some(f) = s.get_mut(0..1) {
                f.make_ascii_uppercase();
            }
            s
        }
        Value::Tuple(items) => tuple_repr(items),
        Value::Record(fs) => format!("{{{}}}", fs.borrow().iter().map(|(k, v)| format!("'{k}': {}", py_repr(v))).collect::<Vec<_>>().join(", ")),
        Value::Secret(_) => "Secret(********)".into(),
        Value::Cell(c) => display(&c.borrow()),
        Value::Future(f) => format!("Future({})", f.fname),
        Value::Task(t) => format!("Task({})", t.name),
        Value::Scope(s) => format!("Scope({})", s.name),
        Value::Join(_) => "Join".into(),
        Value::Sleep(ns) => format!("Sleep({ns}ns)"),
        Value::Ready(v) => display(v),
        Value::Bytes(b) => format!("BytesVal(data=b'{}')", String::from_utf8_lossy(b)),
        other => format!("<{}>", other.type_name()),
    }
}

/// `copy p` (6.11): a deep copy of every heap part; trivial values are copied by value.
pub fn deep_copy(v: &Value) -> Value {
    match v {
        Value::List(items) => Value::List(shared(items.borrow().iter().map(deep_copy).collect())),
        Value::Array(items) => Value::Array(shared(items.borrow().iter().map(deep_copy).collect())),
        Value::Map(m) => {
            let m = m.borrow();
            Value::Map(shared(OrderedMap { index: m.index.clone(), entries: m.entries.iter().map(|(k, v)| (deep_copy(k), deep_copy(v))).collect() }))
        }
        Value::Set(m) => {
            let m = m.borrow();
            Value::Set(shared(OrderedMap { index: m.index.clone(), entries: m.entries.iter().map(|(k, v)| (deep_copy(k), deep_copy(v))).collect() }))
        }
        // a copy of an aggregate copies the parts it owns and holds the same loans, as a copy of a loan is that loan
        // (7.9; D-349, D-366)
        Value::Tuple(t) => {
            let items = t.items.iter().enumerate().map(|(i, x)| if lent_part(&t.lent, i) == Some(true) { x.clone() } else { deep_copy(x) }).collect();
            Value::Tuple(Rc::new(TupleVal::new(items, t.lent.clone())))
        }
        Value::Record(fs) => {
            let r = fs.borrow();
            let fields = r.fields.iter().enumerate().map(|(i, (k, v))| (k.clone(), if lent_part(&r.lent, i) == Some(true) { v.clone() } else { deep_copy(v) })).collect();
            Value::Record(shared(RecordVal::new(fields, r.lent.clone())))
        }
        Value::Obj(o) => {
            let o = o.borrow();
            Value::Obj(shared(Obj::new(o.ty.clone(), o.fields.iter().map(|(k, v)| (k.clone(), deep_copy(v))).collect())))
        }
        Value::Enum(e) => Value::Enum(Rc::new(EnumVal { ty: e.ty.clone(), variant: e.variant.clone(), fields: e.fields.iter().map(|(k, v)| (k.clone(), deep_copy(v))).collect() })),
        Value::Failure(f) => {
            let f = f.borrow();
            Value::Failure(shared(FailureVal {
                code: f.code.clone(),
                payload: f.payload.iter().map(|(k, v)| (k.clone(), deep_copy(v))).collect(),
                origin: f.origin.clone(),
                message: f.message.clone(),
                cause: f.cause.clone(),
                annotations: f.annotations.clone(),
            }))
        }
        Value::Builder(b) => Value::Builder(shared(b.borrow().clone())),
        Value::Cell(c) => deep_copy(&c.borrow()),
        other => other.clone(),
    }
}

/// The structural key of a hashable value, or the reason it is not hashable.
pub fn freeze(v: &Value) -> Result<Key, String> {
    Ok(match v {
        Value::Null => Key::Null,
        Value::Bool(b) => Key::Bool(*b),
        Value::Int(n) => Key::Int(*n),
        Value::Str(s) => Key::Str(s.to_string()),
        Value::List(items) => Key::List(items.borrow().iter().map(freeze).collect::<Result<_, _>>()?),
        Value::Tuple(items) => Key::Tuple(items.iter().map(freeze).collect::<Result<_, _>>()?),
        Value::Enum(e) => Key::Enum(e.ty.to_string(), e.variant.to_string(), e.fields.iter().map(|(k, v)| Ok((k.to_string(), freeze(v)?))).collect::<Result<_, String>>()?),
        Value::Obj(o) => {
            let o = o.borrow();
            Key::Inst(o.ty.to_string(), o.fields.iter().map(|(k, v)| Ok((k.to_string(), freeze(v)?))).collect::<Result<_, String>>()?)
        }
        Value::Dec(d) => Key::Dec(d.neg, d.coeff, d.scale),
        Value::Dur(ns) => Key::Dur(*ns),
        Value::Scalar(s) => Key::Scalar(s.to_string()),
        Value::Cell(c) => freeze(&c.borrow())?,
        other => return Err(format!("{} is not Hashable", display(other))),
    })
}
