# Cleanup, async and concurrency (LR 12)

<!-- covers: LR 12 -->
*Agent mode. Human mode: [Cleanup, async and concurrency](../guide/12-async.md). Source of truth: LR 12.*

> Preview: async, cancellation, scopes and sharing (LR 12.3, 12.4, 12.5, 12.6 and 12.7) arrive in v0.3. `defer` and destruction order (LR 12.1, 12.2) are in v0.0.1.

## Rules

| Rule | Statement | LR |
|---|---|---|
| LANG-12-01 | A `defer` body is checked as if written at every exit of its block (after the block's own loans end, before its locals drop); it cannot alter the outcome; the places it names are initialised at every exit. | LR 12.1 |
| LANG-12-02 | Failures produced inside a `defer` are consumed there (`or recover`, `catch … recover`, `report`) and never replace the primary outcome; cleanup reports are labelled as such. | LR 12.1 |
| LANG-12-03 | `Future` and `Task` are linear; a pending future is never destroyed; cancellation is observed only at cancellation points. | LR 12.3, 12.4 |
| LANG-12-04 | A scope joins or cancels every task it launched before its block exits. | LR 12.5 |
| LANG-12-05 | Only `Shareable` environments without loans cross `launchParallel`/`launchBlocking`; loans cross only cooperative `launch` within the owner's block. | LR 12.5, 12.7 |
| LANG-12-06 | A `MutexGuard` is never live across `await`. | LR 12.3, 12.7 |

## Exit and destruction order (LR 12.2, LR 5.1, LR 7.9)

At every block exit, including cancellation and `or fail`:

1. the block's loans end; tasks launched on a `scope` declared in the block are cancelled and joined (their
   carried loans end) (LR 12.4, LR 12.5);
2. deferred blocks run, last registered first (LR 12.1);
3. during an unwind (cancellation or `fail`), `abandon` runs on every live `Abandonable` local, reverse
   initialisation order (LR 12.4);
4. owned locals drop, reverse initialisation order; a class value runs its `cleanup`, then drops its fields in
   declaration order (LR 7.9).

## `defer` constraints (LR 12.1)

- Block form only; no `return`, `fail`, `give`, `break`, `continue`, `await` inside (`E-DEF-001`); `defer await` is
  not in v1.
- Places it names must be initialised at every exit: no `move` of them between the `defer` and an exit
  (`E-DEF-002`).
- It may not use a binding of its own block that holds a loan (`E-OWN-052`); loans of enclosing blocks still apply.
- Its effects join the function's summary; a silent recovery inside it adds `recovers`.

## Async model (LR 12.3, LR 12.4)

- `async func f(…) T` call: evaluates arguments, returns `Future[T]` (`Future[Outcome[T]]` with `try`); the body
  starts at the first `await` or at `launch`.
- `await e`: only directly in an `async` body (not in a non-async lambda, not in `defer`).
- `Future[T]` is linear: `await`, launch, or `discard`. `discard` only for a never-polled future whose environment
  holds no linear value and no exclusive loan; it emits a `not_started` record.
- A future holds its arguments' loans (LR 7.3).
- Cancellation points: `await` (cooperative); `await` and `checkpoint()` (parallel and blocking pools). Cancellation
  is not a failure: not catchable, recoverable or matchable.
- No linear value live across a cancellation point unless it implements `Abandonable`
  (`func abandon(owned self)`: synchronous, non-failing, non-suspending). An `Abandonable` local may also be
  live at `or fail` (the unwind abandons it); a normal exit must still consume it. `Task` implements it.
- The futures and tasks being awaited are not "live across" their own `await`.

## Launch forms (LR 12.5)

| Form | Executor | Environment |
|---|---|---|
| `s.launch(f)` → `Task[T]` | same cooperative executor and thread | may borrow any binding declared in or before the scope |
| `s.launchParallel(f)` → `Task[T]` | CPU worker pool | `Shareable`, no loans; cancellation at `await` and `checkpoint()` |
| `s.launchBlocking(work)` → `Task[T]` | bounded blocking pool | `work: fn once() T`, `Shareable` environment; the only place `block` is allowed in async code; cancellation at `checkpoint()` |
| `s.detach(move task, supervisor)` | — | moves the join obligation to a `Supervisor`; removes the task from the scope |
| `join(a, b)` → `(A, B)` | polled inside the parent | consumes both futures; loans allowed; never returns early; failures stay in the tuple |

A `Task` is awaited inside its scope or detached. A scope exiting by `fail`, `return`, `break` or cancellation
cancels and joins its running tasks before its deferred blocks run; results never awaited are consumed by the
scope (a failure inside one is reported, labelled as abandoned by the scope) (LR 12.5).

`TaskCompletion[T]` = `Completed(value: T)` | `Cancelled(reason: Cancellation)` | `Defect(info: TaskDefect)`;
observed only by the supervisor of a detached task; linear when `T` is; consumed with `match` (LR 12.6).

## Shareable (LR 12.7)

- Shareable: trivial types; `String`, `Bytes`, `BigInt`; `List`, `Array`, `Map`, `OrderedMap`, `Set`,
  `FixedList`, `FixedMap` of shareables; classes, enums, tuples and records whose fields are all shareable;
  `Shared[T]`, `Weak[T]`, `Mutex[T]` with `T: Shareable`; `AtomicInt`, `AtomicBool`; `fn` / `fn once` closures
  capturing only `copy`/`move` of shareables; capability values.
- Not shareable: loans and views, `MutexGuard`, `fn mut` closures, `Future`, `Task`, thread-bound resources.

`List.of("a", "b")` would infer `List[StringView]`, a stored loan rejected with `E-OWN-053`; annotate
`let words: List[String] = …` to make owned, shareable strings (LR 3.5, LR 7.7, LR 12.7).

Mutation between tasks: `Mutex[T]`. Async: `await m.lock()`; sync: `m.lockBlocking()` (`block`). Both return
`Outcome[MutexGuard[T]] from mutex`; the guard is an exclusive loan of `T`, not `Shareable`, unlocks when
destroyed, and cannot cross `await`. Atomics: `AtomicInt`, `AtomicBool` (LR 12.7).

## Diagnostics

| Code | Trigger | Correction | LR |
|---|---|---|---|
| E-DEF-001 | `return`, `fail`, `give`, `break`, `continue`, `await` (including `or fail`) inside `defer` | consume failures with `catch … recover`, `or recover` or `report`; move control flow out of the `defer` | LR 12.1 |
| E-DEF-002 | a place the `defer` names is moved out before an exit | keep it owned until every exit (lend it instead of moving it), or do the deferred work before the move and drop the `defer`; registering the `defer` later does not help, since the place is still moved at the exit | LR 12.1 |
| E-OWN-052 | a `defer` uses a loan-holding binding of its own block | use the owner (or a parameter), or copy the value into a non-loan binding | LR 12.1 |
| E-OBL-001 | a future, task or outcome dropped at block exit; an outcome unconsumed inside `defer` | `await` it, launch it, or consume it | LR 6.12, 12.1, 12.3 |
| E-OBL-005 | a linear value live across `await` whose type is not `Abandonable` | consume it before the `await` (`let v = e or fail`), or pass it into the awaited call | LR 12.4 |
| E-ASY-001 | a `Task` leaves its scope block (`return`, `give`, assignment to an outer binding) | `await` it inside the scope, or `detach` it to a `Supervisor` | LR 12.3 |
| E-ASY-002 | a `MutexGuard` live across `await` | end the guard's block before the `await` | LR 12.3 |
| E-ASY-003 | a pool launch whose future arguments or closure captures include a loan or a non-`Shareable` value | pass owned shareable data (`copy`/`move`); use cooperative `launch` for borrowed data | LR 12.7 |
| E-EFF-020 | `block` inside an `async` function or anything it calls | `await m.lock()`, or `launchBlocking` | LR 11.1 |

## Correction patterns

### E-DEF-001: `or fail` inside `defer` (LR 12.1)

```fortis
// reject: E-DEF-001
export error FlushFailed {
  code: "demo.flush"
}

try func flush() {
  fail FlushFailed()
}

try func work(n: Int) Int {
  defer {
    flush() or fail                   // reject: E-DEF-001
  }
  return n
}

test "work" {
  expect work(1) == 1
}
```

```fortis
// accept
export error FlushFailed {
  code: "demo.flush"
}

try func flush() {
  fail FlushFailed()
}

func work(n: Int) Int {
  defer {
    flush() catch e {
      report e
      recover Void
    }
  }
  return n
}

test "work" {
  expect work(1) == 1
}
```

### E-DEF-002: a deferred place moved away (LR 12.1)

```fortis
// reject: E-DEF-002
export class Handle {
  export open: Bool
  export func close(editable self) {
    self.open = false
  }
}

func keep(h: owned Handle) {
}

func run() {
  let mut input = Handle(open: true)
  defer { input.close() }
  keep(move input)                    // reject: E-DEF-002
}

test "run" {
  run()
  expect true
}
```

```fortis
// accept
export class Handle {
  export open: Bool
  export func close(editable self) {
    self.open = false
  }
}

func keep(h: owned Handle) {
}

func run() {
  let mut input = Handle(open: true)
  input.close()
  keep(move input)
}

test "run" {
  run()
  expect true
}
```

### E-OWN-052: a loan binding used in `defer` (LR 12.1)

```fortis
// reject: E-OWN-052
export class Buffer {
  export used: Int
  export func peek(self) alias Buffer from self => alias self
}

func run(items: Buffer) Int {
  let seen = items.peek()
  defer { discard seen.used }         // reject: E-OWN-052
  return seen.used
}

test "run" {
  expect run(Buffer(used: 1)) == 1
}
```

```fortis
// accept
export class Buffer {
  export used: Int
  export func peek(self) alias Buffer from self => alias self
}

func run(items: Buffer) Int {
  defer { discard items.used }
  let seen = items.peek()
  return seen.used
}

test "run" {
  expect run(Buffer(used: 1)) == 1
}
```

### E-OBL-001: a future dropped (LR 6.12, LR 12.3)

```fortis
// reject: E-OBL-001
async func work(n: Int) Int => n

async func run(n: Int) Int {
  let pending = work(n)               // reject: E-OBL-001
  return n
}

async func main() {
  discard await run(1)
}
```

```fortis
// accept
async func work(n: Int) Int => n

async func run(n: Int) Int {
  let pending = work(n)
  return await pending
}

async func main() {
  discard await run(1)
}
```

### E-OBL-005: a linear value across `await` (LR 12.4)

```fortis
// reject: E-OBL-005
export error Negative {
  code: "demo.negative"
}

try func parse(n: Int) Int {
  if n < 0 {
    fail Negative()
  }
  return n
}

async func pause() {
}

async try func run(n: Int) Int {
  let pending = parse(n)
  await pause()                       // reject: E-OBL-005
  return pending or fail
}

async try func main() {
  discard await run(3) or fail
}
```

```fortis
// accept
export error Negative {
  code: "demo.negative"
}

try func parse(n: Int) Int {
  if n < 0 {
    fail Negative()
  }
  return n
}

async func pause() {
}

async try func run(n: Int) Int {
  let value = parse(n) or fail
  await pause()
  return value
}

async try func main() {
  discard await run(3) or fail
}
```

### E-ASY-001: a task escaping its scope (LR 12.3)

```fortis
// reject: E-ASY-001
async func work(n: Int) Int => n

async func run(n: Int) Task[Int] {
  scope tasks {
    let t = tasks.launch(work(n))
    return move t                     // reject: E-ASY-001
  }
}

async func main() {
}
```

```fortis
// accept
async func work(n: Int) Int => n

async func run(n: Int) Int {
  scope tasks {
    let t = tasks.launch(work(n))
    return await t
  }
}

async func main() {
  discard await run(2)
}
```

### E-ASY-002: a guard across `await` (LR 12.3)

```fortis
// reject: E-ASY-002
async func pause() {
}

async try func run(m: Mutex[Int]) Int {
  let g = await m.lock() or fail
  await pause()                       // reject: E-ASY-002
  return 0
}

async try func main() {
  let m = Mutex.create(1)
  discard await run(m) or fail
}
```

```fortis
// accept
async func pause() {
}

async try func run(m: Mutex[Int]) Int {
  {
    let g = await m.lock() or fail
  }
  await pause()
  return 0
}

async try func main() {
  let m = Mutex.create(1)
  discard await run(m) or fail
}
```

### E-ASY-003: a non-`Shareable` capture crossing a pool launch (LR 12.7)

```fortis
// reject: E-ASY-003
async func pools() Int {
  let words: StringView = "abc"
  scope tasks {
    let blocking = tasks.launchBlocking(fn once[move words]() Int => words.scalarCount())   // reject: E-ASY-003
    return await blocking
  }
}

async func main() {
  discard await pools()
}
```

```fortis
// accept
async func pools() Int {
  let words: List[String] = List.of("a", "b", "c")
  scope tasks {
    let blocking = tasks.launchBlocking(fn once[move words]() Int => words.count())
    return await blocking
  }
}

async func main() {
  discard await pools()
}
```

## Facts used when writing code

- `main` may be `async func main` or `async try func main`; it runs as the root task on the core cooperative
  executor (LR 4.6).
- `await first or fail` leaving a scope cancels the other tasks of that scope; a task cancelled before its first
  poll never runs (LR 12.5).
- The v1 executor runs every cooperative task of one scope tree on one thread; `launch` never needs `Shareable`
  (LR 12.5).
- `Shared[T]` gives read access only; mutation goes through `Mutex[T]` or an atomic (LR 7.8, LR 12.7).
- No cross-backend agreement is claimed for interleaving timing; write output after `await`s in a fixed order
  (LR 12.7).
