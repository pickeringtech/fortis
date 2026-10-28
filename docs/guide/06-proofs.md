# Proving arithmetic and indexing

<!-- covers: LR 6.3, LR 6.4, LR 6.7 -->
*Human mode. Agent mode: [Proofs, by rule](../agent/06-proofs.md). Source of truth: LR 6.3, LR 6.4 and LR 6.7.*

> Preview: predicate refinements (LR 6.3) arrive in v0.1. The interval rules for arithmetic and indexing (LR 6.4, 6.7) are in v0.0.1.

An integer operator in Fortis never overflows, never divides by zero and never shifts out of range, and an index
never falls outside its list. The checker guarantees this before the program runs, with a small, fixed set of
rules about the range of values each integer can hold. This chapter explains those rules: where the checker
learns a range, how conditions narrow it, what makes it forget, and what to write when it cannot prove what you
need. There is no solver behind them; what the rules below do not prove, the checker does not prove (LR 6.3,
LANG-06-02).

Language 1.0 has exactly these rules. A later edition may let the checker prove more (spike SP-18, register
X-146), but it will never make a rejected program's meaning change: it can only accept more programs.

## Why integer operators need a proof

The operators `+ - * / % << >>` and unary `-` on integers are **proof-required**: the checker accepts one only
when it can show that overflow, division by zero, a shift out of range, and `MIN / -1` cannot happen. Otherwise
the operation is rejected with `E-REF-002`, and the record names the operands' ranges and the checked operation to
use instead (LR 6.4). Here `a` and `b` can be any `Int`, so their sum can overflow, and the checker says so:

```fortis
// reject: E-REF-002
pure func total(a: Int, b: Int) Int => a + b      // reject: E-REF-002

export func main(args: List[String]) Void {
}
```

```output
example.ft:2:42: error E-REF-002 [LANG-06-02] (Refinement unproved, unproved): `+` may overflow Int: operands [-9223372036854775808, 9223372036854775807] and [-9223372036854775808, 9223372036854775807] give [-18446744073709551616, 18446744073709551614]; use Int.add
    proved_interval: "[-18446744073709551616, 18446744073709551614]"
    required_range: "[-9223372036854775808, 9223372036854775807]"
    widening_point: ""
```

The status `unproved` matters: the checker does not claim the program is wrong, only that it could not prove it
right (Programme 7.1). A record whose status is `violated` is different: the checker proved the rule broken, as
for a literal outside the target's range or a change to a length-refined collection, and the program must change.
For an unproved operator there are two ways forward. Tell the checker the ranges you mean, with refined types, and
the operator is proved:

```fortis
// accept
import fortis.host { Host }

pure func total(a: Int<0..1000000>, b: Int<0..1000000>) Int => a + b

export func main(args: List[String]) Void {
  let caps = Host.capabilities()
  caps.console.print("${total(400000, 250000)}")
}
```

```output
650000
```

Or, when the values really can be anything, use the checked operation the record names. `Int.add(a, b)` needs no
proof: it returns an outcome that fails with `numeric.overflow` instead of overflowing (LR 6.4):

```fortis
// accept (exit status 1)
import fortis.host { Host }

try func total(a: Int, b: Int) Int {
  let sum = Int.add(a, b) or fail
  return sum
}

export try func main(args: List[String]) Void {
  let caps = Host.capabilities()
  let small = total(1, 2) or fail
  caps.console.print("${small}")
  let huge = total(9223372036854775807, 1) or fail
  caps.console.print("${huge}")
}
```

```output
3
```

The second call fails, so `main` ends with one failure record on standard error and exit status 1. The wrapping
(`Int.wrappingAdd`) and saturating (`Int.saturatingAdd`) families are the other choices, for code that wants
two's-complement wrapping or clamping on purpose (LR 6.4). Integer behaviour never depends on the build mode:
there is no "overflow check in debug only" (LR 6.4).

The result of a proved operation has a range too: the base type refined to the computed interval (LR 6.4). That is
why `total(a, b)` above could be returned as `Int`, and why it could also be returned as `Int<0..2000000>`.

## Where the checker learns a range

For every integer place in a function, the checker tracks an interval, a lowest and a highest value (LR 6.3):

- a **literal** has exactly its value;
- a **parameter** or a **field** has the range of its declared type: `Int<1..12>` is [1, 12], plain `Int` is the
  whole 64-bit range;
- a **call's result** has the range of the callee's declared return type, with one exception: the success value
  of a checked operation such as `Int.add(a, b)` has the range of the operation on its operands, because the call
  fails instead of overflowing (LR 6.3, X-93);
- `+`, `-` and `*` combine ranges by interval arithmetic, and `/` and `%` give the range of the quotient or
  remainder (LR 6.3).

```fortis
// accept
import fortis.host { Host }

pure func percent(part: Int<0..100>, whole: Int<1..100>) Int<0..10000> => part * 100 / whole

try func combined(a: Int<0..50>, b: Int<0..50>) Int<0..100> {
  let sum = Int.add(a, b) or fail     // the success value is in [0, 100]
  return sum
}

export try func main(args: List[String]) Void {
  let caps = Host.capabilities()
  caps.console.print("${percent(3, 4)}%")
  let both = combined(30, 40) or fail
  caps.console.print("${both}")
}
```

```output
75%
70
```

`part * 100` is in [0, 10000] and `whole` is at least 1, so the division is safe and its quotient is in [0, 10000],
which fits the return type. `Int.add` on two values in [0, 50] succeeds only with a value in [0, 100], so `sum`
can be returned as `Int<0..100>` with no further check (LR 6.3).

## Narrowing: what a condition tells the checker

A comparison in an `if`, `while` or `match` condition narrows the range of the place it compares, inside the
branch it guards (LR 6.3). The forms are `p < c`, `p <= c`, `p > c`, `p >= c`, `p == c` and `p != c`, where `p`
is a tracked place and `c` a literal, a constant or another tracked place (which is narrowed too). `&&` narrows by
both comparisons. The `else` branch learns the opposite of a single comparison. And when a branch ends by leaving
(`return`, `fail`, `break`, `continue`, or a call that never returns), the rest of the block learns the opposite as
well, which is what makes a guard clause work (LR 6.3):

```fortis
// accept
import fortis.host { Host }

pure func clamp(value: Int, low: Int<0..100>, high: Int<0..100>) Int<0..100> {
  if value < low {
    return low
  }
  if value > high {
    return high
  }
  return value              // here value is in [low, high], so within [0, 100]
}

export func main(args: List[String]) Void {
  let caps = Host.capabilities()
  caps.console.print("${clamp(-5, 0, 10)} ${clamp(7, 0, 10)} ${clamp(99, 0, 10)}")
}
```

```output
0 7 10
```

After `if value < low { return low }`, the checker knows `value >= low` for the rest of the function, and after
the second guard it knows `value <= high` (LR 6.3). Two forms do **not** narrow: `||`, and a function call such
as `isSmall(value)`, whatever the function does (LR 6.3). Write the comparison itself in the condition.

## Where paths meet, and loops

Where two paths meet, the range is the hull of the two: one path with [0, 5] and another with [10, 20] give
[0, 20] (LR 6.3).

A loop is where most surprises come from. At the top of a loop, every place the loop body assigns is widened to
the full range of its declared type, because the checker does not count iterations (LR 6.3). The one exception is
the index of a ranged `for`, which has the range of its range. So a counter keeps a useful range only if its type
says so: declare it with a refinement, `let mut i: Int<0..1000> = 0` (LR 6.3, LR 5.3).

```fortis
// accept
import fortis.host { Host }

pure func countdown(start: Int<0..100>) Int {
  let mut n: Int<0..100> = start
  let mut steps: Int<0..100> = 0
  while n > 0 {
    n = n - 1              // n > 0 here, so n - 1 is in [0, 99]
    if steps < 100 {
      steps = steps + 1    // steps < 100 here, so steps + 1 is in [1, 100]
    }
  }
  return steps
}

export func main(args: List[String]) Void {
  let caps = Host.capabilities()
  caps.console.print("${countdown(42)}")
}
```

```output
42
```

Inside the loop, `n > 0` narrows `n` to [1, 100], so `n - 1` fits `Int<0..100>`. `steps` is widened to [0, 100] at
the top of every iteration, its declared type; the `if steps < 100` then narrows it so that `steps + 1` fits.

Here is what happens without such a condition. `total` is widened to its declared range at the loop's top, so
adding the index can exceed it, and the assignment is rejected with `E-REF-001` (LR 6.3):

```fortis
// reject: E-REF-001
pure func sumTo(n: Int<0..1000>) Int<0..500500> {
  let mut total: Int<0..500500> = 0
  for i in 0..n {
    total = total + i                   // reject: E-REF-001
  }
  return total
}

export func main(args: List[String]) Void {
}
```

The repair accumulates into a plain `Int` with the checked operation, and converts once at the end with
`validate`, which fails if the value does not fit (LR 6.3):

```fortis
// accept
import fortis.host { Host }

pure try func sumTo(n: Int<0..1000>) Int<0..500500> {
  let mut total: Int = 0
  for i in 0..n {
    total = Int.add(total, i) or fail
  }
  return Int<0..500500>.validate(total) or fail
}

export try func main(args: List[String]) Void {
  let caps = Host.capabilities()
  let s = sumTo(1000) or fail
  caps.console.print("${s}")
}
```

```output
500500
```

## What makes the checker forget

A fact about a place is discarded when something else may have changed it: when the place, a prefix of it or a
field of it is passed as an `editable` argument or receiver, is loaned exclusively, or is reachable from an
`editable` capture of a closure that is called (LR 6.3). The place then has the range of its declared type again.
Assigning to the place replaces its facts with the range of the new value. Inside a loop, a place invalidated
anywhere in the body is widened at the top, like an assigned place (LR 6.3).

```fortis
// reject: E-REF-001
class Counter {
  export n: Int<0..10>
}

func reset(c: editable Counter) Void {
  c.n = 0
}

pure func bump(c: editable Counter) Void {
  if c.n < 10 {
    reset(editable c)
    c.n = c.n + 1                 // reject: E-REF-001
  }
}

export func main(args: List[String]) Void {
}
```

`c.n < 10` would have made `c.n + 1` fit, but `reset(editable c)` may have changed `c.n`, so after it `c.n` is
back to its declared [0, 10], and `c.n + 1` is [1, 11], which `Int<0..10>` cannot hold. Test again after the call,
or reorder the code so that the test is the last thing before the addition.

## Indexing

Indexing is proof-required too: `list[i]` is accepted only when `i` is known to be at least 0 and below the
list's length (LR 6.7). The checker knows a length in three situations (LR 6.3, LR 6.7):

- an `Array[T, N]` has length `N`;
- a list whose type is refined by its length, `List[T]<length: 0..100>`;
- after a comparison `i < p.count()` that still holds, and throughout the body of `for i in a..<p.count()` when the
  body neither assigns `p` nor loans it exclusively (LR 6.3, X-93).

These length facts give only the upper bound; the lower bound comes from `i`'s type (a range starting at 0) or a
separate `i >= 0` test. `i <= p.count()` is not enough, because `i` may equal the length (LR 6.3).

```fortis
// accept
import fortis.host { Host }

pure func positives(xs: List[Int]) Int {
  let mut found: Int<0..1000000> = 0
  for i in 0..<xs.count() {
    if xs[i] > 0 && found < 1000000 {
      found = found + 1
    }
  }
  return found
}

pure func itemOr(xs: List[Int], i: Int, fallback: Int) Int {
  if i >= 0 && i < xs.count() {
    return xs[i]
  }
  return fallback
}

export func main(args: List[String]) Void {
  let caps = Host.capabilities()
  let xs = List.of(3, -1, 4, -1, 5)
  caps.console.print("${positives(xs)} positive; xs[2] = ${itemOr(xs, 2, 0)}; xs[9] = ${itemOr(xs, 9, 0)}")
}
```

```output
3 positive; xs[2] = 4; xs[9] = 0
```

When no such fact is at hand, the index is rejected with `E-REF-002`, and the record suggests the test or the two
operations that need no proof (LR 6.7):

```fortis
// reject: E-REF-002
pure func item(xs: List[Int], i: Int) Int => xs[i]          // reject: E-REF-002

export func main(args: List[String]) Void {
}
```

`xs.get(i)` returns the element as an optional (`null` outside the list) and `xs.at(i)` returns an outcome that
fails with `collection.index`; neither needs a proof (LR 6.7).

A collection refined by its length is a property of the value, so such a place cannot be loaned `editable` or
changed: that is `E-REF-003` (LR 6.3). Change an unrefined collection, then `validate` it into the refined type.

## The remainder

For `a % d`, where `d`'s range excludes zero and `m` is the largest absolute value `d` can have, the result is in
[0, m-1] when `a` is never negative, in [-(m-1), 0] when `a` is never positive, and in [-(m-1), m-1] otherwise
(LR 6.3). That is what makes the last digit of a non-negative number provably a digit:

```fortis
// accept
import fortis.host { Host }

pure func lastDigit(n: Int<0..>) Int<0..9> => n % 10

export func main(args: List[String]) Void {
  let caps = Host.capabilities()
  caps.console.print("${lastDigit(1234)}")
}
```

```output
4
```

## When the value is outside the target's range

Assigning or returning a value whose range lies partly outside the target's refinement is `E-REF-001` (LR 6.3),
as in `sumTo` and `bump` above. The fix is a wider declared type, or an explicit `validate`, which is the point
where a value from the outside world, or from arithmetic the checker cannot follow, enters a refined type
(LR 6.3, LANG-06-03).

The rules in this chapter are the whole of what the checker proves. When they cannot reach a bound, the checked
operations and `validate` always can, and they mark the line where a reader should look (LR 6.3, LR 6.4).
