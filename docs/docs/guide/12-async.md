# Cleanup, async and concurrency

<!-- covers: LR 12 -->
*Human mode. Agent mode: [Cleanup, async and concurrency, by rule](../agent/12-async.md). Source of truth: LR 12.*

> Preview: async, cancellation, scopes and sharing (LR 12.3, 12.4, 12.5, 12.6 and 12.7) arrive in v0.3. `defer` and destruction order (LR 12.1, 12.2) are in v0.0.1.

This chapter is about time: what runs when a block ends, in what order values are destroyed, and how a program
does several things at once. Fortis keeps all three visible. Cleanup is a `defer` block you can read, async work
is a `Future` you must `await` or launch, concurrent tasks live inside a `scope` block that cannot end while they
run, and data shared between threads is either immutable or behind a `Mutex`.

## `defer`

`defer { … }` registers a block that runs when the enclosing block exits, by any route: falling off the end,
`return`, `fail`, `break`, `continue`, or cancellation (LR 12.1). Deferred blocks run in reverse order of
registration, before the block's local values are dropped. There is only the block form.

The classic use is closing what you opened, right next to where you opened it:

```fortis
// accept
import fortis.host { FileSystem, Path, Console }

try func copyFile(fs: FileSystem, source: Path, target: Path) {
  let mut input = fs.open(source) or fail
  defer { input.close() }
  let mut output = fs.create(target) or fail
  defer { output.close() }
  let data = input.readAll() or fail
  output.writeAll(data) or fail
}

try func main(fileSystem: FileSystem, console: Console) {
  let source = Path.parse("source.txt") or fail
  let target = Path.parse("copy.txt") or fail
  fileSystem.writeAll(source, "copied by Fortis") or fail
  copyFile(fileSystem, source, target) or fail
  let text = fileSystem.readText(target) or fail
  console.print(text)
}
```

```output
copied by Fortis
```

Whichever `or fail` leaves `copyFile`, every file opened before it is closed, and nothing else is. A `defer` is
checked as if its body were written at every exit of its block (LR 12.1): after the loans held by the block's own
bindings have ended and before its locals are dropped. That is why the places it names must still be there at
every exit. Moving one of them away between the `defer` and an exit is `E-DEF-002` (LR 12.1):

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

func main() {
  let mut input = Handle(open: true)
  defer { input.close() }
  keep(move input)                    // reject: E-DEF-002
}
```

A deferred block cannot change how the function ends: `return`, `fail`, `give`, `break`, `continue` and `await`
are not allowed inside it (`E-DEF-001`, LR 12.1). There is no outcome for it to propagate to, so a failure
produced inside it is consumed there, with `or recover`, `catch … recover` or `report`; `or fail` is the same
`E-DEF-001` (LR 12.1):

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

try func main() {
  discard work(1) or fail
}
```

```fortis
// accept
import fortis.host { Console }

export error FlushFailed {
  code: "demo.flush"
}

try func flush() {
  fail FlushFailed()
}

func work(console: Console) {
  defer {
    flush() catch e {
      report e
      recover Void
    }
  }
  console.print("work")
}

func main(console: Console) {
  work(console)
  console.print("done")
}
```

```output
work
done
```

The failure is reported, labelled as raised during cleanup, on standard error (LR 12.1). Recovering a cleanup
failure without reporting it is allowed too, and adds `recovers` to the function (LR 12.1).

Because the block's own loans have ended by the time a deferred block runs, it may not use a binding of its
block that holds a loan: `E-OWN-052` (LR 12.1).

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

func main() {
  discard run(Buffer(used: 1))
}
```

`defer` can be this flexible because it lowers to explicit cleanup code at each exit, not to a closure or a
destructor, so no loan has to be held across the block (LR 12.1). `defer await` is not in Language 1.0;
asynchronous cleanup belongs to the owning `scope` or to explicit `await`s before the block ends (LR 12.1).

## Destruction order

At a block exit, the order is always the same (LR 12.2, LR 7.9): the block's loans end; its deferred blocks run,
last registered first; its owned locals are dropped, last initialised first; and a class value that is dropped
runs its own `cleanup` method before its fields are dropped. The order does not depend on how the block was left.

```fortis
// accept
import fortis.host { Console }

export class Noisy {
  export name: String
  export console: Shared[Console]
  cleanup(editable self) {
    self.console.print("drop ${self.name}")
  }
}

func steps(console: Shared[Console], stop: Bool) {
  let first = Noisy(name: "first", console: console.retain())
  defer {
    console.print("defer one")
  }
  let second = Noisy(name: "second", console: console.retain())
  defer {
    console.print("defer two")
  }
  if stop {
    console.print("early return")
    return
  }
  console.print("end of body")
}

func main(out: Console) {
  let console = Shared.create(move out)
  steps(console.retain(), false)
  steps(console.retain(), true)
}
```

```output
end of body
defer two
defer one
drop second
drop first
early return
defer two
defer one
drop second
drop first
```

The order is the same on every backend and under every kind of exit, including cancellation at an `await`
(LR 12.2).

## Async functions and futures

Calling an `async func` does not run it. The call evaluates the arguments and produces a `Future[T]` (or
`Future[Outcome[T]]` for an `async try` function); the body starts when the future is awaited or launched on a
scope (LR 12.3). `await` suspends the current task until the future completes and gives its value. An `async`
program starts from `async func main` or `async try func main`, which runs as the root task on the core
cooperative executor (LR 4.6).

```fortis
// accept
import fortis.host { Console }

async func work(console: Console, n: Int) Int {
  console.print("work ${n} runs")
  return n
}

async func main(console: Console) {
  let pending = work(console, 1)
  console.print("future built")
  let value = await pending
  console.print("got ${value}")
}
```

```output
future built
work 1 runs
got 1
```

An async call looks like a call; the `await` (or the `launch`) is the visible point where the work happens
(LR 1.1). `await` may appear only directly in an `async` function's body, not in a lambda that is not itself
`async` and not in a `defer` (LR 12.3).

A `Future` is linear: it must be awaited, launched on a scope, or discarded (LR 12.3). Dropping one at the end of
a block is `E-OBL-001`, like dropping any linear value (LR 6.12):

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

`discard` of a future that was never polled is the one exception to the rule that a linear value cannot be
discarded: it is allowed when the future's captured environment holds no linear value and no exclusive loan, and
it writes a `not_started` record to the task's reporter (LR 12.3, LR 5.5). A future holds the loans of its
arguments, exactly like a value returned `from` them, so it cannot outlive the places it borrows (LR 7.3, LR 12.3).

## Cancellation

Cancellation is a signal delivered to a task, observed only at a **cancellation point**: an `await` on the
cooperative executor, and an `await` or an explicit `checkpoint()` on the parallel and blocking pools (LR 12.4).
It is not a failure: nothing can catch, recover or match it. A task that observes it unwinds to its boundary,
running deferred blocks on the way, and completes as cancelled. Synchronous code always runs to completion.

Unwinding runs no user code except `defer` blocks and `abandon` methods, so a linear value that is still alive at
an `await` would have nobody to consume it if the task were cancelled there. The rule is therefore that no linear
value may be live across a cancellation point unless its type implements `Abandonable` (`E-OBL-005`, LR 12.4):

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

Consume the outcome before the `await` (`let value = parse(n) or fail`), pass it into the awaited call, or
restructure (LR 12.4).

`interface Abandonable { func abandon(owned self) }` is the escape hatch: its `abandon` is synchronous, cannot
fail and cannot suspend, and the unwind calls it on every live `Abandonable` local in reverse initialisation
order, after the deferred blocks (LR 12.4). Unwinding means leaving by cancellation *or by `fail`*, so an
`Abandonable` local is also the one linear value that may be live at an `or fail`; a normal exit must still
consume it explicitly. This is exactly what a transaction wants: failure rolls back, success must commit.

```fortis
// accept (exit status 1)
import fortis.host { Console }

export error Odd {
  code: "demo.odd"
}

export class Tx implements Abandonable {
  witness: Must
  export console: Shared[Console]
  export func begin(console: Shared[Console]) Tx {
    console.print("begin")
    return Tx(witness: Must.create(), console: console.retain())
  }
  export func commit(owned self) {
    self.console.print("commit")
    let Tx(witness: w, console: c) = move self
    Must.discharge(move w)
  }
  func abandon(owned self) {
    self.console.print("abandon")
    let Tx(witness: w, console: c) = move self
    Must.discharge(move w)
  }
}

try func step(console: Shared[Console], n: Int) Int {
  console.print("step ${n}")
  if n % 2 == 1 {
    fail Odd()
  }
  return n
}

try func work(console: Shared[Console], n: Int) Int {
  let tx = Tx.begin(console)
  let v = step(console, n) or fail
  move tx.commit()
  return v
}

try func main(out: Console) {
  let console = Shared.create(move out)
  discard work(console, 2) or fail
  discard work(console, 1) or fail
}
```

```output
begin
step 2
commit
begin
step 1
abandon
```

The `Must` field makes `Tx` linear (LR 6.12): the only ways to get rid of one are `commit` and the unwind's
`abandon`. The second call fails, the failure leaves `main`, and the program ends with status 1 and one report of
the failure on standard error (LR 4.6).

## Structured scopes and `launch`

Concurrency in Fortis lives inside a block. `scope name { … }` is a block that owns tasks, and `name.launch(f)`
consumes a future and starts it as a `Task[T]` (LR 12.5). A task is linear: it is awaited inside the scope, or
handed to a `Supervisor` with `name.detach(move task, supervisor)`. A scope never lets a task outlive it.

```fortis
// accept
import fortis.host { Console }

async func measure(text: StringView) Int => text.scalarCount()

async func measureBoth(a: StringView, b: StringView) (Int, Int) {
  scope tasks {
    let first = tasks.launch(measure(a))
    let second = tasks.launch(measure(b))
    return (await first, await second)
  }
}

async func main(console: Console) {
  let left = "cooperative"
  let right = "tasks"
  let pair = await measureBoth(left, right)
  console.print("${pair.0} and ${pair.1}")
}
```

```output
11 and 5
```

The futures borrow `a` and `b`. That is allowed for cooperative `launch` because the scope joins its tasks before
any binding declared in or before it is dropped (LR 12.5).

When the scope's block exits by `fail`, `return`, `break` or cancellation while tasks are still running, it
cancels them and waits for each to finish, as part of ending the block's loans and before its deferred blocks run
(LR 12.5). So `await first or fail` leaving the block cancels `second`:

```fortis
// accept
import fortis.host { Console }

export error Bad {
  code: "demo.bad"
}

async try func quick(console: Console) Int {
  console.print("quick")
  fail Bad()
}

async func late(console: Console) Int {
  console.print("late ran")
  return 1
}

async try func run(console: Console) Int {
  scope tasks {
    let first = tasks.launch(quick(console))
    let second = tasks.launch(late(console))
    let x = await first or fail
    return Int.wrappingAdd(x, await second)
  }
}

async recovers func main(console: Console) {
  let r = await run(console) or recover -1
  console.print("recovered ${r}")
}
```

```output
quick
recovered -1
```

`late` was cancelled before it ever ran, so its line never appears. A task cannot leave its scope, by `return`,
`give` or assignment to an outer binding: `E-ASY-001` (LR 12.3):

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

### Three ways to launch

| Form | Runs on | Requirements (LR 12.5) |
|---|---|---|
| `s.launch(f)` | the same cooperative executor and thread | the future may borrow any binding declared in or before the scope |
| `s.launchParallel(f)` | the CPU worker pool | the future's environment is `Shareable` and holds no loans |
| `s.launchBlocking(work)` | the bounded blocking pool | `work` is a synchronous `fn once() T` whose environment is `Shareable`; the only place `block` is allowed inside async code |

```fortis
// accept
import fortis.host { Console }

async func sum(items: owned List[Int]) Int {
  let mut total = 0
  for n in items {
    total = Int.saturatingAdd(total, n)
  }
  return total
}

async func pools() (Int, Int) {
  let numbers = List.of(1, 2, 3, 4)
  let words: List[String] = List.of("a", "b", "c")
  scope tasks {
    let parallel = tasks.launchParallel(sum(copy numbers))
    let blocking = tasks.launchBlocking(fn once[move words]() Int => words.count())
    return (await parallel, await blocking)
  }
}

async func main(console: Console) {
  let counts = await pools()
  console.print("sum ${counts.0}, words ${counts.1}")
}
```

```output
sum 10, words 3
```

The values cross to the pools by `copy` and `move`. A loan, or a value that is not `Shareable`, cannot cross:
`E-ASY-003` (LR 12.7). Without the annotation, `List.of("a", "b", "c")` would be a list of `StringView`s, views of
static text, and views are never `Shareable`:

```fortis
// reject: E-ASY-003
async func pools() Int {
  let words = List.of("a", "b", "c")
  scope tasks {
    let blocking = tasks.launchBlocking(fn once[move words]() Int => words.count())   // reject: E-ASY-003
    return await blocking
  }
}

async func main() {
  discard await pools()
}
```

The v1 executor runs every cooperative task of one scope tree on one thread, which is why `launch` never needs
`Shareable` (LR 12.5).

### Detaching a task

`detach` transfers a task's join obligation to the `Supervisor` capability and removes it from the scope
(LR 12.5). The scope then no longer waits for it, and a supervisor observes how it ends through a
`TaskCompletion` (LR 12.6). The core does not say when a detached task runs relative to the end of the program,
and the completion of an `async main` ends the process (LR 4.6), so the example below shows only the hand-over:

```fortis
// accept
import fortis.host { Console, Supervisor }

async func work() Int {
  return 1
}

async func run(console: Console, supervisor: Supervisor) {
  scope tasks {
    let t = tasks.launch(work())
    tasks.detach(move t, supervisor)
    console.print("detached one task")
  }
}

async func main(console: Console, supervisor: Supervisor) {
  await run(console, supervisor)
  console.print("done")
}
```

```output
detached one task
done
```

### `join` without a scope

`join(a, b)` consumes two futures, polls both inside the current task and returns `(A, B)` once both have
completed (LR 12.5). It accepts futures that hold loans and never returns early: a failure produced by one future
is simply part of the tuple, so the caller consumes both outcomes.

```fortis
// accept
import fortis.host { Console }

export error Bad {
  code: "demo.bad"
}

async try func left(console: Console) Int {
  console.print("left")
  fail Bad()
}

async func right(console: Console) Int {
  console.print("right")
  return 2
}

async recovers func run(console: Console) {
  let pair = await join(left(console), right(console))
  let (a, b) = move pair
  let text = match move a {
    success(v) => "left ${v}"
    failure(f) => match move f {
      is Bad => "left failed"
      otherwise(other) => {
        report other
        give "left failed differently"
      }
    }
  }
  console.print("${text}, right ${b}")
}

async recovers func main(console: Console) {
  await run(console)
}
```

```output
left
right
left failed, right 2
```

## Task completion

A child task cannot complete as cancelled or defective without the same event reaching its parent: cancelling a
scope cancels every task in it, and a defect in any task terminates the whole scope tree up to its supervisor
(LR 12.5). So `await task` simply gives the body's result. Only the supervisor of a *detached* task sees how it
ended, as a `TaskCompletion[T]` with three variants (LR 12.6):

| Variant | Meaning |
|---|---|
| `Completed(value: T)` | the body's result, which may itself be an `Outcome` |
| `Cancelled(reason: Cancellation)` | the task was cancelled |
| `Defect(info: TaskDefect)` | the task was terminated by a defect (LR 10.8) |

`Cancelled` and `Defect` are never seen as a `Failure`. `TaskCompletion[T]` is linear when `T` is, and a
supervisor consumes it with `match` (LR 12.6).

## Sharing data between tasks

A value may cross `launchParallel` or `launchBlocking` only if its type is **`Shareable`**, a structural
property (LR 12.7). These types are `Shareable`:

- trivial types, `String`, `Bytes` and `BigInt`;
- `List`, `Array`, `Map`, `OrderedMap`, `Set`, `FixedList` and `FixedMap` of shareable elements;
- classes, enums, tuples and records whose fields are all shareable;
- `Shared[T]`, `Weak[T]` and `Mutex[T]` where `T` is shareable, and `AtomicInt` and `AtomicBool`;
- `fn` and `fn once` closures whose captures are all `copy` or `move` of shareable values;
- capability values.

Loans and views, `MutexGuard`, `fn mut` closures, `Future`, `Task` and resources that declare themselves
thread-bound are not (LR 12.7).

Shared data is immutable unless it sits behind a `Mutex[T]` or is an atomic (LR 7.8, LR 12.7). In async code
`await m.lock()` suspends until the lock is free; in synchronous code, including `launchBlocking` bodies,
`m.lockBlocking()` blocks the thread, which is the `block` effect (LR 12.7). Both return an outcome holding a
`MutexGuard[T]`, an exclusive loan that unlocks when it is destroyed. A guard cannot be held across an `await`:
`E-ASY-002` (LR 12.3).

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

End the guard's block before suspending:

```fortis
// accept
import fortis.host { Console }

async func pause() {
}

async try func run(m: Mutex[Int]) Int {
  {
    let g = await m.lock() or fail
  }
  await pause()
  return 0
}

async try func main(console: Console) {
  let m = Mutex.create(1)
  let r = await run(m) or fail
  console.print("released and resumed: ${r}")
}
```

```output
released and resumed: 0
```

`AtomicInt` and `AtomicBool` give counters and flags that tasks can share without a `Mutex` (LR 12.7):

```fortis
// accept
import fortis.host { Console }

func main(console: Console) {
  let counter = AtomicInt.create(5)
  let previous = counter.fetchAdd(2)
  console.print("${previous} then ${counter.load()}")
  let flag = AtomicBool.create(true)
  flag.store(false)
  console.print("${flag.load()}")
}
```

```output
5 then 7
false
```

Because an exclusive loan is never shared and shared data is immutable unless it is behind a `Mutex` or an
atomic, Fortis programs are free of data races by construction, on both the VM and native code. What the
language does not promise is that two backends interleave tasks with the same timing (LR 12.7).
