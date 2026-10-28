# Appendix C: Lowering to Rust

<!-- covers: LR C -->
*Human mode. Agent mode: [Lowering reference, by shape](../agent/appendix-c-lowering.md). Source of truth: LR C.*

Fortis has two execution paths that share one semantics: the development VM runs the checked intermediate
representation directly, and the production backend turns it into safe Rust, a crate marked
`#![forbid(unsafe_code)]`, which `rustc` then compiles (LR 1, LR 7.10). This appendix explains how Fortis
constructs map onto Rust, which checks Rust's own compiler repeats and which only the Fortis frontend can make,
and what the language rejects because no safe lowering exists. You do not need any of it to write Fortis; it
explains why some ownership rules are shaped the way they are.

## Every construct has a shape

The ownership rules are chosen so that every program the checker accepts has a documented lowering to safe Rust
(LR 7.10). The table in LR C.1 lists one shape per construct. A few rows show the pattern (LR C.1):

| Fortis | Rust |
|---|---|
| a `type` newtype | a struct with one private field |
| a class | a `struct` and an `impl` block |
| a trivial type | a `Copy` struct or enum |
| a read loan, an exclusive loan | `&T`, `&mut T` |
| `copy p` | a call of a generated `Clone` implementation |
| a `from` return | `fn f<'a>(x: &'a T) -> &'a U` |
| `Shared[T]`, `Weak[T]` | `Arc<T>`, `Weak<T>` |
| an interface, `dynamic I` | a trait with generics; `Box<dyn I>`, `&dyn I` or `Arc<dyn I>` |
| a lambda | a closure with a concrete environment (`Fn`, `FnMut` or `FnOnce`) |
| a `try func` | a function returning `Result<T, Failure>` |
| `T?` | `Option<T>` |
| `defer` | its body inlined at every exit path, not a `Drop` |
| `cleanup` | an `impl Drop` that calls the method |
| an `async func`, a `scope` | an `async fn` state machine; a join set polled by the parent future |

Two consequences are worth knowing. First, the checker never inserts a clone, a reference count or hidden sharing
to make a rejected program acceptable (LANG-07-03, LR 7.10): an ownership-bearing value is duplicated where the
source asks for it, by a `copy` (lowered to a generated `Clone`, LR C.1) or by an operation whose defined meaning
includes copying, such as `filter` (LR 6.7), while trivial values copy implicitly (LR 6.11) and `retain` makes
another handle to shared data (LR 7.8).
Second, a `defer` block is plain code at each exit, which is why a deferred block can use the places it names
freely between its registration and the exit (LR 12.1, LR C.1).

## Who checks what

Each row of LR C.1 names which checker is authoritative (LR C.1).

**Rust-checked** properties are ones `rustc` verifies again on the generated crate: loan validity, lifetimes,
use after move, `Send` and `Sync`, trait bounds and exhaustiveness. If `rustc` rejects generated code that the
Fortis frontend accepted, and the construct is in this Rust-checked fragment, that is a compiler defect
(LR 7.10, LR C.1).

**Frontend-checked** properties have no Rust type: linear obligations (an `Outcome` must be consumed), the closed
effect set, `noalloc`, closure kinds against the captures the body needs, observation definite assignment. That
`rustc` accepts the generated code says nothing about them; the IR validator and the restricted Rust subset are
the controls (LR C.1). For loans the frontend is the stricter of the two: a Fortis loan ends at the end of its
block, not at its last use (LR 7.3, LR C.1).

## Shapes with no lowering

A handful of ownership situations can be described but have no safe lowering. They are rejected before
lowering, each with the alternative the LR names (LR C.2). Where an earlier rule already excludes a shape, that
rule's more specific code is used; `E-OWN-090` is for shapes only the lowering can identify (LR C.2).

| Shape | Code | Write instead | LR |
|---|---|---|---|
| a loan returned `from` two parameters | `E-SYN-000` (a loan type names one origin) | return an owned value, or split the function | LR C.2, LR 7.6 |
| a class that stores a loan | `E-OWN-053` | own the value, copy it, hold a `Shared`, or pass it per call | LR C.2, LR 7.7 |
| a self-referential class | `E-OWN-053` | indices, or `Shared` with `Weak` | LR C.2, LR 7.7 |
| a loan stored in `Shared` or `Mutex` | `E-OWN-090` | own the value | LR C.2 |
| a `fn mut` closure escaping into a `Shared` | `E-OWN-090` | a `Mutex` around the state and a `fn` closure | LR C.2 |
| a future holding a loan launched on a parallel pool | `E-ASY-003` | pass owned data | LR C.2, LR 12.7 |
| a partial move | `E-OWN-040` | move the whole value, or restructure | LR C.2, LR 7.2 |

No shape is ever lowered with `unsafe` or by erasing a lifetime (LR 7.10, LR C.2).

### A partial move

Moving one field out of a value that still owns the others would leave a half-owned value. It is rejected with
`E-OWN-040` (LR 7.2, LR C.2):

```fortis
// reject: E-OWN-040
class Order {
  export id: Int
  export lines: List[Int]
}

export func main(args: List[String]) Void {
  let order = Order(id: 7, lines: List.of(1, 2, 3))
  let lines = move order.lines        // reject: E-OWN-040
}
```

Consuming the whole value with a destructuring `let` takes every field at once, so nothing is left half-owned
(LR 9.8):

```fortis
// accept
import fortis.host { Host }

class Order {
  export id: Int
  export lines: List[Int]
}

export func main(args: List[String]) Void {
  let caps = Host.capabilities()
  let order = Order(id: 7, lines: List.of(1, 2, 3))
  let Order(id: id, lines: lines) = move order
  caps.console.print("order ${id} has ${lines.count()} lines")
}
```

```output
order 7 has 3 lines
```

### A stored loan

A field may not hold a loan or a view: stored loans are not in Language 1.0, so a class with a `StringView`
field is `E-OWN-053` (LR 7.7, LR C.2). The one exception is a loan `from static`, which borrows from no owner, such
as a field of type `StringView from static` holding a string literal (LR 7.7):

```fortis
// reject: E-OWN-053
class Line {                          // reject: E-OWN-053
  export number: Int
  export text: StringView
}

export func main(args: List[String]) Void {
}
```

The class owns its text instead (LR 7.7):

```fortis
// accept
import fortis.host { Host }

class Line {
  export number: Int
  export text: String
}

export func main(args: List[String]) Void {
  let caps = Host.capabilities()
  let line = Line(number: 1, text: "first line")
  caps.console.print("${line.number}: ${line.text}")
}
```

```output
1: first line
```

### A `fn mut` closure inside a `Shared`

A `fn mut` closure holds an exclusive loan or mutable state, and a `Shared` hands out only read access to what it
holds, so the combination has no lowering: `E-OWN-090` (LR 7.8, LR C.2):

```fortis
// reject: E-OWN-090
export func main(args: List[String]) Void {
  let mut count = 0
  let bump = fn mut[editable count]() Void {
    count = Int.saturatingAdd(count, 1)
  }
  let shared = Shared.create(move bump)     // reject: E-OWN-090
}
```

The LR's alternative puts the state behind something designed for shared mutation and uses a plain `fn` closure
(LR C.2). Here the state is an `AtomicInt` held in a `Shared`, and the closure owns its own handle (LR 7.8,
LR 12.7):

```fortis
// accept
import fortis.host { Host }

export func main(args: List[String]) Void {
  let caps = Host.capabilities()
  let counter = Shared.create(AtomicInt.create(0))
  let handle = counter.retain()
  let bump = fn[move handle]() Void {
    discard handle.fetchAdd(1)
  }
  bump()
  bump()
  caps.console.print("count ${counter.load()}")
}
```

```output
count 2
```

### A loan sent to a parallel pool

A task on the parallel pool runs on a worker thread, so the future it runs must hold no loans and everything it
carries must be `Shareable`; a future that borrows is `E-ASY-003` (LR 12.5, LR 12.7, LR C.2).

```fortis
// reject: E-ASY-003
import fortis.host { Host }

async func total(items: ListView[Int]) Int {
  let mut sum = 0
  for x in items {
    sum = Int.saturatingAdd(sum, x)
  }
  return sum
}

export async func main(args: List[String]) Void {
  let caps = Host.capabilities()
  let numbers = List.of(1, 2, 3)
  scope tasks {
    let t = tasks.launchParallel(total(numbers))   // reject: E-ASY-003
    let sum = await t
    caps.console.print("${sum}")
  }
}
```

Pass the task owned data instead (LR 12.5, LR C.2):

```fortis
// accept
import fortis.host { Host }

async func total(items: owned List[Int]) Int {
  let mut sum = 0
  for x in items {
    sum = Int.saturatingAdd(sum, x)
  }
  return sum
}

export async func main(args: List[String]) Void {
  let caps = Host.capabilities()
  let numbers = List.of(1, 2, 3)
  scope tasks {
    let t = tasks.launchParallel(total(move numbers))
    let sum = await t
    caps.console.print("${sum}")
  }
}
```

```output
6
```

## What counts as allocation

`noalloc` is decided by matching the lowering against a fixed list of allocation-free patterns, not by looking at
what the optimiser produced (LR 11.4, LR C.3). The allocation-free patterns are arithmetic and comparison; field
and array access; building trivial and inline values (`Array`, `FixedList`, `FixedMap`, a bounded `Failure`) and
any class or record from values that are trivial, moved, freshly built, returned by a call or retained; read and
exclusive loans; calls to functions whose summary has no `alloc`; `match`; `defer`; and `capture` into a frame of
bounded size (LR C.3). Everything else allocates: growing a `List`, `String` or `Map`, `Shared.create`, creating
a `dynamic` value, `copy` of a heap-owning value, boxed futures from `dynamic` async calls, extended failure
payloads, and launches on the pools. In Language 1.0 a cooperative `launch` allocates too, because the runtime
host declares no inline task storage; `join(a, b)` does not (LR C.3).

So arithmetic on proved integers is `noalloc`, and pushing onto a list is not (LR 11.1, LR C.3):

```fortis
// reject: E-EFF-001
pure noalloc func grow(items: editable List[Int]) Void {
  editable items.push(1)              // reject: E-EFF-001
}

export func main(args: List[String]) Void {
}
```

```fortis
// accept
import fortis.host { Host }

pure noalloc func square(n: Int<-1000..1000>) Int => n * n

export func main(args: List[String]) Void {
  let caps = Host.capabilities()
  caps.console.print("${square(12)}")
}
```

```output
144
```
