# Lexical structure

<!-- covers: LR 3 -->
*Human mode. Agent mode: [Lexical structure, by rule](../agent/03-lexical.md). Source of truth: LR 3.*

This chapter is about the text of a program before it means anything: files, lines, names, keywords, literals,
strings and comments. Fortis has no semicolons, decides what a name is from its first letter, and has exactly one
kind of comment. Each of those choices keeps the page quieter and makes the parser simpler, so that a tool (or a
person) never needs to know what a name refers to in order to know how a line is built.

## Source text

A source file is UTF-8 text with the extension `.ft` (LR 3.1). Identifiers are ASCII: a letter or underscore,
then letters, digits or underscores. Unicode is welcome everywhere else: in string literals, comments and data.

```fortis
// accept
import fortis.host { Host }

export func main(args: List[String]) Void {
  let caps = Host.capabilities()
  let word = "café"
  caps.console.print("${word}: ${word.scalarCount()} scalars, ${word.byteCount()} bytes")
}
```

```output
café: 4 scalars, 5 bytes
```

A file that begins with a byte-order mark is rejected (LR 3.1). Source text is also treated as untrusted input
with quotas: a file over 16 MiB or 2,000,000 tokens, or with brackets, blocks, prefix operators or string
interpolations nested more than 256 deep, is `E-LIM-002` and is not parsed at all (LR 3.1).

## Lines and statements

A newline ends a statement. There is no semicolon in Fortis, and no rule that inserts one behind your back: a `;`
anywhere outside a string, a comment or a raw template region (a kit's template text, [chapter 16](16-kits.md)) is
`E-SYN-001` (LR 3.2, LR 16). The checker's record for it carries a correction,
the edit that deletes the semicolon:

```fortis
// reject: E-SYN-001
export func main(args: List[String]) Void {
  let a = 1; let b = 2          // reject: E-SYN-001
}
```

```output
example.ft:3:12: error E-SYN-001 [LANG-03-01] (Syntax, violated): ';' is not a token
    expected_tokens: []
    found_token: ";"
    correction: delete the semicolon
      example.ft:3:12-3:13 -> ""
```

A statement continues onto the next line in exactly three situations (LR 3.2):

- the newline is inside a `(…)`, `[…]` or `{…}` that belongs to the same expression or argument list, which is
  how long argument lists and record literals spread over several lines;
- the line *ends* with a token that cannot end an expression: `,` `(` `[` `=` `=>`, an arithmetic, bitwise,
  comparison or logical operator, `??`, `..`, `..<` or `.`;
- the next line *starts* with one of `.` `or` `catch` `??` `else` `)` `]` `with` `where` `implements` `derive`
  `from`.

So a method chain may put each `.` at the start of a line, and a failure decision may go on its own line, which is
where a reviewer wants it:

```fortis
// accept
import fortis.host { Host }

pure try func half(n: Int) Int {
  if n % 2 != 0 { return 0 }
  return Int.div(n, 2) or fail
}

export recovers func main(args: List[String]) Void {
  let caps = Host.capabilities()
  let words: List[String] = List.of("alpha", "beta", "gamma")
  let count = words
    .reversed()
    .count()
  let missing: Int? = null
  let n = missing
    ?? 7
  let h = half(10)
    or recover -1
  let total = 1 +
    2 +
    3
  caps.console.print("${count} ${n} ${h} ${total}")
  if total > 5 {
    caps.console.print("big")
  }
  else {
    caps.console.print("small")
  }
}
```

```output
3 7 5 6
big
```

The two lists are not symmetrical, and that is on purpose: a binary operator continues a line only when it is
*last* on the line. A `+` at the start of a line would make the line before it a complete statement, so it is a
parse error rather than a silent change of meaning (LR 3.2):

```fortis
// reject: E-SYN-000
export func main(args: List[String]) Void {
  let a = 1
  let b = 2
  let total = a
    + b                         // reject: E-SYN-000
}
```

The `<` and `>` that open and close a refinement such as `Int<0..100>` are not comparison operators and do not
continue a line, so `type Score = Int<0..100>` ends where it looks like it ends (LR 3.2).

A brace either opens a body (after a function head, `if`, `else`, a loop, `defer`, `catch e`, a declaration and
so on) or is part of an expression (a record literal or record type). A body may be empty, span many lines, or sit
on one line as `{ statement }`, as `{ return 0 }` does above (LR 3.2). `return`, `fail` and `give` take their
operand on the same line; a bare `return` on its own line returns `Void` (LR 3.2).

A token sequence that is not a sentence of the grammar, and that no more specific syntax code describes, is
`E-SYN-000` (LR 3.2).

## Names: the first letter decides

Whether an identifier names a type or a value is decided by its first character, with no symbol table (LR 3.3):

| First character | Names | Examples |
|---|---|---|
| uppercase letter | a type, interface, mixin, enum, variant, contract, error kind or observation schema | `Invoice`, `List`, `Draft` |
| lowercase letter or `_` | a value, function, method, field, parameter, module alias or label | `invoice`, `total`, `text` |

This is what lets the parser read `List[Int]` as a generic type and `list[i]` as indexing, and `Int<0..9>` as a
refinement where `a < b` is a comparison, without knowing what any name refers to (LR 3.3). Module path segments
and package names are lowercase (`acme.billing`), and constants are ordinary lowercase names declared with
`const` (LR 3.3).

## Keywords

Keywords are reserved: none of them can name a declaration, a binding or a parameter (`E-SYN-002`, LR 3.4).

```fortis
// reject: E-SYN-002
export func main(args: List[String]) Void {
  let match = 1                 // reject: E-SYN-002
}
```

The reserved words include the declaration heads that kits add, such as `model`, `component`, `context` and
`state`, so that no module in any kit can collide with them (LR 3.4, LR 16). That surprises people when the
natural name for a local is `state` or `view`; the checker reports it like any other keyword.

There is one relaxation. After a `.`, and as the name of a method or associated function, a keyword is an
ordinary name (LR 3.4). That is how the standard library spells `String.from(view)`, and your own types may do the
same:

```fortis
// accept
import fortis.host { Host }

class Celsius {
  export degrees: Int

  // `from` is a keyword, but a method or associated function may use it as its name.
  export pure func from(degrees: Int) Celsius => Celsius(degrees: degrees)
}

export func main(args: List[String]) Void {
  let caps = Host.capabilities()
  let reading = Celsius.from(21)
  caps.console.print("${reading.degrees} degrees")
}
```

```output
21 degrees
```

[Appendix A](appendix-a-keywords.md) lists every keyword and the words that look like keywords but are not
(`launch`, `retain`, `validate` and the rest).

## Literals

Each literal has a form and a type (LR 3.5):

```fortis
// accept
import fortis.host { Host }

export func main(args: List[String]) Void {
  let caps = Host.capabilities()
  let million = 1_000_000
  let mask = 0xFF
  let flags = 0b1010
  let mode = 0o17
  let ratio = 1.5
  let big = 2e3
  let single = 3.0f32
  let price = 19.99d
  let ready = true
  let wait = 250ms
  let later = 2min
  let none: Int? = null
  caps.console.print("${million} ${mask} ${flags} ${mode}")
  caps.console.print("${ratio} ${big} ${single} ${price} ${ready}")
  caps.console.print("${wait} ${later} ${none ?? -1}")
}
```

```output
1000000 255 10 15
1.5 2000.0 3.0 19.99 true
250ms 2min -1
```

- An integer literal (decimal, `0x`, `0b` or `0o`, with `_` between digit groups) takes its type from context,
  and is an `Int` when nothing says otherwise. Its value must fit the type it lands in, refinement included,
  and the checker knows the value exactly, so a literal that does not fit is rejected at compile time (LR 3.5).
- A float literal needs a `.` or an exponent; `f32` makes a `Float32` (LR 3.5).
- `d` makes a `Decimal`, an exact decimal number, which displays with exactly its scale (LR 3.5, LR 6.5).
- `250ms`, `3s`, `2min` and `1h` are `Duration` literals (LR 3.5).
- `null` is the absent case of an optional type (LR 3.5, LR 6.8).

```fortis
// reject: E-REF-001
export func main(args: List[String]) Void {
  let small: Int<0..10> = 11    // reject: E-REF-001
}
```

A byte string `b"abc"` is a `BytesView` of static bytes (LR 3.5):

```fortis
// accept
import fortis.host { Host }

export func main(args: List[String]) Void {
  let caps = Host.capabilities()
  let magic = b"FTS1"
  caps.console.print("${magic.byteCount()} bytes")
}
```

```output
4 bytes
```

## Strings and interpolation

A string literal without interpolation is a `StringView from static`: a view of text stored in the program, which
costs nothing to use (LR 3.5). A literal with `${…}` in it builds a new `String`, which allocates. That `String`
can be passed straight to anything that reads a `StringView`, but a binding declared as `StringView` cannot hold
it, because there would be nothing for the view to point into once the statement ends (`E-TYP-040`, LR 3.5):

```fortis
// reject: E-TYP-040
export func main(args: List[String]) Void {
  let name = "ada"
  let fixed: StringView = "hello"
  let built: String = "hello, ${name}"
  let wrong: StringView = "hello, ${name}"     // reject: E-TYP-040
}
```

A raw string, `"""…"""`, has no interpolation and no escapes, so `${` inside it is just text (LR 3.5).

Each interpolated expression is evaluated exactly once, left to right with the rest of the string, and must have a
type that implements `Displayable` (LR 3.6):

```fortis
// accept
import fortis.host { Console, Host }

func note(console: Console, label: StringView, value: Int) Int {
  console.print("evaluating ${label}")
  return value
}

export func main(args: List[String]) Void {
  let caps = Host.capabilities()
  caps.console.print("${note(caps.console, "a", 1)} then ${note(caps.console, "b", 2)}")
  caps.console.print("""a raw ${string}""")
}
```

```output
evaluating a
evaluating b
1 then 2
a raw ${string}
```

A value whose type is not `Displayable` cannot be interpolated; the record names `Displayable` as the expected
type (`E-TYP-040`, LR 3.5). The same applies to the result of a fallible call: an interpolated expression cannot
fail or suspend, and an `Outcome` is not displayable, so write the call on its own line and interpolate the
binding (LR 3.6):

```fortis
// reject: E-TYP-040
import fortis.host { Host }

class Opaque {
  export n: Int
}

try func risky(n: Int) Int => n

export try func main(args: List[String]) Void {
  let caps = Host.capabilities()
  let o = Opaque(n: 1)
  caps.console.print("${o}")                // reject: E-TYP-040
  caps.console.print("${risky(2)}")         // reject: E-TYP-040
  let r = risky(2) or fail
  caps.console.print("${r}")
}
```

Interpolation never escapes text for HTML or any other context; that is the job of the Web UI kit's templates
(LR 3.6).

## Comments

There is one comment form: `//` to the end of the line. There are no block comments and no separate
documentation-comment syntax (LR 3.7):

```fortis
// reject: E-SYN-000
/* not a comment in Fortis */  // reject: E-SYN-000
export func main(args: List[String]) Void {
}
```

A run of `//` lines directly above a declaration, with no blank line in between, is that declaration's
documentation (LR 3.7). `fortis view --decls` lists each declaration with it:

```fortis
// accept
import fortis.host { Host }

// The number of whole minutes in a number of seconds.
// Rounds down: 119 seconds is one minute.
export pure func minutes(seconds: Int<0..86400>) Int => seconds / 60

export func main(args: List[String]) Void {
  let caps = Host.capabilities()
  caps.console.print("${minutes(119)}")
}
```

```output
1
```

A fenced `fortis` block inside documentation is compiled as an example and must start with one of the markers
this guide uses (`// accept`, `// reject: …`, `// execute: …` or `// illustrative`); a marker directly above a
declaration never becomes its documentation (LR 3.7).
