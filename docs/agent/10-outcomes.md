# Outcomes and failures (LR 10)

<!-- covers: LR 10 -->
*Agent mode. Human mode: [Outcomes and failures](../guide/10-outcomes.md). Source of truth: LR 10.*

> Preview: linear containers (LR 10.7) arrive in v0.1. The rest of this chapter (LR 10) is in v0.0.1.

## Rules

| Rule | Statement | LR |
|---|---|---|
| LANG-10-01 | An `Outcome[T]` stays wrapped until consumed by an operation of LR 10.2; its success value is never reachable otherwise. | LR 10.1 |
| LANG-10-02 | Linear obligations survive storage in collections, fields and closures; dropping one is rejected. | LR 10.7 |
| LANG-10-03 | A reported failure is unreachable afterwards; `annotate` keeps exactly one obligation. | LR 10.6 |
| LANG-10-04 | `noalloc try` functions use the bounded failure representation on success and failure paths. | LR 10.3 |
| LANG-10-05 | `recover` without a prior `report` adds the `recovers` effect. | LR 10.4 |
| LANG-10-06 | Defects follow the closed taxonomy and are never catchable. | LR 10.8 |

## Facts

- `try func f(…) T` returns `Outcome[T]`; `return v` = success, `fail f` = failure; the call needs no prefix
  (LR 10.1). Patterns `success(x)`, `failure(f)`; by hand: `Outcome.success(v)`, `Outcome.failure(move f)` for a failure binding `f` (LR 10.1).
- Consuming operations, the complete list (LR 10.2):

  | Operation | Effect | Where |
  |---|---|---|
  | `e or fail` | propagate | `try` functions |
  | `e or recover v` | discharge, `v` lazy; adds `recovers` | anywhere |
  | `e catch n { … }` | block ends in `recover v`, `return`, `fail` or a `Never` call | anywhere |
  | `match e { success(x) => …, failure(f) => … }` | the `failure` arm consumes `f` | anywhere |
  | `report f` / `report f as r` | consume; `r: Report` receipt | `io` allowed (not `pure`) |
  | move to `owned Outcome[T]` / `owned Failure`, store in linear container or field | transfer | anywhere |
  | `fail f` (`f: Failure`) | propagate | `try` functions |

- No bare `or value`, no `?`, no call-site `try`, no `unwrap` (LR 10.2). `or` binds loosest: write
  `let v = f() or fail` on its own line (LR 1.1).
- Integer arithmetic the checker cannot prove safe, as the left operand of an outcome operator, is a checked
  region handled like a call: `total = total + r or fail`, `value * 10 + d or recover 0` (LR 6.4).
- In `test` and `property` bodies `or fail` is implied after a fallible call nothing else consumes:
  `expect half(4) == 2` (LR 14).
- `Failure`: stable `code` (`StringView from static`), typed payload, origin, optional cause chain; linear, not
  copyable; created in a `pure` function it reads no clock and allocates no identifier; bounded payload (64 bytes
  inline) satisfies `noalloc` (LR 10.3).
- `error Name { … }` with the lines `code: "…"`, optionally `payload { … }` and `message: "…${field}…"`, one per
  line: constructor `Name(…)` producing a `Failure`,
  pattern `is Name(…)`, display template over payload fields only (not rendered in records yet, and `Failure` is
  not `Displayable`: read `e.code` and payloads, X-154, D-57; constructor calls where a `Failure` value is expected
  are refused today, D-52); library failures have no `is`-nameable kind: test `e.code` (D-58); codes unique per package and stable; payload
  fields trivial or length-bounded `String`/`Bytes` (LR 10.3).
- `catch`: `return`/`fail` leave the function; `recover v` is the expression's value; `give` not allowed; covers
  only its left operand; `recover` after the failure was consumed (`report`, `fail`, `move`, `match`) adds
  nothing, otherwise it consumes and adds `recovers` (LR 10.4).
- Failure match: `match move f { is A(x: v) => …, is B as b => { …consume b… }, otherwise(o) => { …consume o… } }`;
  payloads bound by copy; `is` without `as` discharges (adds `recovers`); `otherwise(name)` is required (open type);
  `match f` without `move` inspects only and `f` must still be consumed (LR 10.5).
- `report` is a statement; with the `log` statements, the `io` needing no capability (diagnostic plane only); a
  `pure` function cannot report (LR 10.6, LR 11.6). A failure is recorded with `report e` alone, never logged as
  well; `log.warn(…)` is for what is not a failure, such as a fallback taken (LR 13).
  `annotate(f: owned Failure, note: StringView) Failure`; `Failures.aggregate(list: owned List[Failure]) Failure`
  (LR 10.6).
- A failing `try main`: one report on standard error, exit status 1 (LR 4.6).
- Linear containers: `push(move x)` in, `pop()` out as a linear `T?`, `consumeEach(owned self, fn mut(owned T) Void)`
  (in order, no early exit), `clear()` and dropping rejected, `count()`/`isEmpty()` borrow;
  `Outcome.collect(move list) Outcome[List[T]]`; a generic function holding `T` linearly declares `T: linear`
  (LR 10.7).
- Defects (contract violation in trusted code, stack exhaustion, out of memory, runtime invariant): native aborts,
  the VM terminates the task; never a `Failure`, never caught, no completion value; observed only as
  `TaskCompletion.Defect` by a supervisor (LR 10.8).

## Diagnostics

| Code | Trigger | Correction | LR |
|---|---|---|---|
| E-OBL-001 | an outcome (or any linear value) dropped at the end of a statement or block, overwritten or discarded (a never-polled future holding nothing linear and no exclusive loan may be discarded, LR 12.3) | consume it: `or fail`, `or recover v`, `catch`, `match`, `report`, move | LR 10.1 |
| E-TYP-040 | an `Outcome[T]` used where `T` is expected | consume it first: `let v = f() or fail` | LR 10.1 |
| E-OBL-002 | a path through a `catch` block ends without `recover`, `return`, `fail` or a `Never` call | end every path | LR 10.4 |
| E-ERR-001 | `fail` outside a `try` function | declare the function `try` | LR 5.4 |
| E-EFF-001 | a discharged, unreported failure in a function without `recovers` (the record carries a `correction` inserting `recovers `) | write `recovers`, or `report` the failure before `recover` | LR 11.1 |
| E-OWN-020 | a failure used after `report`, `fail` or `move` consumed it | consume it once | LR 7.4 |
| E-OBL-005 | a linear value live across a cancellation point, unless its type is `Abandonable` (as `Task` is); the future or task being awaited is not live across its own `await` | consume it before the `await`, pass it into the awaited call, or hold an `Abandonable` handle instead | LR 12.4 |
| E-OBL-006 | a `fn`/`fn mut` closure owning a linear value | `fn once` | LR 8.3 |
| E-OBL-007 | `...move` dropping a linear field | destructure and consume it | LR 9.2 |

## Correction patterns

### E-OBL-001: dropped outcome (LR 10.1)

```fortis
// reject: E-OBL-001
export error Missing {
  code: "store.missing"
}

pure try func lookup(key: Int) Int {
  if key > 3 {
    fail Missing()
  }
  return key
}

pure try func check(key: Int) Bool {
  lookup(key)                                  // reject: E-OBL-001
  return true
}

test "check" {
  expect check(1) or fail
}
```

```fortis
// accept
export error Missing {
  code: "store.missing"
}

pure try func lookup(key: Int) Int {
  if key > 3 {
    fail Missing()
  }
  return key
}

pure try func check(key: Int) Bool {
  let found = lookup(key) or fail
  return found == key
}

test "check" {
  expect check(1)
}
```

### E-TYP-040: an outcome where a value is expected (LR 10.1)

```fortis
// reject: E-TYP-040
export error Missing {
  code: "store.missing"
}

pure try func lookup(key: Int) Int {
  if key > 3 {
    fail Missing()
  }
  return key
}

pure func double(n: Int) Int => Int.saturatingMul(n, 2)

pure try func twice(key: Int) Int {
  return double(lookup(key))                   // reject: E-TYP-040
}

test "twice" {
  expect twice(2) == 4
}
```

```fortis
// accept
export error Missing {
  code: "store.missing"
}

pure try func lookup(key: Int) Int {
  if key > 3 {
    fail Missing()
  }
  return key
}

pure func double(n: Int) Int => Int.saturatingMul(n, 2)

pure try func twice(key: Int) Int {
  let v = lookup(key) or fail
  return double(v)
}

test "twice" {
  expect twice(2) == 4
}
```

### E-OBL-002: a `catch` path without an ending (LR 10.4)

The path that falls through also drops `e`: `E-OBL-001` on the same line (LR 10.1).

```fortis
// reject: E-OBL-002, E-OBL-001
export error Missing {
  code: "store.missing"
}

pure try func lookup(key: Int) Int {
  if key > 3 {
    fail Missing()
  }
  return key
}

pure recovers func safe(key: Int) Int {
  let v = lookup(key) catch e {                // reject: E-OBL-002, E-OBL-001
    if key > 100 {
      recover 0
    }
  }
  return v
}

test "safe" {
  expect safe(1) == 1
}
```

```fortis
// accept
export error Missing {
  code: "store.missing"
}

pure try func lookup(key: Int) Int {
  if key > 3 {
    fail Missing()
  }
  return key
}

pure recovers func safe(key: Int) Int {
  let v = lookup(key) catch e {
    if key > 100 {
      recover 0
    }
    recover -1
  }
  return v
}

test "safe" {
  expect safe(1) == 1
  expect safe(7) == -1
}
```

### E-EFF-001: a silent discard without `recovers` (LR 10.4, LR 11.1)

```fortis
// reject: E-EFF-001
export error Missing {
  code: "store.missing"
}

pure try func lookup(key: Int) Int {
  if key > 3 {
    fail Missing()
  }
  return key
}

pure func orZero(key: Int) Int => lookup(key) or recover 0     // reject: E-EFF-001

test "orZero" {
  expect orZero(9) == 0
}
```

```fortis
// accept
export error Missing {
  code: "store.missing"
}

pure try func lookup(key: Int) Int {
  if key > 3 {
    fail Missing()
  }
  return key
}

pure recovers func orZero(key: Int) Int => lookup(key) or recover 0

test "orZero" {
  expect orZero(9) == 0
  expect orZero(2) == 2
}
```

Alternative when the discard must not be silent: a `catch e` block whose first line is `report e` and whose last
is `recover 0`, which adds no `recovers` but needs `io`, so the function cannot be `pure` (LR 10.4, LR 10.6).

### E-OWN-020: reporting and then propagating (LR 10.6, LR 7.4)

```fortis
// reject: E-OWN-020
export error Missing {
  code: "store.missing"
}

pure try func lookup(key: Int) Int {
  if key > 3 {
    fail Missing()
  }
  return key
}

try func strict(key: Int) Int {
  let v = lookup(key) catch e {
    report e
    fail e                                     // reject: E-OWN-020
  }
  return v
}

test "strict" {
  expect strict(1) == 1
}
```

```fortis
// accept
export error Missing {
  code: "store.missing"
}

pure try func lookup(key: Int) Int {
  if key > 3 {
    fail Missing()
  }
  return key
}

try func strict(key: Int) Int {
  let v = lookup(key) catch e {
    fail annotate(move e, "strict lookup")
  }
  return v
}

test "strict" {
  expect strict(1) == 1
}
```

### Matching failures by kind (LR 10.5)

```fortis
// accept
export error NotANumber {
  code: "parse.not_a_number"
  payload { position: Int }
}

export error Empty {
  code: "parse.empty"
}

pure try func firstDigit(text: StringView) Int<0..9> {
  if text.scalarCount() == 0 {
    fail Empty()
  }
  let s = text.scalarAt(0) or fail
  let d = Scalar.digitValue(s)
  if d == null {
    fail NotANumber(position: 0)
  }
  return d
}

pure try recovers func classify(text: StringView) StringView from static {
  return match firstDigit(text) {
    success(d) => "digit"
    failure(f) => match move f {
      is NotANumber(position: p) => "not a number"
      is Empty => "empty"
      otherwise(other) => fail other
    }
  }
}

test "classify" cases {
  "digit": { text: "7x", expected: "digit" }
  "letter": { text: "x7", expected: "not a number" }
  "empty": { text: "", expected: "empty" }
} as row {
  expect classify(row.text) == row.expected
}
```

### Consuming a list of outcomes (LR 10.7)

```fortis
// accept
export error Missing {
  code: "store.missing"
}

pure try func lookup(key: Int) Int {
  if key > 3 {
    fail Missing()
  }
  return key
}

pure try func sumAll(keys: List[Int]) Int {
  let mut outcomes: List[Outcome[Int]] = List.default()
  for k in keys {
    outcomes.push(lookup(k))
  }
  let values = Outcome.collect(move outcomes) or fail
  let mut total = 0
  for v in values {
    total = Int.saturatingAdd(total, v)
  }
  return total
}

test "sumAll" {
  expect sumAll(List.of(1, 2, 3)) == 6
  let bad = sumAll(List.of(1, 9)) catch e {
    expect e.code == "store.missing"
    recover -1
  }
  expect bad == -1
}
```
