# Lexical structure (LR 3)

<!-- covers: LR 3 -->
*Agent mode. Human mode: [Lexical structure](../guide/03-lexical.md). Source of truth: LR 3.*

## Rules

| Rule | Statement | LR |
|---|---|---|
| LANG-03-01 | A `;` outside strings, comments and raw template regions is a parse error (`E-SYN-001`). There is no semicolon token and no automatic insertion. | LR 3.2 |
| LANG-03-02 | A newline ends a statement unless (a) it is inside `(…)`, `[…]` or `{…}` of the same expression or argument list, (b) the line ends with a trailing continuation token, or (c) the next line starts with a leading continuation token. Refinement `<` `>` are not continuation tokens. | LR 3.2 |
| LANG-03-03 | Uppercase first letter: type, interface, mixin, enum, variant, contract, error kind, observation schema. Lowercase or `_`: value, function, method, field, parameter, module alias, label. Decided lexically, no symbol table. | LR 3.3 |
| LANG-03-04 | Every keyword of LR Appendix A is reserved in every declaration and binding position (`E-SYN-002`), including the kit heads; a keyword is a valid name after `.` and as a method or associated-function name. | LR 3.4 |
| LANG-03-05 | Literal typing follows the LR 3.5 table; an out-of-range literal is rejected at compile time (`E-REF-001`). | LR 3.5 |
| LANG-03-06 | Each interpolated expression is evaluated once, left to right, must be `Displayable`, and must not be fallible or suspending. | LR 3.6 |

## Token facts

| Fact | LR |
|---|---|
| Source: UTF-8, extension `.ft`; identifiers ASCII `[A-Za-z_][A-Za-z0-9_]*`; Unicode only in strings, comments and data; a byte-order mark is rejected. | LR 3.1 |
| Quotas: a file over 16 MiB or 2,000,000 tokens, or nesting of brackets, blocks, prefix operators or interpolations deeper than 256, is `E-LIM-002` and is not parsed. | LR 3.1 |
| Trailing continuation tokens: `,` `(` `[` `=` `=>` `+` `-` `*` `/` `%` `<<` `>>` `&` `\|` `^` `&&` `\|\|` `??` `==` `!=` `<` `<=` `>` `>=` `..` `..<` `.` | LR 3.2 |
| Leading continuation tokens: `.` `or` `catch` `??` `else` `)` `]` `with` `where` `implements` `derive` `from` | LR 3.2 |
| Body brace (statement block or declaration body) vs expression brace (record literal or type): decided by the token before `{`; expression brace after `=`, `(`, `,`, `:`, `[`, `??`, `==`, `!=`, a compound assignment, `return`, `give`, `fail`, `recover`. A body may be one line: `{ statement }`. | LR 3.2 |
| `return`, `fail`, `give` take their operand on the same line; a bare `return` on its own line returns `Void`. | LR 3.2 |
| Parsing decisions P1 (one `NL` per run of blank/comment lines), P2 (longest chains), P3 (alternatives in grammar order) are normative for any parser. | LR 3.2 |
| Integer literal: decimal, `0x`, `0b`, `0o`, `_` separators; type from context, default `Int`. Float: needs `.` or exponent; `f32` suffix gives `Float32`. Decimal: `d` suffix. Duration: `ms` `s` `min` `h`. | LR 3.5 |
| `"text"` is `StringView from static`; `"a ${x}"` is a fresh `String` that a binding annotated `StringView` cannot hold (`E-TYP-040`); `"""raw"""` is `StringView from static`, no escapes, no interpolation; `b"abc"` is `BytesView from static`; `null` is the absent case of `T?`. | LR 3.5 |
| Interpolation takes a read loan of the places it names; it never escapes for HTML or any other context. | LR 3.6 |
| Comments: only `//`. Documentation is the `//` run directly above a declaration with no blank line between. A fenced `fortis` block in documentation needs a first-line marker (`// accept`, `// reject: CODE`, `// execute: R`, `// illustrative`). | LR 3.7 |

## Diagnostics

| Code | Trigger | Correction | LR |
|---|---|---|---|
| E-SYN-001 | `;` in code | delete it (the record carries the edit) | LR 3.2 |
| E-SYN-000 | not a sentence of the grammar: a line starting with a binary operator, `/* */`, a byte-order mark, a top-level statement | move the operator to the end of the previous line; use `//`; remove the mark | LR 3.2 |
| E-SYN-002 | keyword used as a declaration, binding or parameter name (`match`, `state`, `view`, `model`, …) | rename; keywords are valid only after `.` and as method or associated-function names | LR 3.4 |
| E-REF-001 | literal outside its target type or refinement | change the literal or widen the declared type | LR 3.5 |
| E-TYP-040 | interpolated expression not `Displayable` (including an `Outcome` from a fallible call); interpolated literal bound to a `StringView` | bind the result first (`let v = f() or fail`, then `${v}`); annotate `String` | LR 3.5 |
| E-LIM-002 | source over the size, token or nesting quota | split the file or reduce nesting | LR 3.1 |

## Correction patterns

### E-SYN-001: semicolon (LR 3.2)

The record carries a correction, an edit that deletes the semicolon; apply it as given.

```fortis
// reject: E-SYN-001
pure func pair() Int {
  let a = 1; let b = 2           // reject: E-SYN-001
  return a + b
}

test "pair" {
  expect pair() == 3
}
```

```output
example.ft:3:12: error E-SYN-001 [LANG-03-01] (Syntax, violated): ';' is not a token
    expected_tokens: []
    found_token: ";"
    correction: delete the semicolon
      example.ft:3:12-3:13 -> ""
```

```fortis
// accept
pure func pair() Int {
  let a = 1
  let b = 2
  return a + b
}

test "pair" {
  expect pair() == 3
}
```

### E-SYN-000: binary operator at the start of a line (LR 3.2)

Binary operators continue a line only when they are last on it.

```fortis
// reject: E-SYN-000
pure func total(a: Int<0..100>, b: Int<0..100>) Int {
  let sum = a
    + b                          // reject: E-SYN-000
  return sum
}

test "total" {
  expect total(1, 2) == 3
}
```

```fortis
// accept
pure func total(a: Int<0..100>, b: Int<0..100>) Int {
  let sum = a +
    b
  return sum
}

test "total" {
  expect total(1, 2) == 3
}
```

Leading `.`, `or`, `catch`, `??` and `else` do continue a line:

```fortis
// accept
pure try func half(n: Int) Int {
  if n % 2 != 0 { return 0 }
  return Int.div(n, 2) or fail
}

pure recovers func pipeline(fallback: Int?) Int {
  let n = fallback
    ?? 7
  let h = half(n)
    or recover -1
  return h
}

test "pipeline" {
  expect pipeline(null) == 0
  expect pipeline(10) == 5
}
```

### E-SYN-002: keyword as a name (LR 3.4)

Kit head words (`state`, `view`, `model`, `component`, `context`, `action`, `resource`, `inject`, `provide`, …)
are reserved like every other keyword.

```fortis
// reject: E-SYN-002
pure func size(state: Int) Int => state     // reject: E-SYN-002

test "size" {
  expect size(1) == 1
}
```

```records
{"class": "Syntax", "code": "E-SYN-002", "column": 16, "fields": {"expected_tokens": ["lowerIdent"], "found_token": "state"}, "file": "example.ft", "line": 2, "message": "keyword 'state' used as an identifier", "rule": "LANG-03-04", "severity": "error", "status": "violated"}
```

```fortis
// accept
pure func size(current: Int) Int => current

test "size" {
  expect size(1) == 1
}
```

A keyword is allowed as a method or associated-function name and after `.`:

```fortis
// accept
class Celsius {
  export degrees: Int
  export pure func from(degrees: Int) Celsius => Celsius(degrees)
}

test "from" {
  expect Celsius.from(21).degrees == 21
}
```

### E-REF-001: literal out of range (LR 3.5)

```fortis
// reject: E-REF-001
pure func percent() Int<0..100> {
  let p: Int<0..100> = 101       // reject: E-REF-001
  return 100
}

test "percent" {
  expect percent() == 101
}
```

```fortis
// accept
pure func percent() Int<0..100> {
  let p: Int<0..100> = 100
  return p
}

test "percent" {
  expect percent() == 100
}
```

### E-TYP-040: interpolation (LR 3.5, LR 3.6)

```fortis
// reject: E-TYP-040
try func risky(n: Int) Int => n

try func label(n: Int) String {
  return "value ${risky(n)}"     // reject: E-TYP-040
}

test "label" {
  expect label(2) == "value 2"
}
```

```fortis
// accept
try func risky(n: Int) Int => n

try func label(n: Int) String {
  let v = risky(n) or fail
  return "value ${v}"
}

test "label" {
  expect label(2) == "value 2"
}
```

```fortis
// reject: E-TYP-040
pure func greeting(name: StringView) Int {
  let text: StringView = "hello, ${name}"     // reject: E-TYP-040
  return text.byteCount()
}

test "greeting" {
  expect greeting("ada") == 10
}
```

```fortis
// accept
pure func greeting(name: StringView) Int {
  let text: String = "hello, ${name}"
  return text.byteCount()
}

test "greeting" {
  expect greeting("ada") == 10
}
```

## Facts used when writing code

- Order: interpolated expressions run once each, left to right with the string (LR 3.6).
- A string literal is free (`StringView from static`); converting it to `String` where a `String` is expected
  allocates and is the one implicit conversion (LR 6.6).
- A byte string literal `b"…"` is a `BytesView from static`; read bytes with `get(index) Byte?` and
  `byteCount()` (LR 3.5, LR 6.6).
- Documentation comments are `//` runs directly above a declaration; `fortis view --decls` lists them (LR 3.7).
