# Worked examples

<!-- covers: LR 2 -->
*Human mode. Each program is shown whole, with its output, and then explained piece by piece. Source of truth:
the LR; the [tour](tour.md) has the worked module of LR 2.*

> Preview: the concurrent example uses async, scopes and join (LR 12.3, LR 12.5), which arrive in v0.3. The ledger's newtype (LR 6.9) is in v0.1.

The chapters each show one construct at a time. Real programs use several at once, and the interesting part is
how they fit together: who owns what, where a failure goes, which line is allowed to change which value. These
four programs are the size of a first real task. Every one of them is checked and run by the toolchain, like
every example in this guide (LR 0).

## A word tally

The first program counts the words of a text, keeps them in the order it first saw them, and reports a few
things about the counts. It uses a class with private state, a `Map`, loops, and closures of two kinds.

```fortis
// accept
import fortis.host { Console }
import fortis.text { Text, Builder }

// One word and the number of times it occurred.
class Entry {
  export word: String
  export count: Int
}

// Counts words, remembering the order in which each was first seen.
class Tally {
  counts: Map[String, Int]
  order: List[String]

  export pure func empty() Tally => Tally(counts: Map.default(), order: List.default())

  export func add(editable self, word: StringView) {
    let seen = self.counts.get(word) ?? 0
    if seen == 0 {
      self.order.push(word as String)
    }
    self.counts.set(word as String, Int.saturatingAdd(seen, 1))
  }

  export pure func entries(self) List[Entry] {
    let mut out: List[Entry] = List.default()
    for word in self.order {
      let n = self.counts.get(word) ?? 0
      out.push(Entry(word: copy word, count: n))
    }
    return out
  }
}

// The entry with the highest count; the first one seen wins a tie.
pure func mostFrequent(entries: ListView[Entry]) Entry? {
  let mut best: Entry? = null
  let mut bestCount = 0
  for entry in entries {
    if entry.count > bestCount {
      best = Entry(word: copy entry.word, count: entry.count)
      bestCount = entry.count
    }
  }
  return best
}

func main(console: Console) {
  let mut tally = Tally.empty()
  for word in Text.words("the cat and the hat and the bat") {
    tally.add(word)
  }
  let entries = tally.entries()
  for entry in entries {
    console.print("${entry.word}: ${entry.count}")
  }

  let threshold = 2
  let repeated = entries.countWhere(fn[threshold](e: Entry) Bool => e.count >= threshold)
  console.print("${repeated} words occur at least ${threshold} times")

  let mut summary = Builder.create()
  {
    let mut describe = fn mut[editable summary](e: Entry) Void {
      summary.append("[${e.word} ${e.count}]")
    }
    for entry in entries {
      describe(entry)
    }
  }
  console.print(summary.finish())

  match mostFrequent(entries) {
    present(top) => console.print("most frequent: ${top.word}")
    null => console.print("no words")
  }
}
```

```output
the: 3
cat: 1
and: 2
hat: 1
bat: 1
2 words occur at least 2 times
[the 3][cat 1][and 2][hat 1][bat 1]
most frequent: the
```

### The tally's state is private

`Tally`'s two fields carry no `export`, so they are private to the module (LR 4.4). Inside this module the
generated constructor `Tally(counts: …, order: …)` is usable, because every field is visible here; another
module could not call it and would have to go through `Tally.empty()`, an associated function: a function in the
class with no `self` (LR 9.2). That is how a class keeps an invariant, here that `order` lists each key of
`counts` exactly once.

`Map.default()` and `List.default()` are empty collections; their element types come from the field types they
initialise (LR 6.10). Rather than iterating the map, the class keeps the order of first sight in a list of its
own, so the order is part of its state and is visible in its declaration.

### Changing the tally is visible where it is declared

`add` declares `editable self`: it needs an exclusive loan of the tally. The tally is a `let mut` binding, which
is what allows `tally.add(word)` to change it, so a reader of `main` learns from one declaration that `tally`
changes (LR 7.5, LR 9.3). `entries` takes plain `self`, a read loan, so `tally.entries()` cannot change anything.

`word` is a `StringView`, a borrowed piece of text. The map's keys are owned `String`s, so `add` makes each one
with `word as String`, which is where the allocation happens and is written down (LR 6.6, LR 6.13). The two
conversions each produce a fresh value, and a fresh value is passed to an `owned` parameter such as `push`'s
without any marker (LR 7.5).

A lookup needs no owned key: a `Map` with `String` keys takes its `get` key as a `StringView`, so
`self.counts.get(word)` looks the word up as it is, allocating nothing (LR 6.7). Older code writes
`get(String.from(word))` or `get(copy word)`; `fortis fmt` drops the copy. `get` returns an optional read loan of
the stored count, `null` when the key is absent, and `?? 0` supplies the fallback (LR 6.7, LR 6.8). The count is incremented with `Int.saturatingAdd`, one of the
named integer operations, because a plain `+` on an unbounded `Int` could overflow and the checker would refuse
it (LR 6.4).

### Building a list and returning it

`entries` is `pure`, although it pushes onto a list: the list is created in the same call, so writing to it
changes nothing anyone else can see (LR 11.1). It returns the list with `return out`, which moves the local to
the caller, since nothing can use it afterwards (LR 7.5). Each `Entry` gets its own copy of the word, written `copy word`:
duplicating a `String` allocates, so it is never implicit (LR 6.11, LR 7.5).

`mostFrequent` takes a `ListView[Entry]`, a borrowed view of a list, and `main` passes `entries`, the list
itself (LR 6.7). It returns `Entry?`: an entry, or `null` for an empty list (LR 6.8).

### Two closures, two kinds

`countWhere` takes a callback. `fn[threshold](e: Entry) Bool => …` is a `fn`, the kind that only reads; its
capture list names `threshold`, which, being an `Int`, may be listed bare and is copied in (LR 8.2, LR 8.3). The
lambda does nothing but compare, so its inferred summary is `pure` and it satisfies `countWhere`'s `pure fn`
parameter without saying so (LR 6.7, LR 11.3).

`describe` appends to `summary`, so it captures it with `editable summary` and is a `fn mut`; its binding is
`let mut`, which is what lets each call, `describe(entry)`, change it (LR 7.5, LR 8.2). While `describe` exists it holds an
exclusive loan of `summary`, so the closure lives in an inner block: at the block's closing brace the closure is
dropped, the loan ends, and `summary.finish()` may read the builder again (LR 7.3, LR 8.3). Without the block the
call to `finish` would be rejected as a use of a loaned owner.

### Matching on an optional

`match mostFrequent(entries)` covers both cases of the optional with the patterns `present(top)` and `null`
(LR 6.8, LR 9.8). An `if` would also work; the `match` makes both outcomes visible side by side.

## An inventory ledger, as a package

The second program is a package: a library module with the ledger and its tests, and an application module that
uses it. It shows nominal ids, declared errors, and the ways a failure can be handled: propagated, recovered
after a report, and left to end the program.


```yaml
# file: guide.ledger/project.yaml
schema: fortis.project/v1
name: guide.ledger
version: 1.0.0
entry: app
exports: [stock]
dependencies:
  fortis: "1.0"
```

```fortis
// accept
// file: guide.ledger/src/stock.ft

// A stock-keeping unit. A nominal type: no Int, and no other id, is ever a Sku by accident.
export type Sku = Int<1..999999>

export error UnknownSku {
  code: "ledger.unknown_sku"
  payload { sku: Sku }
  message: "no item has SKU ${sku}"
}

export error Insufficient {
  code: "ledger.insufficient"
  payload { sku: Sku, held: Int, wanted: Int }
  message: "SKU ${sku} holds ${held}, but ${wanted} were wanted"
}

export class Item {
  export sku: Sku
  export name: String
  export quantity: Int<0..10000>
}

export class Ledger {
  items: List[Item]

  export pure func empty() Ledger => Ledger(items: List.default())

  export func add(editable self, item: owned Item) {
    self.items.push(move item)
  }

  // Where the item with this SKU is held.
  pure try func indexOf(self, sku: Sku) Int {
    let mut i = 0
    for item in self.items {
      if item.sku == sku {
        return i
      }
      i = i + 1 or fail
    }
    fail UnknownSku(sku: sku)
  }

  export pure try func quantityOf(self, sku: Sku) Int<0..10000> {
    let at = self.indexOf(sku) or fail
    let item = self.items.at(at) or fail
    return item.quantity
  }

  // Takes `wanted` units and returns what is left.
  export try func take(editable self, sku: Sku, wanted: Int<0..10000>) Int<0..10000> {
    let at = self.indexOf(sku) or fail
    let item = self.items.at(at) or fail
    if wanted > item.quantity {
      fail Insufficient(sku: sku, held: item.quantity, wanted)
    }
    let left = (item.quantity - wanted) as Int<0..10000> or fail
    self.items.set(at, Item(sku, name: copy item.name, quantity: left)) or fail
    return left
  }
}

test "taking stock leaves the rest" {
  let mut ledger = Ledger.empty()
  ledger.add(Item(sku: Sku(7), name: "washer", quantity: 10))
  expect ledger.take(Sku(7), 4) == 6
  expect ledger.quantityOf(Sku(7)) == 6
}

test "taking too much fails and changes nothing" {
  let mut ledger = Ledger.empty()
  ledger.add(Item(sku: Sku(7), name: "washer", quantity: 3))
  let left = ledger.take(Sku(7), 4) catch e {
    expect e.code == "ledger.insufficient"
    recover 0
  }
  expect left == 0
  expect ledger.quantityOf(Sku(7)) == 3
}
```

```fortis
// accept (exit status 1)
// file: guide.ledger/src/app.ft
import fortis.host { Console }
import guide.ledger.stock { Ledger, Item, Sku }

try func main(console: Console) {
  let mut ledger = Ledger.empty()
  ledger.add(Item(sku: Sku(101), name: "bolt", quantity: 40))
  ledger.add(Item(sku: Sku(202), name: "nut", quantity: 5))

  let bolts = ledger.take(Sku(101), 15) or fail
  console.print("bolts left: ${bolts}")

  // An expected failure, handled where it happens: reported once, then replaced.
  let nuts = ledger.take(Sku(202), 9) catch e {
    console.print("could not take 9 nuts: ${e.code}")
    report e
    recover ledger.quantityOf(Sku(202)) or fail
  }
  console.print("nuts left: ${nuts}")

  // An unexpected failure propagates: main fails, and the runtime reports it once.
  let washers = ledger.take(Sku(303), 1) or fail
  console.print("never printed: ${washers}")
}
```

```output
bolts left: 25
could not take 9 nuts: ledger.insufficient
nuts left: 5
```

The program ends with exit status 1 on purpose: the last `take` names a SKU the ledger does not hold.

### The package

A package is a directory with `project.yaml` and `src/`. The manifest names the package, its version, its
dependencies (here only the standard package `fortis`), the modules other packages may import (`exports`) and,
for an application, its entry module (LR 4.1). Each file under `src/` is a module named after its path, so
`src/stock.ft` is the module `stock`, imported from the sibling module as `guide.ledger.stock` (LR 4.2, LR 4.3).
`fortis run guide.ledger` checks the package and runs `main` in the entry module; `fortis test guide.ledger` runs
its tests.

### Ids that cannot be confused

`type Sku = Int<1..999999>` declares a new nominal type over a refined `Int` (LR 6.9). A `Sku` is not an `Int`:
a quantity can never be passed where a SKU is expected, or the other way round. `Sku(101)` is a literal the
checker proves to be in range; a value from outside the program (a parsed line, a request) would go through
`raw as Sku or fail`, which checks an unproved value and fails when it is out of range (LR 6.3, LR 6.9, LR 6.13). Two `Sku`s compare with `==`
because a newtype over a trivial type is `Equatable` when its representation is (LR 6.9).

### Declared failures

`error UnknownSku { … }` declares a failure kind: a stable `code`, a typed `payload`, and a `message` template
that may name only payload fields (LR 10.3). `fail UnknownSku(sku: sku)` inside a `try` function ends it with that
failure (LR 10.1).

`indexOf` is `pure try`: it reads only the ledger and may fail. Its counter uses `i = i + 1 or fail`: the checker
cannot prove that `i + 1` fits, so under `or fail` the `+` is the checked addition, which fails rather than
wrapping (LR 6.4). `items.at(at)` is the checked way to read a list
element, an outcome that fails past the end (LR 6.7).

### The checker's view of a subtraction

`take` first refuses to take more than is held. After that test the checker knows that `wanted` lies in
`0..10000` and so does `item.quantity`, but it does not record that one is at most the other: its facts are
intervals of single places (LR 6.3). The difference could, as far as it knows, be negative, so the result is
validated into `Int<0..10000>`, which cannot fail here but says exactly what is being assumed (LR 6.3). The
chapter on [proving arithmetic and indexing](06-proofs.md) covers what the checker can prove on its own.

`items.set(…)` replaces an element and returns an `Outcome[Void]`, failing past the end, so it too ends in
`or fail` (LR 6.7). The replacement item copies the name with `copy item.name`: `item` is a read loan of the old
element, and the new one needs its own `String` (LR 7.5).

### Three ways to handle a failure

In `main`, every call to a `try` function produces an outcome that must be consumed exactly once (LR 10.1):

- `ledger.take(Sku(101), 15) or fail` propagates any failure out of `main` (LR 10.2). `take` changes the ledger,
  which `main` declares `let mut`.
- The second `take` is expected to fail. Its `catch e { … }` block runs only on failure; it prints the code,
  writes the failure to the diagnostic plane with `report e`, which consumes it, and ends with `recover …`, the
  value the expression has instead (LR 10.4, LR 10.6). Because the failure was reported before being replaced,
  `main` needs no `recovers` modifier (LR 10.4). Inside a `catch` block, `or fail` still means the enclosing
  function, so the fallback read can itself propagate (LR 10.4).
- The last `take` fails with `ledger.unknown_sku`, and `or fail` passes it out of `main`. When a `try main`
  fails, the runtime writes exactly one report of the failure to standard error and the process exits with
  status 1 (LR 4.6).

The tests in `stock.ft` are part of the module and are compiled only in test builds (LR 14). In a test body a
failing call fails the test, so `expect ledger.take(Sku(7), 4) == 6` needs no `or fail`, and expecting a failure
inside `catch` consumes it (LR 14).

## An expression evaluator

The third program evaluates arithmetic expressions held as a tree. It shows an enum with payloads, exhaustive
`match`, recursion, and arithmetic that fails instead of overflowing.

```fortis
// accept
import fortis.host { Console }

export error DivideByZero {
  code: "calc.divide_by_zero"
}

// An arithmetic expression: a tree whose nodes own their children.
enum Expr {
  Number(value: Int)
  Add(left: Expr, right: Expr)
  Multiply(left: Expr, right: Expr)
  Divide(left: Expr, right: Expr)
  Negate(operand: Expr)
}

// Evaluates an expression; overflow and division by zero are failures, never wrong answers.
pure try func eval(e: Expr) Int {
  return match e {
    Number(value: v) => v
    Add(left: l, right: r) => {
      let a = eval(l) or fail
      let b = eval(r) or fail
      give a + b or fail
    }
    Multiply(left: l, right: r) => {
      let a = eval(l) or fail
      let b = eval(r) or fail
      give a * b or fail
    }
    Divide(left: l, right: r) => {
      let a = eval(l) or fail
      let b = eval(r) or fail
      if b == 0 {
        fail DivideByZero()
      }
      give a / b or fail
    }
    Negate(operand: o) => {
      let a = eval(o) or fail
      give 0 - a or fail
    }
  }
}

// Renders an expression with full parentheses.
pure func show(e: Expr) String {
  return match e {
    Number(value: v) => "${v}"
    Add(left: l, right: r) => "(${show(l)} + ${show(r)})"
    Multiply(left: l, right: r) => "(${show(l)} * ${show(r)})"
    Divide(left: l, right: r) => "(${show(l)} / ${show(r)})"
    Negate(operand: o) => "-${show(o)}"
  }
}

pure func number(v: Int) Expr => Number(value: v)

func main(console: Console) {
  let examples = List.of(
    Expr.Add(left: number(2), right: Multiply(left: number(3), right: number(4))),
    Negate(operand: Divide(left: number(10), right: number(3))),
    Divide(left: number(1), right: Add(left: number(2), right: Negate(operand: number(2)))),
    Multiply(left: number(4611686018427387904), right: number(2)),
  )
  for e in examples {
    match eval(e) {
      success(v) => console.print("${show(e)} = ${v}")
      failure(f) => {
        console.print("${show(e)} fails: ${f.code}")
        report f
      }
    }
  }
}
```

```output
(2 + (3 * 4)) = 14
-(10 / 3) = -3
(1 / (2 + -2)) fails: calc.divide_by_zero
(4611686018427387904 * 2) fails: numeric.overflow
```

### An enum is a closed set of shapes

`Expr` has five variants, each with named payload fields (LR 9.7). Inside a `match` over an `Expr` the variant
names need no `Expr.` prefix (LR 9.7). The enum is closed: a `match` must cover every variant, and if a sixth were
added, every `match` that does not handle it would be reported, each missing variant by name (LR 9.8). Neither
`eval` nor `show` has a catch-all arm, and none is allowed as the last arm of a match over a closed enum: that is
what makes adding a variant safe (LR 9.8).

`match e` takes a read loan of the expression, and the payload bindings `l`, `r` and `o` are read loans of the
children (LR 9.8). Nothing is copied while walking the tree.

### Arms that compute

A single-expression arm is just `Pattern => value`. An arm that needs statements is a block and ends with
`give value` (LR 9.8, LR 5.4). Inside those blocks `or fail` and `fail` leave `eval` itself, not the arm: they
always mean the enclosing function (LR 5.4).

Each operation is checked: `a + b or fail`, `a * b or fail`, `a / b or fail` and `0 - a or fail` cannot be
proved safe on any `Int`, so under `or fail` each is the checked operation, failing with `numeric.overflow` or
`numeric.divide_by_zero` (LR 6.4). The evaluator also checks for a
zero divisor itself, so that a division by zero reports the calculator's own code. Integer division truncates
toward zero, so `10 / 3` is `3` (LR 6.4).

### Matching on an outcome

`main` handles each result with `match` and the outcome patterns `success(v)` and `failure(f)` (LR 9.8, LR 10.2).
The `failure` arm owns the failure and must consume it, here with `report f` (LR 10.2). Because every failure is
reported rather than silently replaced, `main` needs no `recovers` (LR 10.4, LR 11.2).

## Concurrent work with a scope

The last program runs work concurrently on the core executor. Each batch of readings is summed by its own task;
the tasks belong to a `scope`, which joins them before it ends.

> Preview: async functions, `scope` and `join` (LR 12.3, 12.5) arrive in v0.3.

```fortis
// accept
import fortis.host { Console }

export error Empty {
  code: "stats.empty"
}

// The sum of a batch of readings; overflow is a failure.
async try func total(readings: owned List[Int]) Int {
  if readings.isEmpty() {
    fail Empty()
  }
  let mut sum = 0
  for r in readings {
    sum = sum + r or fail
  }
  return sum
}

// The largest reading of a batch, or null for an empty one.
async func peak(readings: owned List[Int]) Int? {
  let mut best: Int? = null
  for r in readings {
    let current = best ?? r
    if r >= current {
      best = r
    }
  }
  return best
}

// Sums both batches concurrently in one scope; the scope joins all its tasks before it ends.
async try func sumAll(console: Console, a: owned List[Int], b: owned List[Int]) Int {
  scope tasks {
    let first = tasks.launch(total(move a))
    let second = tasks.launch(total(move b))
    let x = await first or fail
    let y = await second or fail
    console.print("batch totals ${x} and ${y}")
    return x + y or fail
  }
}

async try func main(console: Console) {
  defer {
    console.print("finished")
  }
  let all = await sumAll(console, List.of(3, 5, 8), List.of(13, 21)) or fail
  console.print("grand total ${all}")

  let peaks = await join(peak(List.of(4, 9, 2)), peak(List.default()))
  console.print("peaks ${peaks.0 ?? -1} and ${peaks.1 ?? -1}")

  let empty = await sumAll(console, List.of(1), List.default()) catch f {
    console.print("could not sum: ${f.code}")
    report f
    recover 0
  }
  console.print("after the failure: ${empty}")
}
```

```output
batch totals 16 and 34
grand total 50
peaks 9 and -1
could not sum: stats.empty
after the failure: 0
finished
```

### Calling an async function builds a future

`total` and `peak` are `async`. Calling one does not run it: `total(move a)` evaluates its arguments and returns
a `Future`, and the body starts when the future is awaited or launched (LR 12.3). A future is linear: it must be
awaited, launched, or discarded, and discarding is allowed only for a future never started whose environment
holds no linear value and no exclusive loan (LR 12.3). The batches are passed as `owned`
lists, written `move a` at the call, so each task owns its data (LR 7.6).

### The scope owns its tasks

`scope tasks { … }` is a block that owns tasks. `tasks.launch(future)` starts a task and returns a `Task`, which
must be awaited inside the scope (LR 12.5). `await first or fail` waits for the first task and propagates its
failure. If that happens, the scope cancels `second` and waits for it before the block ends, so a scope never
leaves a task running behind it (LR 12.5). That is what the third call to `sumAll` exercises: the second batch is
empty, its task fails with `stats.empty`, the failure leaves `sumAll`, and `main` reports it and recovers.

`console` is a plain `Console` parameter, a read loan of the capability. Cooperative `launch` may use loans of
anything declared before the scope, because the scope joins its tasks before those bindings are dropped
(LR 12.5).

### Two futures without a scope

`join(a, b)` consumes two futures, runs both inside the current task, and returns both results as a tuple once
both have finished (LR 12.5). Here the results are optional `Int`s, read as `peaks.0` and `peaks.1` (LR 6.9).

### Deterministic output

`async try func main` runs on the core cooperative executor as the root task (LR 4.6). Every cooperative task of
a scope tree runs on one thread (LR 12.5), and in this program only `sumAll` and `main` print, after their awaits,
so the output does not depend on how the tasks interleave. Nothing about the timing of interleavings is promised
to be the same on every backend (LR 12.7); a program whose output must be stable keeps its printing out of
concurrently running tasks, as this one does.

### Cleanup at every exit

`defer { … }` registers a block that runs when the enclosing block exits by any route: falling off the end, a
`return`, or a failure leaving through `or fail` (LR 12.1). Here it prints `finished` last. A deferred block may
not itself `return`, `fail` or `await`; it can only clean up (LR 12.1).
