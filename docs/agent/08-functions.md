# Functions and closures (LR 8)

<!-- covers: LR 8 -->
*Agent mode. Human mode: [Functions and closures](../guide/08-functions.md). Source of truth: LR 8.*

## Rules

| Rule | Statement | LR |
|---|---|---|
| LANG-08-01 | Every parameter has a type and the success type is written, except that a function or method head leaves out `Void` and a lambda's are inferred as LR 8.2 allows; modifiers in the order `[export] [Contract …] [pure] [noalloc] [async] [try] [recovers] func`; aspects on the lines above. | LR 8.1 |
| LANG-08-02 | A directly loaned literal with no written captures or kind infers both (LR 8.2); stored, returned, field and `owned` argument lambdas list captures and kind. Inference must meet the callback type; a body needing a stronger written kind is rejected. | LR 8.2 |
| LANG-08-03 | A name not in the written or inferred capture list is inaccessible in the lambda body. | LR 8.3 |
| LANG-08-04 | Callable assignability: kind `fn` ≤ `fn mut` ≤ `fn once`; parameter modes match exactly; modifiers at least as strong; an owned closure's region at least as long. The backend boxes a closure only where `dynamic fn` is written. | LR 8.4 |

## Facts

- Expression body: `func f(x: T) R => expr`, exactly one expression (LR 8.1).
- A `Void` result is left out of every function and method head, expression bodies and `cleanup` included:
  `func reset(c: editable Counter) { … }`. A function type keeps it (`fn(Int) Void`), a block-bodied
  lambda may omit it when its result is `Void`. Older code writes `) Void {`; `fortis fmt` drops it (LR 8.1).
- A named function expression becomes a non-capturing `fn` with its declared signature, including in an
  unannotated binding and through every import form. Written owner arguments or a complete expected function
  type fix generic parameters. Remaining unresolved parameters are `E-TYP-040`: call directly or wrap a typed
  lambda. A generic derived `default` value needs written owner arguments. Instance method values, including newtype and interface methods, require a direct call or a lambda
  with captures. A payload variant constructor needs a complete expected function type (LR 8.1, LR 9.7).
- `try`, `async`, `recovers` must be written when the summary has them (functions and lambdas); `pure` and
  `noalloc` are optional claims, inferred for lambdas; the formatter prints no inferred one back on a lambda and
  keeps one the author wrote (the callee's function type states them and the checker enforces it there) (LR 8.1,
  LR 8.2, X-345).
- Lambda shape below; `=> expr` for an expression body; a block body writes its non-`Void` return type, an expression body
  may leave it out: it is the callee's (or annotation's) result type when that names one, else the body's type
  widened as an unannotated `let` (LR 8.2, X-345); parameter types may be omitted only when passed directly to a
  parameter of known function type; lambdas are not generic (LR 8.2).

```fortis
// illustrative
[pure] [noalloc] [async] [try] [recovers] fn[captures](params) ReturnType { body }
fn(params) ReturnType => expression
fn(params) => expression
```

- A literal directly passed to a non-`owned` function parameter, with no written list or `mut`/`once`,
  infers captures and kind. Read a trivial local/parameter: copy; read another place: alias; write or
  pass editable: editable capture and `fn mut`. Repeated uses capture once, editing wins; distinct field
  paths stay distinct and loan their root. Ordinary loan/region/authority rules still apply. Moving out
  needs explicit captures and `fn once` (`E-FUN-010`). Parentheses do not change eligibility. Stored,
  returned, field, `owned` argument, or explicitly marked lambdas use written captures (LR 8.2, LR 8.3).
- Kinds (LR 8.2):

  | Kind | Captures may be | Call | Binding | Lowers to |
  |---|---|---|---|---|
  | `fn` | `copy`, `alias`, `move` (read-only use) | `f(x)` | `let` | `Fn` |
  | `fn mut` | plus `editable`, and mutated `move` captures | `f(x)`, an exclusive loan of `f` | `let mut` | `FnMut` |
  | `fn once` | plus `move` captures moved out | `move f(x)`, once | `let` | `FnOnce` |

- A `fn mut` call needs no `editable` where the binding shows it may change (`let mut`, an `editable` parameter or
  capture), and is checked as the marked call; through a `let` it is `E-OWN-001`, marked or not. Older code writes
  `editable f(x)` (LR 8.2, LR 7.5).
- A `move` capture alone does not require `fn once`; moving the captured value out does (LR 8.2).
- Capture entries: `copy x`, `alias x`, `editable x`, `move x`; a trivial value may be bare (`[n]` = copy).
  Evaluated left to right when the lambda is evaluated. Module `const`s, functions and types are not captures
  (LR 8.3).
- Capturing a field path (`alias self.lines`) captures that path and loans the root owner for the closure's
  lifetime; two `editable` captures of one owner, or `editable` plus any other capture of it, conflict (LR 8.3).
- A loan-holding closure passed to a plain `fn` parameter is a read loan for the call: no region in the type.
  Owned positions (`owned fn` parameter, return) must name its region: `fn(Int) Bool from limit`; a field never
  holds one (LR 8.3, LR 7.7).
- A closure capturing a linear value by `move` is linear: it must be `fn once` and be called exactly once on every
  path, or passed to an `owned fn once` parameter (LR 8.3).
- `fn() (StringView from s) from s`: when the result is loan-carrying, `from` after it belongs to the result; the
  closure's own region goes on a parenthesised result (LR 8.4).
- `dynamic fn(…) R`: boxed, owned, allocates on creation; the only way to hold closures of different concrete
  types in one collection (LR 8.4).

## Diagnostics

| Code | Trigger | Correction | LR |
|---|---|---|---|
| E-TYP-040 | incomplete function or enum constructor signature, instance method used as a value, or callback parameter mode differing from the function type | fix all type parameters through written arguments or a complete expected type; call directly or wrap a typed lambda with the required captures; match parameter modes exactly | LR 8.1, LR 8.4, LR 9.7 |
| E-FUN-010 | the body needs a stronger kind than declared (a `fn` mutates a capture, a `fn mut` moves one out); reported at the use, with a `correction` edit to the kind | strengthen the kind (`fn` → `fn mut` → `fn once`) | LR 8.2 |
| E-FUN-011 | a name used in the body is not in the written or inferred capture list | add the capture with the right marker | LR 8.3 |
| E-OWN-010 | two captures of one owner where one is `editable` | capture once, or narrow to disjoint field paths read-only | LR 8.3 |
| E-OBL-006 | a `fn` or `fn mut` captures a linear value by `move` | declare `fn once` and call it once with `move f()` | LR 8.3 |
| E-OWN-001 | a `fn mut` called through a binding that cannot change (a `let`), marked or not; a `fn once` call without `move` | hold the `fn mut` in a `let mut` and call `f(…)`; write `move f(…)` | LR 7.5 |
| E-OWN-020 | a `fn once` called a second time (the first call moved it) | call it once; restructure | LR 7.4 |
| E-OWN-011 | a captured place used while the closure holds an exclusive loan of it | end the closure's block before using the place | LR 8.3 |

Modifiers appear in exactly the order of LR 8.1; any other order is a canonicalisation error that the formatter
fixes (LR 8.1). Today's tools disagree with that rule: both checkers report an out-of-order head as `E-SYN-000`
(LR 3.2) and `fortis fmt` refuses the file, a disagreement recorded in X-153 and D-59. Write modifiers in order.

## Correction patterns

### E-FUN-010: kind too weak (LR 8.2)

```fortis
// reject: E-FUN-010
func countCalls() Int {
  let mut calls = 0
  {
    let bump = fn[editable calls]() Void {
      calls = Int.saturatingAdd(calls, 1)        // reject: E-FUN-010
    }
  }
  return calls
}

test "calls" {
  expect countCalls() == 0
}
```

```fortis
// accept
func countCalls() Int {
  let mut calls = 0
  {
    let mut bump = fn mut[editable calls]() Void {
      calls = Int.saturatingAdd(calls, 1)
    }
    bump()
    bump()
  }
  return calls
}

test "calls" {
  expect countCalls() == 2
}
```

### E-FUN-011: stored lambda missing a capture (LR 8.3)

Write `fn[limit]` on the stored lambda, or pass the literal directly as below so captures are inferred.

```fortis
// reject: E-FUN-011
pure func countAbove(items: List[Int], limit: Int) Int {
  let above = fn(n: Int) Bool => n > limit     // reject: E-FUN-011
  return items.countWhere(above)
}

test "above" {
  expect countAbove(List.of(1, 5, 9), 4) == 2
}
```

```fortis
// accept
pure func countAbove(items: List[Int], limit: Int) Int {
  return items.countWhere(fn(n: Int) Bool => n > limit)
}

test "above" {
  expect countAbove(List.of(1, 5, 9), 4) == 2
}
```

### E-OWN-001: `fn mut` called through a binding that cannot change (LR 7.5, LR 8.2)

A `fn mut` held by a `let mut` binding is called as `add(item)`; held by a `let`, it cannot be called at all.

```fortis
// reject: E-OWN-001
func sum(items: List[Int]) Int {
  let mut total = 0
  {
    let add = fn mut[editable total](n: Int) Void {
      total = Int.saturatingAdd(total, n)
    }
    for item in items {
      add(item)                                   // reject: E-OWN-001
    }
  }
  return total
}

test "sum" {
  expect sum(List.of(1, 2, 3)) == 6
}
```

```fortis
// accept
func sum(items: List[Int]) Int {
  let mut total = 0
  {
    let mut add = fn mut[editable total](n: Int) Void {
      total = Int.saturatingAdd(total, n)
    }
    for item in items {
      add(item)
    }
  }
  return total
}

test "sum" {
  expect sum(List.of(1, 2, 3)) == 6
}
```

### E-OWN-010: conflicting captures (LR 8.3)

```fortis
// reject: E-OWN-010
func reset() Int {
  let mut count = 5
  {
    let mut both = fn mut[editable count, alias count]() Void {   // reject: E-OWN-010
      count = 0
    }
    both()
  }
  return count
}

test "reset" {
  expect reset() == 0
}
```

```fortis
// accept
func reset() Int {
  let mut count = 5
  {
    let mut clear = fn mut[editable count]() Void {
      count = 0
    }
    clear()
  }
  return count
}

test "reset" {
  expect reset() == 0
}
```

### E-OBL-006: a linear capture in a repeatable closure (LR 8.3)

The `fn mut` below also never consumes what it owns, so its binding is dropped with an obligation: `E-OBL-001`
on the same line (LR 6.12).

```fortis
// reject: E-OBL-006, E-OBL-001
export error Oops {
  code: "demo.oops"
}

pure try func risky(n: Int) Int {
  if n > 9 {
    fail Oops()
  }
  return n
}

pure recovers func later(n: Int) Int {
  let pending = risky(n)
  let mut run = fn mut[move pending]() Int {        // reject: E-OBL-006, E-OBL-001
    return 0
  }
  return run()
}

test "later" {
  expect later(3) == 3
}
```

```fortis
// accept
export error Oops {
  code: "demo.oops"
}

pure try func risky(n: Int) Int {
  if n > 9 {
    fail Oops()
  }
  return n
}

pure recovers func later(n: Int) Int {
  let pending = risky(n)
  let run = fn once[move pending]() Int {
    return move pending or recover 0
  }
  return move run()
}

test "later" {
  expect later(3) == 3
  expect later(12) == 0
}
```

### A loan-holding closure in an owned position needs its region (LR 8.3, LR 8.4)

A returned closure that holds a loan names its region in the return type (`fn(Int) Bool from limit`); without it
the function is rejected (LR 8.3). Which code reports that is not yet settled (X-152): the record names the place
and asks for `… from limit`.

```fortis
// accept
class Limit {
  export value: Int
}

func above(limit: Limit) fn(Int) Bool from limit {
  return fn[alias limit](n: Int) Bool => n > limit.value
}

test "above" {
  let limit = Limit(value: 3)
  let check = above(alias limit)
  expect check(5)
  expect !check(2)
}
```

### Callback purity (LR 8.4)

A non-`pure` callable where `pure fn` is required is a type mismatch (`expected_type` `pure fn(…)`).

```fortis
// reject: E-TYP-040
import fortis.host { Console }

pure func applyPure(f: pure fn(Int) Int, x: Int) Int => f(x)

func logged(console: Console, n: Int) Int {
  console.print("called")
  return n
}

func run(console: Console) {
  discard applyPure(fn[alias console](n: Int) Int => logged(console, n), 1)   // reject: E-TYP-040
}

test "run" {
  expect true
}
```

```fortis
// accept
pure func applyPure(f: pure fn(Int) Int, x: Int) Int => f(x)

pure func double(n: Int) Int => Int.saturatingMul(n, 2)

test "run" {
  expect applyPure(double, 4) == 8
  expect applyPure(fn(n: Int) Int => Int.saturatingAdd(n, 1), 4) == 5
}
```

## Async function values (LR 8.4, E-TYP-040)

An async function type promises that a call defers the body. A synchronous function or lambda cannot satisfy
that type, including `async try fn`. Declare the function `async`, or wrap its call in an async lambda.
This applies at every conversion site, including fields and generic arguments. The `try` rules are unchanged.

```fortis
// reject: E-TYP-040
func value() Int => 3
func main() {
  let f: async fn() Int = value   // reject: E-TYP-040
}
```

```fortis
// accept
import fortis.host { Console }
async func value() Int => 3
func plain() Int => 3
async func main(console: Console) {
  let f: async fn() Int = value
  let g: async fn() Int = async fn() Int => plain()
  let a = await f()
  let b = await g()
  console.print("${a} ${b}")
}
```

```output
3 3
```

## Callback parameter modes (LR 8.4)

Callback modes match exactly, for a lambda, named function or stored callable (`E-TYP-040`).
An untyped lambda parameter inherits its type and mode from the expected function type; a written type
uses read mode unless it says `editable` or `owned`. Inferred captures do not change this rule.

```fortis
// reject: E-TYP-040
func apply(value: owned String, f: fn(owned String) Int) Int => f(move value)
func count() Int {
  return apply("tea", fn(s: String) => s.scalarCount())   // reject: E-TYP-040
}
```

```fortis
// accept
func apply(value: owned String, f: fn(owned String) Int) Int => f(move value)
func count() Int {
  return apply("tea", fn(s) => s.scalarCount())
}
test "the untyped parameter inherits owned mode" {
  expect count() == 3
}
```

## A function value needs a complete signature (LR 8.1, E-TYP-040)

An array constructor needs its element type and length before it can be stored as a function value.

```fortis
// reject: E-TYP-040
func main() {
  let make = Array.filled   // reject: E-TYP-040
}
```

Write the owner arguments, provide a complete expected function type where it fixes the signature, or wrap the
call in a lambda with typed parameters:

```fortis
// accept
import fortis.host { Console }
func main(console: Console) {
  let make = fn(n: Int) Array[Int, 2] => Array[Int, 2].filled(n)
  let values = make(7)
  console.print("${values.count()}")
}
```

```output
2
```
