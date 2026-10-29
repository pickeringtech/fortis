# Tests

<!-- covers: LR 14 -->
*Human mode. Agent mode: [Test declarations, by rule](../agent/14-tests.md). Source of truth: LR 14.*

> Preview: property tests and shrinking (LR 14) arrive in v0.2. Example tests and cases tables are in v0.0.1.

Tests in Fortis are declarations, part of the core grammar, written in the same module as the code they test.
Every package can be tested without any kit, and the same runner handles example tests, tables, properties and
fuzzing (LR 14). Test declarations are compiled only in test builds, so they cost a production binary nothing,
and they may import modules meant only for tests (LR 14).

## A first test

A `test` has a name and a body. The body behaves like the body of a `try` function, and a failure that escapes
it fails the test; a call that can fail needs no `or fail` there, because its failure fails the test anyway
([`or fail` is implied](#or-fail-is-implied), LR 14). `expect condition` checks a `Bool`, records what it
compared, and fails the test when the condition is false (LR 14).

```fortis
// accept
export error Negative {
  code: "demo.negative"
}

pure try func root(n: Int<-10000..10000>) Int {
  if n < 0 {
    fail Negative()
  }
  let mut r: Int<0..101> = 0
  while r < 100 && (r + 1) * (r + 1) <= n {
    r = r + 1
  }
  return r
}

test "square roots round down" {
  expect root(16) == 4
  expect root(17) == 4
  let zero = root(0)
  expect zero == 0
}
```

`expect` takes a `Bool` and nothing else, the same rule as an `if` condition (LR 14, LR 5.2):

```fortis
// reject: E-TYP-010
pure func double(n: Int<0..1000>) Int => n * 2

test "doubling" {
  expect double(2)           // reject: E-TYP-010
}
```

The record an `expect` writes carries the actual and the expected value, so a failing test says what it saw
instead of only "assertion failed" (LR 14).

## Expecting a failure

To test that something fails, consume its failure inside the test with `catch` and inspect it there. Expecting a
failure consumes its obligation, exactly as handling it in ordinary code would (LR 14, LR 10.4):

```fortis
// accept
export error Empty {
  code: "cart.empty"
}

pure try func average(total: Int<0..100000>, count: Int<0..1000>) Int {
  if count == 0 {
    fail Empty()
  }
  return total / count
}

test "an empty cart has no average" {
  let result = average(10, 0) catch e {
    expect e.code == "cart.empty"
    recover -1
  }
  expect result == -1
}

test "a full cart averages" {
  expect average(10, 4) == 2
}
```

To check a failure's payload as well as its code, match the failure inside the `catch` block through a read loan
(`match e`, not `match move e`): that is inspection only, so it consumes nothing, and the block still ends in
`recover` (LR 10.5, LR 10.4):

```fortis
// accept
export error NotEven {
  code: "demo.not_even"
  payload { value: Int }
}

pure try func half(n: Int<0..1000>) Int {
  if n % 2 == 1 {
    fail NotEven(value: n)
  }
  return n / 2
}

test "an odd number is refused with its value" {
  let r = half(7) catch e {
    expect e.code == "demo.not_even"
    match e {
      is NotEven(value: v) => {
        expect v == 7
      }
      otherwise(other) => {
        expect false
      }
    }
    recover -1
  }
  expect r == -1
}
```

## Tables

When one check runs over many inputs, write the inputs as a table. `cases { … }` holds named rows, and `as row`
binds each row explicitly to the name you choose; nothing else enters scope (LR 14).

```fortis
// accept
pure func grade(score: Int<0..100>) StringView from static {
  if score >= 70 {
    return "distinction"
  }
  if score >= 40 {
    return "pass"
  }
  return "fail"
}

test "grade boundaries" cases {
  "zero":            { score: 0, expected: "fail" }
  "just below pass": { score: 39, expected: "fail" }
  "pass":            { score: 40, expected: "pass" }
  "distinction":     { score: 70, expected: "distinction" }
  "top":             { score: 100, expected: "distinction" }
} as row {
  expect grade(row.score) == row.expected
}
```

Each row is its own case with its own result, so one wrong row does not hide the others. The type of `row` is
inferred from how the body uses its fields, and then every row is checked against it at compile time (LR 14).
Because `row.score` is passed to an `Int<0..100>` parameter, a row with `score: 150` is checked exactly as the
literal `150` would be there, and rejected as out of range (`E-REF-001`, LR 14, LR 3.5):

```fortis
// reject: E-REF-001
pure func percent(part: Int<0..100>) Int => part

test "percent" cases {
  "low":      { part: 0, expected: 0 }
  "too high": { part: 150, expected: 150 }     // reject: E-REF-001
} as row {
  expect percent(row.part) == row.expected
}
```

The binding is required: a table without `as name` is not a sentence of the grammar (`E-SYN-000`, LR 14, LR 3.2),
because a test whose body used `score` with no visible declaration would be one more thing a reader has to guess.

```fortis
// reject: E-SYN-000
pure func double(n: Int<0..1000>) Int => n * 2

test "doubling" cases {
  "one": { n: 1, expected: 2 }
} {                                   // reject: E-SYN-000
  expect double(n) == expected
}
```

## Properties

A property states something that must hold for every input of a type. The runner draws the inputs from the
parameter types, refinements included, so `Int<0..1000>` only ever produces values in that range (LR 14):

```fortis
// accept
pure func clampTo(n: Int<0..1000>, high: Int<0..1000>) Int {
  if n > high {
    return high
  }
  return n
}

property "clamping never exceeds the bound" forall (n: Int<0..1000>, high: Int<0..1000>) {
  expect clampTo(n, high) <= high
}

property "reversing twice is the identity" forall (items: List[Int<0..100>]) {
  expect items.reversed().reversed() == items
}
```

When a property fails, the runner shrinks the input to a minimal one that still fails the same way, inside the
valid domain, and reports it with the seed and the command that replays it (LR 14, X-131).

## Fuzzing

`fortisc test --fuzz` turns every property into a coverage-guided fuzz target (the interpreter's `fortis test`
does not take `--fuzz` yet, D-59): it mutates inputs from a corpus,
keeps any input that reaches a new branch, and saves the corpus beside the package so the next run starts from it
(LR 14, X-131). There is no separate fuzz declaration; the keyword `fuzz` stays reserved (X-131).

## Running tests

Each backend has a command (LR 14 names the runner):

| Command | Runs the tests on | Notes |
|---|---|---|
| `fortis test FILE` or `fortis test DIR` | the definitional interpreter | the reference behaviour |
| `fortisc test FILE` or `fortisc test DIR` | the development VM | the full runner: `--test NAME`, `--case NAME`, `--seed S`, `--fuzz`, `--replay FILE`, `--cases N` |
| `fortisc native FILE --tests` | native code | builds the tests into an executable and runs it |

While you iterate, run the tests on the VM: the interpreter is the reference behaviour, and it is much slower
(about a hundred times on a large package), so `fortisc test` gives the fast loop and `fortis test` the final word.

Every positive example in this guide with tests is run on all three; the few that a backend still fails are
listed against their recorded defects (D-53, D-54, D-55), and the guide shows what the language requires.
Selecting, replaying and fuzzing work in `fortisc test`: a failing property's record names the case and its
seed, and `fortisc test FILE --test NAME --seed S` reruns exactly that case. The record writes the command as
`fortis test …`, but the interpreter's `fortis test` does not yet take these options and does not shrink (D-59),
so run them with `fortisc test`.

## `or fail` is implied

A test can only react to a failure it did not expect by failing, so in `test` and `property` bodies `or fail` is
implied: a call of a fallible function has its success type, and its failure fails the test with its record
(LR 14). So `expect root(16) == 4` checks the root, and fails the test if `root` fails. A call that `or`, `catch`
or `match` consumes keeps its outcome, so expecting a failure is as explicit as ever, and outside tests nothing
changes. Integer arithmetic works the same way: an operation the checker cannot prove safe fails the test if it
overflows, so `expect a * b == c` needs no handler (LR 6.4). Older tests write `expect (root(16) or fail) == 4`;
the explicit form is still accepted, and `fortis fmt` rewrites it.

```fortis
// accept
export error Negative {
  code: "demo.negative"
}

pure try func root(n: Int<-10000..10000>) Int {
  if n < 0 {
    fail Negative()
  }
  let mut r: Int<0..101> = 0
  while r < 100 && (r + 1) * (r + 1) <= n {
    r = r + 1
  }
  return r
}

test "square roots, with or fail implied" {
  expect root(16) == 4
  let zero = root(0)
  expect zero == 0
}
```
