# Lowering reference and the Rust-expressible subset (LR C)

<!-- covers: LR C -->
*Agent mode. Human mode: [Lowering to Rust](../guide/appendix-c-lowering.md). Source of truth: LR C, LR 7.10.*

## Rules

| Rule | Statement | LR |
|---|---|---|
| LANG-07-03 | No clone, reference count or hidden shared ownership is ever inserted to repair a rejected program. | LR 7.10, LR C.1 |
| LANG-07-09 | An ownership shape with no lowering is rejected with `E-OWN-090` (or the earlier rule's more specific code), never lowered with `unsafe` or lifetime erasure. | LR 7.10, LR C.2 |
| LANG-11-03 | `noalloc` is decided by the allocation whitelist of LR C.3 against the actual target and telemetry plan. | LR 11.4, LR C.3 |

Generated application crates carry `#![forbid(unsafe_code)]`; a `rustc` rejection of frontend-accepted code is a
compiler defect only inside the Rust-checked fragment (LR 7.10, LR C.1).

## Shapes (C.1)

| Construct | Native shape | Rust-checked | Frontend-checked | LR |
|---|---|---|---|---|
| `type` newtype | `struct Name(Repr)`, private field | nominal identity | refinement invariant | LR C.1 |
| class | `struct` + `impl` | field privacy | generated-constructor visibility | LR C.1 |
| trivial type | `Copy` struct/enum | `Copy` | the trivial table | LR C.1 |
| `copy p` | `Clone::clone` of a generated `Clone` | | copyable set; allocation | LR C.1 |
| read loan / exclusive loan | `&T` / `&mut T` | loan validity | lexical end (stricter); field-freezing | LR C.1 |
| `move p` | move | use after move | | LR C.1 |
| `from` return | `fn f<'a>(x: &'a T) -> &'a U` | lifetime | single origin | LR C.1 |
| standard view / cursor | `&'a str`, `&'a [T]`, `Cursor<'a, T>` owned by its holder | lifetime | standard types only (X-137) | LR C.1 |
| `Shared[T]` / `Weak[T]` | `Arc<T>` / `Weak<T>` | `Send`/`Sync` | `retain` visibility | LR C.1 |
| `Mutex[T]` | `std::sync::Mutex` wrapper | `Send`/`Sync` | guard across `await` | LR C.1 |
| interface / `dynamic I` | trait + generics / `Box<dyn I>`, `&dyn I`, `Arc<dyn I>` | trait bounds; object safety | substitutability, coherence; dynamic eligibility, allocation | LR C.1 |
| mixin | generated inherent methods | | conflicts, required methods | LR C.1 |
| lambda | closure, concrete environment | `Fn`/`FnMut`/`FnOnce` | declared kind ≥ needed kind; capture list | LR C.1 |
| `try func` / `catch` | `Result<T, Failure>` / `match` | | linearity; recover/return semantics | LR C.1 |
| `T?` / `match` | `Option<T>` / `match` | exhaustiveness (both) | binding ownership | LR C.1 |
| `async func` / `scope` | `async fn` state machine / join set polled by the parent | | linear futures, cancellation at `await`; join on exit | LR C.1 |
| `launchParallel` | pool spawn with `'static + Send` | `Send` | `Shareable` | LR C.1 |
| `defer` | body inlined at every exit (not `Drop`) | each inlined copy's borrows | places initialised at every exit | LR C.1 |
| `cleanup` | `impl Drop` calling the method | drop order | effects contributed | LR C.1 |
| `noalloc`, effects | no representation | | whitelist (C.3); summaries | LR C.1 |
| `capture`/`record` | frame struct writes, record calls | | definite assignment | LR C.1 |
| capability | zero-sized or handle struct, private constructor | | `io` requires capability | LR C.1 |

## Unsupported shapes (C.2)

| Shape | Code | Alternative | LR |
|---|---|---|---|
| loan `from` two parameters | `E-SYN-000` | return owned, or split | LR C.2, LR 7.6 |
| class storing a loan (lifetime parameter, loan or loan-carrying field other than a loan `from static`) | `E-OWN-053` | own, copy, `Shared`, or pass per call | LR C.2, LR 7.7 |
| self-referential class | `E-OWN-053` | indices, or `Shared` + `Weak` | LR C.2, LR 7.7 |
| loan stored in a collection, `Shared` or `Mutex` | `E-OWN-053` | own the value | LR C.2 |
| `fn mut` escaping into `Shared` | `E-OWN-090` | `Mutex` around the state + `fn` closure | LR C.2 |
| future holding a loan on a parallel pool | `E-ASY-003` | pass owned data | LR C.2, LR 12.7 |
| partial move | `E-OWN-040` | move the whole, or restructure | LR C.2, LR 7.2 |

A shape an earlier rule excludes keeps that rule's code (X-126); `E-OWN-090` is for shapes only the lowering
identifies (LR C.2).

## Allocation whitelist (C.3)

| Allocation-free | Allocating | LR |
|---|---|---|
| arithmetic, comparison; field and array access; trivial and inline values (`Array`, `FixedList`, `FixedMap`, bounded `Failure`); a class or record built from trivial, moved, fresh, call-returned or retained (`retain()`, `downgrade()`) values; read/exclusive loans; calls of non-`alloc` functions; `match`; `defer`; `join(a, b)`; `capture` into a bounded frame | `List`/`String`/`Map` growth; `Shared.create`; `dynamic` creation; `copy` of a heap-owning value; boxed futures of `dynamic` async calls; extended failure payloads; `launchParallel`/`launchBlocking`; every cooperative `launch` in v1 (no inline task storage, X-125) | LR C.3 |

## Diagnostics

| Code | Trigger | Correction | LR |
|---|---|---|---|
| E-OWN-040 | `move p.field` while `p` still owns the rest | move the whole binding; consume it with `let Name(field: x, …) = move p` | LR 7.2, LR C.2 |
| E-OWN-053 | lifetime parameter, or a field or storage type argument of a loan or loan-carrying type (not `from static`) | own the data (`String` for `StringView`), copy it, `Shared`, or pass it per call | LR 7.7, LR C.2 |
| E-OWN-090 | a shape with no lowering (`fn mut` into `Shared`/`Mutex`) | own the value; `Mutex` or atomic state with a `fn` closure | LR 7.10, LR C.2 |
| E-ASY-003 | a pool launch whose future borrows or holds a non-`Shareable` value | pass owned `Shareable` data (`move`, `copy`) | LR 12.7, LR C.2 |
| E-EFF-001 | an allocating pattern in a `noalloc` body | remove the allocation (a caller-provided buffer, an inline `Array`/`FixedList`); removing the claim is an API change, never a correction (LR 11.3) | LR 11.1, LR 11.3, LR C.3 |

## Correction patterns

### E-OWN-040: partial move (LR 7.2, LR C.2)

```fortis
// reject: E-OWN-040
class Order {
  export id: Int
  export lines: List[Int]
}

test "lines" {
  let order = Order(id: 7, lines: List.of(1, 2, 3))
  let lines = move order.lines        // reject: E-OWN-040
  expect lines.count() == 3
}
```

```fortis
// accept
class Order {
  export id: Int
  export lines: List[Int]
}

test "lines" {
  let order = Order(id: 7, lines: List.of(1, 2, 3))
  let Order(id: id, lines: lines) = move order
  expect id == 7
  expect lines.count() == 3
}
```

### E-OWN-053: stored loan (LR 7.7, LR C.2)

```fortis
// reject: E-OWN-053
class Line {                          // reject: E-OWN-053
  export text: StringView
}

test "line" {
  expect 1 == 1
}
```

```fortis
// accept
class Line {
  export text: String
}

test "line" {
  let line = Line(text: "first")
  expect line.text == "first"
}
```

### E-OWN-090: `fn mut` into `Shared` (LR C.2)

```fortis
// reject: E-OWN-090, E-OWN-053
test "bump" {
  let mut count = 0
  let bump = fn mut[editable count]() Void {
    count = Int.saturatingAdd(count, 1)
  }
  let shared = Shared.create(move bump)     // reject: E-OWN-090, E-OWN-053
}
```

```fortis
// accept
test "bump" {
  let counter = Shared.create(AtomicInt.create(0))
  let handle = counter.retain()
  let bump = fn[move handle]() Void {
    discard handle.fetchAdd(1)
  }
  bump()
  bump()
  expect counter.load() == 2
}
```

### E-ASY-003: loan sent to a pool (LR 12.7, LR C.2)

```fortis
// reject: E-ASY-003
import fortis.host { Console }

async func total(items: ListView[Int]) Int {
  let mut sum = 0
  for x in items {
    sum = Int.saturatingAdd(sum, x)
  }
  return sum
}

async func main(console: Console) {
  let numbers = List.of(1, 2, 3)
  scope tasks {
    let t = tasks.launchParallel(total(numbers))   // reject: E-ASY-003
    let sum = await t
    console.print("${sum}")
  }
}
```

```fortis
// accept
import fortis.host { Console }

async func total(items: owned List[Int]) Int {
  let mut sum = 0
  for x in items {
    sum = Int.saturatingAdd(sum, x)
  }
  return sum
}

async func main(console: Console) {
  let numbers = List.of(1, 2, 3)
  scope tasks {
    let t = tasks.launchParallel(total(move numbers))
    let sum = await t
    console.print("${sum}")
  }
}
```

```output
6
```

### E-EFF-001: allocation in `noalloc` (LR 11.1, LR C.3)

The record is reported at the allocating statement.

```fortis
// reject: E-EFF-001
pure noalloc func grow(items: editable List[Int]) {
  items.push(1)                       // reject: E-EFF-001
}

test "grow" {
  expect 1 == 1
}
```

```fortis
// accept
pure func grow(items: editable List[Int]) {
  items.push(1)
}

pure noalloc func square(n: Int<-1000..1000>) Int => n * n

test "grow" {
  let mut items: List[Int] = List.default()
  grow(editable items)
  expect items.count() == 1
  expect square(12) == 144
}
```
