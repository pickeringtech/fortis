# Outcomes and failures

<!-- covers: LR 10 -->
*Human mode. Agent mode: [Outcomes and failures, by rule](../agent/10-outcomes.md). Source of truth: LR 10.*

Fortis has no exceptions. A function that can fail says so with `try`, and its caller receives an `Outcome`: a
value that holds either the success value or a `Failure`. An outcome is **linear**: it must be dealt with exactly
once, and the checker rejects a program that drops one (LR 1, LR 10.1). This chapter shows the ways to deal with
an outcome, how failures are declared, matched and reported, and the few conditions, called defects, that are
not failures at all.

An absent value (`T?`), a `Bool` and an outcome are three different things with three different operators
(`??`, `||`, `or`), and none converts into another (LR 1, LR 6.8).

## Fallible functions

`try func f(…) T` returns `Outcome[T]`. Inside it, `return v` succeeds with `v` and `fail f` fails with the
failure `f` (LR 10.1). A call to it needs no prefix, but its result is an `Outcome[T]`, not a `T`: the success
value cannot be reached without consuming the outcome (LR 10.1).

```fortis
// accept
import fortis.host { Console }

export error NotEven {
  code: "demo.not_even"
  payload { value: Int }
  message: "${value} is odd"
}

pure try func half(n: Int) Int {
  if n % 2 != 0 {
    fail NotEven(value: n)
  }
  return n / 2
}

try func main(console: Console) {
  let h = half(10) or fail
  console.print("half of 10 is ${h}")
}
```

```output
half of 10 is 5
```

Using the outcome as if it were the value is a type error (here `E-TYP-040`, LR 10.1), and leaving it unconsumed
is `E-OBL-001` (LR 10.1):

```fortis
// reject: E-OBL-001, E-TYP-040
export error NotEven {
  code: "demo.not_even"
}

pure try func half(n: Int) Int {
  if n % 2 != 0 {
    fail NotEven()
  }
  return n / 2
}

pure func double(n: Int) Int => Int.saturatingMul(n, 2)

func main() {
  half(10)                            // reject: E-OBL-001
  let d = double(half(10))            // reject: E-TYP-040
}
```

## Consuming an outcome

These are all the ways to consume an outcome (LR 10.2):

| Operation | What happens to the failure | Where |
|---|---|---|
| `e or fail` | passed on to the caller | in a `try` function |
| `e or recover v` | replaced by `v`, which is evaluated only on failure | anywhere; adds `recovers` |
| `e catch name { … }` | handled in a block that ends in `recover`, `return` or `fail` | anywhere |
| `match e { success(x) => …, failure(f) => … }` | handled by the `failure` arm, which consumes `f` | anywhere |
| `report f` | written to the diagnostic plane | where `io` is allowed |
| `move` to an `owned Outcome[T]` or `owned Failure` parameter, or into a linear container or field | transferred | anywhere |
| `fail f` | passed on | in a `try` function |

There is no bare `or value`: every discard of a failure is spelled `recover`, so a reader can find them all. There
is no `?`, no `try` at the call site and no `unwrap` (LR 10.2).

An `Outcome[T]` has one method, `map[U](owned self, f: pure fn(T) U) Outcome[U] where U: linear`, which consumes it: a success
becomes the success of `f` applied to its value (read loan) and a failure passes through unchanged with `f` not
called (LR 10.2, X-479). The result is an outcome like any other and is handled on the line, and the receiver is
a `try` call's result or a place written `move result.map(f)`, since the method takes `owned self`. It is the
only helper: `or fail`, `or recover`, `catch` and `match` handle a failure and `annotate` adds context (LR 10.6),
and a `map` over the failure, an `andThen` or an `unwrapOr` would each be a second spelling of one of them.

```fortis
// accept
import fortis.host { Console }

export error NotEven {
  code: "demo.not_even"
}

pure try func half(n: Int) Int {
  if n % 2 != 0 {
    fail NotEven()
  }
  return n / 2
}

recovers func main(console: Console) {
  let label = half(8).map(fn(n) => "half is ${n}") or recover "none"
  console.print(label)
  let missing = half(7).map(fn(n) => "half is ${n}") or recover "none"
  console.print(missing)
}
```

```output
half is 4
none
```

Because `or` binds loosest, the idiom is one decision per line: `let v = f() or fail`, then use `v` (LR 1.1).

```fortis
// accept
import fortis.host { Console }

export error NotEven {
  code: "demo.not_even"
  payload { value: Int }
}

pure try func half(n: Int) Int {
  if n % 2 != 0 {
    fail NotEven(value: n)
  }
  return n / 2
}

pure recovers func halfOrZero(n: Int) Int => half(n) or recover 0

recovers func describe(n: Int) String {
  return match half(n) {
    success(v) => "half of ${n} is ${v}"
    failure(f) => {
      report f
      give "${n} has no half"
    }
  }
}

recovers func main(console: Console) {
  console.print("${halfOrZero(7)} ${halfOrZero(8)}")
  console.print(describe(6))
  console.print(describe(3))
}
```

```output
0 4
half of 6 is 3
3 has no half
```

`describe(3)` reports its failure, so a failure record also goes to standard error; the output above is standard
output only.

### `recovers`: discarding a failure is visible

A failure discharged without being reported, by `or recover`, a `catch` ending in `recover`, or a `match` arm that
drops it, adds the `recovers` effect (LR 10.2, LR 10.4). A function with that effect must say `recovers` in its
head, and because a call imports its callee's effects, so must the functions that call it; a signature therefore
shows who swallowed a failure (LR 8.1, LR 11.2, LR 11.3). Leaving the modifier off is `E-EFF-001` (LR 11.1):

```fortis
// reject: E-EFF-001
export error NotEven {
  code: "demo.not_even"
}

pure try func half(n: Int) Int {
  if n % 2 != 0 {
    fail NotEven()
  }
  return n / 2
}

pure func halfOrZero(n: Int) Int => half(n) or recover 0     // reject: E-EFF-001

func main() {
}
```

## Declaring failures

`Failure` is the one failure type. It carries a stable `code`, a typed payload, an origin (module, function and
line) and an optional chain of causes; it is linear and cannot be copied (LR 10.3). Each kind of failure is
declared with `error` (LR 10.3):

```fortis
// illustrative
export error Name {
  code: "package.area.kind"
  payload { field: Type, … }
  message: "template naming ${field}"
}
```

The declaration gives a constructor, `Name(field: …)`, that produces a `Failure`, a pattern `is Name(…)` for
matching, and a display template (LR 10.3). The `message` is a template, not an interpolated string: its `${…}`
may name only payload fields, and it is rendered when the failure is displayed (LR 10.3). Codes are unique within a
package and stable across versions, and payload fields are trivial values or length-bounded strings, so a failure
is bounded and never carries a secret (LR 10.3). A failure raised with `fail` in a `pure` function reads no clock
and allocates no identifier, so it is as deterministic as the function (LR 10.3).

What a program can read from a failure is its `code` (`e.code`, a `StringView`) and, through an `is Name(field: x)`
pattern, its payload (LR 10.3, LR 10.5). The message template is for display: LR 10.3 says it is rendered when
the failure is displayed, but `Failure` does not implement `Displayable` in Language 1.0's library, so it cannot
be interpolated, and the records the toolchain writes carry the code and payload without rendering the template
yet (X-154, D-57). Build a message from the payload when a program needs one. One more difference from the LR
today: the checkers accept `fail Name(…)`, but refuse a constructor call where a `Failure` value is expected
(`Outcome.failure(Name(…))`, a binding annotated `Failure`), which LR 10.3 allows (D-52).

A test or an adapter that needs to build an outcome by hand uses `Outcome.success(v)` and `Outcome.failure(move f)`
(LR 10.1). Until D-52 is fixed, obtain the `Failure` from a failing call, as this example does:

```fortis
// accept
import fortis.host { Console }

export error Missing {
  code: "store.missing"
  payload { key: Int }
}

pure try func requireKey(key: Int) Int {
  if key > 3 {
    fail Missing(key: key)
  }
  return key
}

pure func fake(key: Int) Outcome[Int] {
  return match requireKey(key) {
    success(value) => Outcome.success(value)
    failure(f) => Outcome.failure(move f)
  }
}

recovers func main(console: Console) {
  let a = fake(2) or recover -1
  let b = fake(9) or recover -1
  console.print("${a} ${b}")
}
```

```output
2 -1
```

## `catch` and `recover`

`e catch name { … }` names the failure and runs the block only when `e` failed (LR 10.4). Inside the block,
`return` and `fail` leave the enclosing function, and `recover v` ends the block with `v` as the value of the whole
expression. A `catch` covers only its left operand (LR 10.4). When the failing call returns `Void`, the replacement
is `Void`'s one value, written `recover Void` (LR 6.2, LR 10.4).

```fortis
// accept
import fortis.host { Console }

export error NotANumber {
  code: "parse.not_a_number"
  payload { position: Int }
}

pure try func parseDigits(text: StringView) Int {
  let n = text.scalarCount()
  let mut value = 0
  let mut i = 0
  while i < n {
    let s = text.scalarAt(i) or fail
    let d = Scalar.digitValue(s)
    if d == null {
      fail NotANumber(position: i)
    }
    value = value * 10 or fail
    value = value + d or fail
    i = Int.add(i, 1) or fail
  }
  return value
}

func parseOrZero(text: StringView) Int {
  let v = parseDigits(text) catch e {
    report e
    recover 0
  }
  return v
}

func main(console: Console) {
  console.print("${parseOrZero("2026")} ${parseOrZero("20x6")}")
}
```

```output
2026 0
```

If the block consumes the failure before `recover`, as `report e` does here, the `recover` discards nothing and
adds no `recovers` effect; if it does not, `recover` consumes the failure and the function gets `recovers`
(LR 10.4). Every path through the block must end in `recover`, `return`, `fail` or a call that never returns
(`E-OBL-002`, LR 10.4):

```fortis
// reject: E-OBL-002, E-OBL-001
export error NotEven {
  code: "demo.not_even"
}

pure try func half(n: Int) Int {
  if n % 2 != 0 {
    fail NotEven()
  }
  return n / 2
}

recovers func careful(n: Int) Int {
  let v = half(n) catch e {          // reject: E-OBL-002, E-OBL-001
    if n > 100 {
      recover 0
    }
  }
  return v
}

func main() {
}
```

The path that falls off the end of the block also leaves `e` unconsumed, which is `E-OBL-001` on the same line
(LR 10.1).

## Matching on failures

`match move f` over a `Failure` uses `is ErrorName(…)` patterns, and because `Failure` is an open type it ends with
`otherwise(name)`, which receives any other failure and must consume it (LR 10.5). The library's own failures
(`numeric.overflow`, `collection.index`, `text.index`, …) have no declared error a program can name in an `is`
pattern yet (D-58), so test their code instead: `if e.code == "numeric.overflow" { … }` inside a `catch e`
block, or after `match f` (a read loan) and before the failure is consumed (LR 10.3, LR 10.5). Payload fields are bound by copy.
An `is` arm without `as` discharges the failure when it matches, which adds `recovers`; an arm that binds the whole
failure with `as name` must consume `name` (LR 10.5).

```fortis
// accept
import fortis.host { Console }

export error NotANumber {
  code: "parse.not_a_number"
  payload { position: Int }
}

export error Empty {
  code: "parse.empty"
}

export error TooLong {
  code: "parse.too_long"
  payload { length: Int }
}

pure try func parseDigits(text: StringView) Int {
  let n = text.scalarCount()
  if n == 0 {
    fail Empty()
  }
  if n > 9 {
    fail TooLong(length: n)
  }
  let mut value = 0
  let mut i = 0
  while i < n {
    let s = text.scalarAt(i) or fail
    let d = Scalar.digitValue(s)
    if d == null {
      fail NotANumber(position: i)
    }
    value = value * 10 or fail
    value = value + d or fail
    i = Int.add(i, 1) or fail
  }
  return value
}

recovers func explain(text: StringView) String {
  return match parseDigits(text) {
    success(v) => "value ${v}"
    failure(f) => match move f {
      is NotANumber(position: p) => "not a digit at ${p}"
      is Empty => "empty input"
      is TooLong as long => {
        report long
        give "too long"
      }
      otherwise(other) => {
        report other
        give "unexpected"
      }
    }
  }
}

recovers func main(console: Console) {
  console.print(explain("42"))
  console.print(explain("4x2"))
  console.print(explain(""))
  console.print(explain("12345678901"))
}
```

```output
value 42
not a digit at 1
empty input
too long
```

A `match f` without `move` is for inspection only: it copies payloads, no arm may consume `f`, and `f` must still
be consumed afterwards (LR 10.5).

## Reports, annotation and aggregation

`report f` is a statement that consumes a failure and writes it to the current task's failure reporter; with
`report f as receipt` it also binds a `Report`, a small receipt that cannot be turned back into a `Failure`, for use
in a public response (LR 10.6). Once reported, the failure is gone, so it cannot also be propagated (LR 10.6), and a
second use is a use after move (`E-OWN-020`, LR 7.4):

```fortis
// reject: E-OWN-020
export error NotEven {
  code: "demo.not_even"
}

pure try func half(n: Int) Int {
  if n % 2 != 0 {
    fail NotEven()
  }
  return n / 2
}

try func strict(n: Int) Int {
  let v = half(n) catch e {
    report e
    fail e                           // reject: E-OWN-020
  }
  return v
}

func main() {
}
```

`report` has the `io` effect, but it is the one `io` operation that needs no capability: it writes only to the
diagnostic plane, which cannot feed anything back into the program (LR 10.6, LR 11.7). A `pure` function therefore
cannot report; it propagates or recovers (LR 10.6).

Two library functions help with context and with many failures (LR 10.6):

- `annotate(move f, note)` returns a failure with the same code and payload, the original as its cause and the
  note added. It is the only way to add context, and it keeps exactly one obligation alive.
- `Failures.aggregate(move list)` consumes a `List[Failure]` and gives one failure whose causes are the inputs: the
  rule for parallel work and retries.

```fortis
// accept
import fortis.host { Console }

export error Missing {
  code: "store.missing"
  payload { key: Int }
}

pure try func lookup(key: Int) Int {
  if key > 3 {
    fail Missing(key: key)
  }
  return key * 10 or fail
}

pure try func lookupBoth(a: Int, b: Int) Int {
  let x = lookup(a) catch e {
    fail annotate(move e, "first key")
  }
  let y = lookup(b) or fail
  return x + y or fail
}

func failures(keys: List[Int]) Failure {
  let mut errors: List[Failure] = List.default()
  for k in keys {
    match lookup(k) {
      success(v) => {
      }
      failure(f) => {
        errors.push(move f)
      }
    }
  }
  return Failures.aggregate(move errors)
}

func main(console: Console) {
  let total = lookupBoth(9, 1) catch e {
    console.print("failed with ${e.code}")
    report e
    recover 0
  }
  console.print("total ${total}")
  let all = failures(List.of(1, 7, 9))
  report all
  console.print("reported")
}
```

```output
failed with store.missing
total 0
reported
```

The annotated failure still has the code `store.missing`; its record lists the note. Metrics, span status and
debug logging take a read loan of a failure and consume nothing (LR 10.6).

### A failing `main`

When a `try main` fails, the runtime prints exactly one report of the failure to standard error and the process
exits with status 1 (LR 4.6). There is no `or die`: propagating to `main` is how a program stops on a failure.

```fortis
// accept (exit status 1)
import fortis.host { Console }

export error Missing {
  code: "store.missing"
  payload { key: Int }
}

pure try func lookup(key: Int) Int {
  if key > 3 {
    fail Missing(key: key)
  }
  return key
}

try func main(console: Console) {
  let a = lookup(2) or fail
  console.print("found ${a}")
  let b = lookup(8) or fail
  console.print("found ${b}")
}
```

```output
found 2
```

## Failures as data: one report each

A parser or a validator often has to try every input, keep going past the bad ones, and report each problem once.
Recovering each failure would add `recovers` to every caller up to `main` (LR 11.3). The alternative keeps every
failure: a `pure` function collects them, annotated with where they came from, into a `List[Failure]` it is lent
`editable`, and the caller reports each one (LR 10.6, LR 10.7):

```fortis
// accept
import fortis.host { Console }

export error NotADigit {
  code: "parse.not_a_digit"
  payload { position: Int }
}

pure try func digit(text: StringView) Int<0..9> {
  let s = text.scalarAt(0) or fail
  let d = Scalar.digitValue(s)
  if d == null {
    fail NotADigit(position: 0)
  }
  return d
}

// Every input is tried; a failure is kept, annotated with where it came from, for one report each.
pure func digits(inputs: List[String], sink: editable List[Failure]) List[Int] {
  let mut found: List[Int] = List.default()
  for text in inputs {
    match digit(text) {
      success(d) => {
        found.push(d)
      }
      failure(f) => {
        sink.push(annotate(move f, "input ${text}"))
      }
    }
  }
  return found
}

func main(console: Console) {
  let mut problems: List[Failure] = List.default()
  let inputs: List[String] = List.of("4", "x", "7", "")
  let found = digits(inputs, editable problems)
  console.print("${found.count()} digits, ${problems.count()} problems")
  move problems.consumeEach(fn mut(f: owned Failure) Void {
    report f
  })
}
```

```output
2 digits, 2 problems
```

`digits` stays `pure`: pushing onto a list it was lent is a write through its own `editable` parameter, which is
its footprint, not `mutate` (LR 11.3). No failure is discarded, so nothing is `recovers`; `annotate` keeps each
failure's code and payload and adds the note (LR 10.6). The two reports go to standard error, one record each, with
the code (`parse.not_a_digit`, and `text.index` for the empty input) and the note (LR 10.6).

## Linear containers

A collection of outcomes, such as `List[Outcome[T]]`, is linear too: every element must be consumed (LR 10.7).
The library says in each signature what happens to linear elements: `push(move item)` transfers one in, `pop()`
transfers one out as a `T?` that must itself be consumed, and `consumeEach(owned self, fn mut(owned T) Void)`
consumes the collection, calling the callback once per element, in order, with no early exit (LR 10.7).
`Outcome.collect(move list)` turns a list of outcomes into an outcome of a list, consuming every input (LR 10.7).

```fortis
// accept
import fortis.host { Console }

export error Missing {
  code: "store.missing"
  payload { key: Int }
}

pure try func lookup(key: Int) Int {
  if key > 3 {
    fail Missing(key: key)
  }
  return key
}

try func main(console: Console) {
  let mut results: List[Outcome[Int]] = List.default()
  results.push(lookup(1))
  results.push(lookup(8))
  results.push(lookup(3))
  let mut good = 0
  let mut bad = 0
  move results.consumeEach(fn mut[editable good, editable bad](r: owned Outcome[Int]) Void {
    match move r {
      success(v) => {
        good = Int.saturatingAdd(good, 1)
      }
      failure(f) => {
        report f
        bad = Int.saturatingAdd(bad, 1)
      }
    }
  })
  console.print("${good} good, ${bad} bad")
  let all = Outcome.collect(List.of(lookup(1), lookup(2), lookup(3))) or fail
  console.print("collected ${all.count()}")
}
```

```output
2 good, 1 bad
collected 3
```

`List.clear()` empties a list, and is rejected for a linear element type, as is dropping the collection with
one held: both would drop the elements (LR 10.7, X-522), while `count()` and `isEmpty()` only borrow it
(LR 10.7).

```fortis
// reject: E-OBL-001
func main() {
  let mut xs: List[Outcome[Int]] = List.default()   // reject: E-OBL-001 — the list drops with its elements held
  xs.push(Outcome.success(1))
  xs.clear()                                        // reject: E-OBL-001 — clear would drop the linear elements
}
```

A generic function may hold values of a type parameter in linear positions only if it declares
`T: linear`, and must then consume them (LR 10.7, LR 6.12). A user generic class or enum follows the same
declaration rule for a linear type argument: without `where T: linear` its instantiation is `E-OBL-003`, and
with that bound its owned contents must be consumed, and dropping the linear instance is `E-OBL-001`. Inference
cannot bypass either rule (LR 10.7, X-611).

A consuming loop (`for item in move xs`) transfers an obligation to its item on each iteration, including
for a bounded `T: linear` (LR 5.3, LR 10.7). Consume it before the body's end or `continue`; `_` cannot discard
it (`E-OBL-001`). The hidden cursor owns the unvisited remainder, which v0.1 has no operation to extract.
Consequently `break`, `return`, or `give`/`recover` leaving the loop are `E-OBL-001` even after consuming the
current item. `fail`, `or fail`, `await` and `checkpoint` are `E-OBL-005` while the remainder is live; the
hidden cursor has no `Abandonable` exemption. An ordinary `continue` keeps the remainder, but a labelled exit
crossing an inner consuming loop must also respect that loop's obligations.

These rules cover standard collections, collection newtypes and user `Consumable` implementations. Moving a
`ListView` does not make it consuming: the view only lends elements. A user consuming cursor that is itself
linear is `E-OBL-003`, because returning `null` from `next()` never consumes the cursor; transfer a bounded
class's contents to a standard collection first (LR 5.3, LR 10.7, X-622).

Reading methods such as `each`, `map`, `tryMap`, `fold`, `filter` and searches borrow the list and its items.
Their callbacks leave the items' obligations in the owner. A `move` marker does not change a reading
signature; bind a fresh linear list before borrowing it, then consume it afterwards. Borrowing such a
temporary and dropping its owner after the statement is `E-OBL-001` (LR 10.7).

`filter` needs `Copyable` elements. `fold` and `tryFold` need a non-linear accumulator; `tryMap` needs a
non-linear result because a later failure drops earlier results; `Outcome.map` and `Outcome.collect` reject
a linear input payload `T` (`E-OBL-003`). `Outcome.map` permits a linear result `U`, whose returned outcome
must still be consumed. A non-failing `List.map` can produce linear results, all owned by its returned
list. Use a consuming loop when each linear payload must be handled (LR 10.7, X-621). `consumeEach` also
transfers an obligation to its owned callback parameter. Returning from that callback stays within the
traversal, but still requires consuming the parameter; its repeatable, synchronous, non-failing function
type cannot propagate a failure or suspend.

## Defects

A **defect** is a condition that cannot be represented as an outcome, because the program's assumptions are
already broken. The list is closed (LR 10.8):

| Defect | Cause | Native code | Development VM |
|---|---|---|---|
| contract violation in trusted code | a `trusted` adapter breaks its declared contract | the process aborts | the task ends with a `TaskDefect` |
| stack exhaustion | unbounded recursion | abort | task terminated |
| out of memory | the allocator fails | abort | task terminated |
| runtime invariant | a bug in the VM or the runtime | abort | task terminated |

Ordinary Fortis code cannot cause a defect: there is no `unwrap`, no unchecked index and no assertion that aborts
(LR 10.8). Expected conditions, such as a bound, a parse error, I/O or arithmetic overflow, are always outcomes. A
defect is never a `Failure`, is never caught by `catch`, and produces no value; only the supervisor of a task that
ended with one sees it, as `TaskCompletion.Defect` (LR 10.8, LR 12.6).

## Consuming every loop item

Leaving a consuming loop would drop its unvisited linear elements (LR 5.3, LR 10.7):

```fortis
// reject: E-OBL-001
recovers func stop(items: owned List[Outcome[Int]]) {
  for item in move items {
    discard item or recover 0
    break                                 // reject: E-OBL-001
  }
}
```

Finish the traversal, consuming every item. `continue` is allowed once that iteration's item is consumed:

```fortis
// accept
import fortis.host { Console }

recovers func main(console: Console) {
  let items = List[Outcome[Int]].of(Outcome.success(1), Outcome.success(2))
  for item in move items {
    let n = item or recover 0
    if n == 1 {
      continue
    }
    console.print("${n}")
  }
}
```

```output
2
```

Propagating a failure also abandons the remainder, so handle each failure inside the loop instead
(LR 5.3, LR 10.7):

```fortis
// reject: E-OBL-005
try func propagate(items: owned List[Outcome[Int]]) {
  for item in move items {
    discard item or fail                   // reject: E-OBL-005
  }
}
```

```fortis
// accept
func drain(items: owned List[Outcome[Int]]) {
  for item in move items {
    match move item {
      success(n) => {
        discard n
      }
      failure(f) => {
        report f
      }
    }
  }
}

test "drain every outcome" {
  drain(List[Outcome[Int]].of(Outcome.success(1), Outcome.success(2)))
}
```
