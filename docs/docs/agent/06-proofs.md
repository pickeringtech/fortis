# Proofs for arithmetic and indexing (LR 6.3, 6.4, 6.7)

<!-- covers: LR 6.3, LR 6.4, LR 6.7 -->
*Agent mode. Human mode: [Proving arithmetic and indexing](../guide/06-proofs.md). Source of truth: LR 6.3,
LR 6.4 and LR 6.7.*

Language 1.0's interval domain is exactly the rules below; nothing else is inferred (LANG-06-02, LR 6.3). SP-18's
wider rules are a later edition (X-146).

## Rules

| Rule | Statement | LR |
|---|---|---|
| LANG-06-02 | Refinement subtyping is structural; the interval domain is exactly the rules of LR 6.3, with no further inference. | LR 6.3 |
| LANG-06-03 | An untrusted value reaches a refined or nominal type only through the checked `as` conversion (also spelled `validate` in v0.1); a proved value already meets its refinement, and `Name(value)` constructs a newtype over it. | LR 6.3, 6.9, 6.13 |
| LANG-05-04 | `for i in a..<b` / `a..b` gives the index the range's interval; the index is immutable. | LR 5.3, 6.3 |

## The domain (LR 6.3)

- Tracked: every integer-typed place of a function, as an interval within its base type.
- Sources: literal = exact; parameter or field = declared type; call result = declared return type, except the
  success value of a checked operation (a checked operator, or its named function `Int.add`, `Int.sub`, `Int.mul`,
  `Int.div`, `Int.rem`, `Int.shiftLeft`), which is the operation on its operands clipped to the type (X-93).
- `+ - *`: interval arithmetic. `/` and `%`: divisor interval must exclude 0; result is the quotient or remainder
  interval. `<<`: amount within [0, width-1].
- `%` with `m` = largest `|d|`: `a` ≥ 0 gives [0, m-1]; `a` ≤ 0 gives [-(m-1), 0]; otherwise [-(m-1), m-1].
- Narrowing: conditions `p < c`, `p <= c`, `p > c`, `p >= c`, `p == c`, `p != c` (`c` a literal, a `const` or a
  tracked place, narrowed too) in `if`, `while`, `match`; `&&` narrows by both; `else` gets the negation of a
  single comparison; a true branch ending in `return`, `fail`, `break`, `continue` or a `Never` call gives the
  negation to the rest of the block. `||` and calls never narrow.
- Join: hull of the incoming intervals.
- Loop header: every place assigned or invalidated in the body is widened to its declared type; a ranged `for`
  index keeps the range's interval. Refine loop counters (`let mut i: Int<0..1000> = 0`).
- Invalidation: an exclusive loan of the place, a prefix or a field (an `editable` argument, an `editable self`
  call on it, marked or not, an `editable` binding), or an `editable` capture of a closure that is called; the
  place returns to its declared type. Assignment replaces the facts with the assigned interval.
- Length facts: `i < p.count()` true branch gives `i` below `p`'s length; in `for i in a..<p.count()` the index is
  below the length throughout a body that neither assigns nor exclusively loans `p` (X-93); `i <= p.count()` does
  not permit indexing; `i >= p.count()` / `i > p.count()` give the bound in their `else`. Upper bound only: the
  lower bound comes from `i`'s type or `i >= 0`.
- Length-refined collections (`List[T]<length: …>`) are immutable: no `editable` loan, no mutation.

## Operators and indexing (LR 6.4, 6.7)

- Proof-required: integer `+ - * / % << >>` and unary `-`; the result type is the base refined to the result
  interval. No dependence on the build mode.
- Checked operators, the alternative needing no proof (X-345): `Int` arithmetic as the left operand of
  `or fail`/`or recover`/`catch` is a checked region; each unproved `+ - * / % <<` or unary `-` in it is the checked
  operation, failing `numeric.overflow`, `numeric.divide_by_zero` or `numeric.shift_range`, the first failure
  ending the region: `value = value * 10 + d or fail`. The region stops at calls, indexes,
  fields, comparisons and other operators: `f(a + b) or fail`, `a + b == c or fail` and `let s = a + b` stay
  `E-REF-002`; write `f(a + b or fail)`, `(a + b or fail) == c`, `let s = a + b or fail`. A fully proved region
  cannot fail: `or fail` on it is `E-TYP-040`. Only `Int` has a checked family; `>>` has none.
- Named alternatives: checked `Int.add`… (`Outcome[Int]`, the same failures; older code writes
  `Int.add(a, b) or fail`, and `fortis fmt` writes the operator where the two are equivalent); wrapping
  `Int.wrappingAdd`…; saturating `Int.saturatingAdd`….
- `list[i]` needs `i` ≥ 0 and below the length (known for `Array[T, N]`, length-refined lists, and the length
  facts above). Alternatives: `get(i)` → `T?`, `at(i)` → `Outcome[T]` failing `collection.index`.

## Diagnostics

| Code | Trigger | Correction | LR |
|---|---|---|---|
| E-REF-002 | an integer operator not proved safe (overflow, zero divisor, shift amount, `MIN / -1`) outside a checked region; an index not proved in bounds | refine the operands' types or test them first; else the checked operator, `a + b or fail`; for an index, `if i >= 0 && i < xs.count()`, or `get`/`at` | LR 6.4, 6.7 |
| E-REF-001 | a value whose interval is not within the target's refinement (assignment, return, argument, literal) | widen the target type, narrow first (a guard), or `x as T or fail` (the retired spelling `T.validate(x) or fail` still accepted) | LR 6.3, 3.5, 6.13 |
| E-REF-003 | a length-refined collection loaned `editable` or mutated | mutate an unrefined collection, then convert with `move c as T` | LR 6.3, 6.13 |

The records carry `required_range`, `proved_interval` and `widening_point`. Read the `status`
field before correcting ([records](records.md)): `violated` means the checker proved the rule
broken (a literal outside the target's range, such as `20` returned as an `Int<0..10>`, or a mutation of a
length-refined collection), so the program is wrong and the value or the type must change; `unproved` means it
could not prove the bound, which is not a claim that the program is wrong, and the correction is a proof (a
refinement or a guard) or the checked alternative.

## Correction patterns

### E-REF-002: an unproved operator (LR 6.4)

```fortis
// reject: E-REF-002
pure func total(a: Int, b: Int) Int => a + b      // reject: E-REF-002

test "total" {
  expect total(1, 2) == 3
}
```

Refined operands (when the domain really is bounded):

```fortis
// accept
pure func total(a: Int<0..1000000>, b: Int<0..1000000>) Int => a + b

test "total" {
  expect total(1, 2) == 3
}
```

The checked operation (when it is not):

```fortis
// accept
try func total(a: Int, b: Int) Int {
  let sum = a + b or fail
  return sum
}

test "total" {
  expect total(1, 2) == 3
}
```

### E-REF-001: a loop-carried refined accumulator (LR 6.3)

```fortis
// reject: E-REF-001
pure func sumTo(n: Int<0..1000>) Int<0..500500> {
  let mut total: Int<0..500500> = 0
  for i in 0..n {
    total = total + i                   // reject: E-REF-001
  }
  return total
}

test "sum" {
  expect sumTo(3) == 6
}
```

```fortis
// accept
pure try func sumTo(n: Int<0..1000>) Int<0..500500> {
  let mut total: Int = 0
  for i in 0..n {
    total = total + i or fail
  }
  return total as Int<0..500500> or fail
}

test "sum" {
  expect sumTo(3) == 6
}
```

### E-REF-001: facts lost to an `editable` call (LR 6.3)

```fortis
// reject: E-REF-001
class Counter {
  export n: Int<0..10>
}

func reset(c: editable Counter) {
  c.n = 0
}

pure func bump(c: editable Counter) {
  if c.n < 10 {
    reset(editable c)
    c.n = c.n + 1                 // reject: E-REF-001
  }
}

test "bump" {
  expect true
}
```

Test after the call that invalidates:

```fortis
// accept
class Counter {
  export n: Int<0..10>
}

func reset(c: editable Counter) {
  c.n = 0
}

pure func bump(c: editable Counter) {
  reset(editable c)
  if c.n < 10 {
    c.n = c.n + 1
  }
}

test "bump" {
  let mut c = Counter(n: 7)
  bump(editable c)
  expect c.n == 1
}
```

### E-REF-002: an unproved index (LR 6.7)

```fortis
// reject: E-REF-002
pure func item(xs: List[Int], i: Int) Int => xs[i]          // reject: E-REF-002

test "item" {
  expect item(List.of(5), 0) == 5
}
```

```fortis
// accept
pure func item(xs: List[Int], i: Int) Int? {
  if i >= 0 && i < xs.count() {
    return xs[i]
  }
  return null
}

test "item" {
  let xs = List.of(5, 6)
  expect item(xs, 1) == 6
  expect item(xs, 2) == null
}
```

### Guard clauses narrow the rest of the block (LR 6.3)

```fortis
// accept
pure func clamp(value: Int, low: Int<0..100>, high: Int<0..100>) Int<0..100> {
  if value < low {
    return low
  }
  if value > high {
    return high
  }
  return value
}

test "clamp" {
  expect clamp(-5, 0, 10) == 0
  expect clamp(7, 0, 10) == 7
  expect clamp(99, 0, 10) == 10
}
```
