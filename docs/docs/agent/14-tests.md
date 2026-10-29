# Test declarations (LR 14)

<!-- covers: LR 14 -->
*Agent mode. Human mode: [Tests](../guide/14-tests.md). Source of truth: LR 14.*

> Preview: property tests and shrinking (LR 14) arrive in v0.2. Example tests and cases tables are in v0.0.1.

## Rules

| Rule | Statement | LR |
|---|---|---|
| LANG-14-01 | A table test binds its rows explicitly: `test "name" cases { "row": { f: v, … } … } as row { … }`; no implicit names enter scope. | LR 14 |
| LANG-14-02 | `expect e` takes a `Bool`, records a typed actual/expected pair, and fails the test when false. | LR 14 |
| LANG-14-03 | In `test` and `property` bodies `or fail` is implied: a call of a fallible function that is not the left operand of `or` or `catch` or a `match` scrutinee has its success type, and its failure fails the test with the failure's record. `let o = f()` keeps the outcome when the body later consumes `o` with `or`, `catch` or `match`, or annotates it `Outcome[…]`. Integer arithmetic the checker cannot prove safe is a checked region with the same implied `or fail` (LR 6.4): write `expect a * b == c`. A lambda in the body keeps its own rules; outside tests nothing changes (X-345). | LR 14 |

Further statements of LR 14:

- Tests are core declarations; no kit is needed to test a package (LR 14).
- A `test` body is an implicit `try` body: `or fail` is implied after a fallible call and may still be written;
  an escaping failure fails the test (LR 14). Write `expect f() == v`, not `expect (f() or fail) == v`.
- A row's record type is inferred from the body's uses; every row literal is then checked against it at compile
  time, exactly as the same literal in that position (LR 14).
- Expecting a failure consumes its obligation inside the test (LR 14).
- `property "name" forall (x: T, …) { … }` draws inputs from the parameter types, refinements included (LR 14).
- Test declarations are compiled only in test builds and may import test-only modules (LR 14).
- `fuzz` and `bench` are reserved words (LR A); fuzzing is `--fuzz` over every property (X-131).

## Commands

<!-- no-citation: the toolchain's test commands and records, not language rules -->

| Command | Backend | Options |
|---|---|---|
| `fortis test FILE\|DIR` | interpreter | `--format json` |
| `fortisc test FILE\|DIR` | VM | `--test NAME`, `--case NAME`, `--seed S`, `--cases N`, `--fuzz [--budget N] [--corpus DIR]`, `--replay FILE`, `--format json` |
| `fortisc native FILE --tests` | native | builds and runs the test executable |

Records (JSON lines with `--format json` or on a pipe): `test-result` (`test`, `case`, `status`, `evidence`),
`property-failure` (`seed`, `original`, `minimal`, `shrinks`, `replay`), and a final `test-summary`
(`passed`, `failed`) (X-131). Both runners write the results on standard error and the summary on standard output.
Exit status 1 when any test fails.

## Diagnostics

| Code | Trigger | Correction | LR |
|---|---|---|---|
| E-TYP-010 | `expect` of a non-`Bool` | compare: `expect f() == v` | LR 14, LR 5.2 |
| E-SYN-000 | a `cases` table without `as name` | add `as row` and use `row.field` | LR 14, LR 3.2 |
| E-REF-001 | a row literal outside the refinement of the position its field reaches | fix the row, or widen the parameter's type | LR 14, LR 3.5 |
| E-OBL-001 | a fallible call left unconsumed outside a test body, or in a lambda written in one (its body is its own function); in the body itself `or fail` is implied | `or fail`, or, to expect the failure, a `catch e` block that inspects `e` with `expect` and ends `recover v` (one statement per line) | LR 14, LR 10.1 |

## Correction patterns

### E-TYP-010: `expect` of a non-`Bool` (LR 14, LR 5.2)

```fortis
// reject: E-TYP-010
pure func double(n: Int<0..1000>) Int => n * 2

test "doubling" {
  expect double(2)           // reject: E-TYP-010
}
```

```fortis
// accept
pure func double(n: Int<0..1000>) Int => n * 2

test "doubling" {
  expect double(2) == 4
}
```

### E-SYN-000: rows not bound with `as` (LR 14, LR 3.2)

```fortis
// reject: E-SYN-000
pure func double(n: Int<0..1000>) Int => n * 2

test "doubling" cases {
  "one": { n: 1, expected: 2 }
} {                                   // reject: E-SYN-000
  expect double(n) == expected
}
```

```fortis
// accept
pure func double(n: Int<0..1000>) Int => n * 2

test "doubling" cases {
  "one": { n: 1, expected: 2 }
  "ten": { n: 10, expected: 20 }
} as row {
  expect double(row.n) == row.expected
}
```

### E-REF-001: a row outside the parameter's refinement (LR 14, LR 3.5)

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

```fortis
// accept
pure func percent(part: Int<0..100>) Int => part

test "percent" cases {
  "low":  { part: 0, expected: 0 }
  "high": { part: 100, expected: 100 }
} as row {
  expect percent(row.part) == row.expected
}
```

### E-OBL-001: an outcome dropped outside a test (LR 10.1, LR 14)

In a test or property body `or fail` is implied after a call of a fallible function (LANG-14-03, X-345). Outside
tests it is not: an outcome nothing consumes is dropped.

```fortis
// reject: E-OBL-001
export error Empty {
  code: "cart.empty"
}

pure try func average(total: Int<0..100000>, count: Int<0..1000>) Int {
  if count == 0 {
    fail Empty()
  }
  return total / count
}

func touch() {
  average(10, 0)             // reject: E-OBL-001
}
```

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

test "empty" {
  let r = average(10, 0) catch e {
    expect e.code == "cart.empty"
    recover -1
  }
  expect r == -1
  expect average(10, 5) == 2
}
```

## Patterns

Property over a refined domain (LR 14):

```fortis
// accept
pure func clampTo(n: Int<0..1000>, high: Int<0..1000>) Int {
  if n > high {
    return high
  }
  return n
}

property "clamp never exceeds the bound" forall (n: Int<0..1000>, high: Int<0..1000>) {
  expect clampTo(n, high) <= high
}
```

- Replay a failing property: run the `replay` command of its `property-failure` record (`--test`, `--seed`,
  `--case`) with `fortisc test` (X-131).
- Never change an expected value, remove an `expect` or weaken a property to make a test pass.
