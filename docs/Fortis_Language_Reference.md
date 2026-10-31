# Fortis Language Reference

**Document:** FSPEC-0.2-LR • **Date:** 22 October 2026 • **Status:** normative reference for Fortis Language 1.0 (the core). Companion documents: `Fortis_Specification.md` (overview and index), `Fortis_Platform_Specification.md` (kits), the Delivery Programme (requirements register, gates, evidence; kept with the maintainers' planning records). The normative grammar is `reference/grammar/fortis.ebnf`.

## Contents

- [0. How to read this document](#0-how-to-read-this-document)
- [1. What Fortis is](#1-what-fortis-is)
- [2. A complete worked module](#2-a-complete-worked-module)
- [3. Lexical structure](#3-lexical-structure)
- [4. Modules and packages](#4-modules-and-packages)
- [5. Statements and expressions](#5-statements-and-expressions)
- [6. Types](#6-types)
- [7. Ownership](#7-ownership)
- [8. Functions and closures](#8-functions-and-closures)
- [9. Classes, interfaces, mixins and enums](#9-classes-interfaces-mixins-and-enums)
- [10. Outcomes and failures](#10-outcomes-and-failures)
- [11. Effects and contracts](#11-effects-and-contracts)
- [12. Cleanup, async and concurrency](#12-cleanup-async-and-concurrency)
- [13. Observation schemas and statements](#13-observation-schemas-and-statements)
- [14. Test declarations](#14-test-declarations)
- [15. Core runtime inventory](#15-core-runtime-inventory)
- [16. Reserved declaration heads for kits](#16-reserved-declaration-heads-for-kits)
- [Appendix A. Keyword inventory](#appendix-a-keyword-inventory)
- [Appendix B. Closed decisions](#appendix-b-closed-decisions)
- [Appendix C. Lowering reference and the Rust-expressible subset](#appendix-c-lowering-reference-and-the-rust-expressible-subset)
- [Appendix D. Rule index and old requirement IDs](#appendix-d-rule-index-and-old-requirement-ids)
- [Appendix E. Glossary](#appendix-e-glossary)

## 0. How to read this document

This document defines the Fortis core language: the syntax, the static rules and the execution semantics that every conforming implementation (the development VM and the Rust backend) must agree on. It does not describe HTTP, databases, web UI, telemetry pipelines or dependency injection; those are kits and live in the Platform Specification.

**Rules.** Normative rules are numbered `LANG-<section>-<nn>` and collected in a table at the end of each section with their acceptance evidence. Prose between the tables is also normative unless marked *illustrative* or *rationale*.

**Examples.** Every example is marked on its first line. `// accept` means a conforming implementation must accept the program. `// reject: E-XXX-NNN` means it must reject it with that diagnostic code at the marked line. `// execute: <result>` states the required observable result. `// illustrative` marks a fragment, a signature list or a template with `…` or bracketed placeholders; it shows a shape and is not a fixture. An `accept`, `reject` or `execute` example that is not a complete module is a statement sequence and is checked as the body of a function. The conformance tooling in `conformance/` extracts every marked block into the G0 corpus and parses it against the EBNF; no example has yet been run through a Fortis compiler, and that suite is the mechanism that will make the semantic markers binding.

**Judgements.** Static rules are written as judgements of the form `Γ ⊢ e : τ ▷ loans, effects, obligations`, read "under environment Γ, expression `e` has type `τ`, creates these loans, has these effects and leaves these obligations". The full judgement is defined in section 7.1 and extended in sections 10 and 11.

**Precedence of sources.** Where this document and the EBNF disagree on syntax, the EBNF wins and the disagreement is a defect to fix. Where this document and an example disagree, the rule text wins.

## 1. What Fortis is

Fortis is a general-purpose, statically checked, compiled programming language designed so that software written mostly by AI agents can be read, reviewed and trusted by humans. It optimises for a reader who sees a file or a diff and needs to answer, without tooling: who owns this value, can this call change it, can this call fail and who handles that, what effects does this function have, and when is this observation recorded. Every one of those questions has a visible answer in the source.

What a reader can see in the file is every *local* decision: ownership operations, mutation permission, failure disposition, effect claims and observation points. Transitive facts (what an imported function ultimately does, which provider a dependency resolves to, what a deployment plan allocates) come from imported signatures and the build configuration, which the toolchain prints next to the source; the language guarantees that those imported signatures cannot be weaker than the bodies behind them.

The language is library-first. A Fortis program is a set of packages; a package is a set of modules; a module is one file. Applications, command-line tools, servers and libraries are all built from the same core with explicit imports. Platform capabilities (HTTP, data access, web UI including SPA and PWA, telemetry, jobs) are ordinary libraries that an application imports; nothing in the core depends on them.

The core design choices are:

- **Explicit ownership.** Assigning or passing an existing owned value states `copy`, `alias` or `move`. Mutable access is a visible `editable` loan. There are no hidden clones or reference counts.
- **Three distinct answers.** An optional value (`T?`), a Boolean and a fallible outcome (`Outcome[T]`) are different types with different operators (`??`, `||`, `or`). Nothing is truthy.
- **Linear failures.** A fallible result must be consumed exactly once: propagated with `or fail`, recovered with `or recover`, handled with `catch` or `match`, or reported with `report`. Dropping an outcome is a compile error.
- **A closed effect set.** `pure`, `noalloc`, `async` and `try` are checked facts about a function, computed transitively from a closed set of eight effects. Contracts are conjunctions of them and nothing else.
- **Lexical loans and synchronous cleanup.** Loans end at the end of the block that holds them; `defer` runs at block exit; async code has one cancellation point (`await`) and structured `scope`s.
- **Two backends, one semantics.** A development VM with hot reload and a production backend that emits safe Rust (`forbid(unsafe_code)`) execute the same checked intermediate representation.

Fortis is not a scripting language, has no runtime reflection, no inheritance, no macros, no exceptions and no implicit conversions. A reader can determine every binding's type, ownership and effects from the file and the signatures it imports.

### 1.1 Notation economy

Fortis puts more words on the page than most languages, so every piece of notation has to earn its place. The rule is: **a symbol or keyword exists only if it changes what a reader may conclude about ownership, mutation, failure, effects or timing.** Decoration, repetition and punctuation that carries no decision are removed. This is the complete inventory of non-alphabetic notation and the question each answers; a construct that answers no question in this table is not in the language.

| Notation | Answers | Not used for anything else |
|---|---|---|
| `{ }` | where a block, record literal or record type begins and ends | — |
| `( )` | call, parameter list, tuple, grouping | — |
| `Name[ ]` | generic arguments after an uppercase name | — |
| `place[ ]` | index after a lowercase place | no slices |
| `fn[ ]` | the capture list of a lambda, only when it is non-empty | — |
| `+ - * / %` `<< >>` `& \| ^` | integer and float arithmetic, proof-required on integers; the operator form of the checked `Int.add` family | no overloading |
| `== !=` `< <= > >=` | `Equatable` and `Comparable`, plus IEEE comparison on floats | — |
| `&& \|\|` `!` | Boolean logic only | no truthiness |
| `+=` and friends | sugar for `x = x op e` | — |
| `< >` | a refinement on a type, in type position only | comparison in expression position only |
| `:` | "has type", named argument, record field, label | no ternary, no slicing |
| `=` | assignment; initialiser in `let` | no equality |
| `=>` | expression body of a function, lambda or match arm | no map literals |
| `.` | member access and module paths | no range (`..` is a separate token) |
| `..` `..<` | integer ranges in `for` headers, refinements and range patterns (9.8) | — |
| `?` `?.` | optional type suffix; optional chaining | never error propagation |
| `??` | absent-value fallback | — |
| `@` | apply an aspect: `@name(args)` on the line above a function head | nothing else; contracts are not applied with `@` |
| `${ }` | string interpolation | — |
| `//` | comment; documentation when directly above a declaration | — |
| `,` | separator in every list | — |
| `...` | spread of a place into a constructor; forwarding of remaining aspect arguments | — |

Keywords that look like ceremony but carry a decision: `let` (a new name, not a reassignment), `mut` (this binding changes), `copy`/`alias`/`move`/`editable` (how this place is used, including as a receiver), `owned`/`editable` on a parameter (what the callee may do), `try`/`pure`/`noalloc`/`async`/`recovers` and a named contract in the same position (what the function may do), `or fail`/`or recover`/`catch` (what happens to a failure), `give` (this block produces a value), `export` (visible outside the module). Each of these would otherwise be inferred from somewhere the reader cannot see.

Silence also has exactly one meaning per position: an unmarked argument or receiver is a read loan that ends when the call returns (or, for an `async` call, with the future it produces), an unmarked `for` source is a read loan that lasts for the loop, and an unmarked assignment or return is a fresh value or a trivial one. A reader who sees no marker knows the line neither mutates nor transfers nor keeps a loan beyond the call or the future it is in.

Two deliberate costs. `(f() or fail).amount` needs its parentheses because `or` binds loosest; the idiom is `let v = f() or fail` on its own line, which is also where a reviewer wants the failure decision to be. And an `async` call looks like a call: it builds a `Future`, and the `await` or `launch` that runs it is the visible point.

What was removed to keep noise down: semicolons; `///` and `/* */` comment forms; `yield`; `or die`; `nostate`; a separate `self: editable Self` spelling (now `editable self`, like any parameter); `alias editable` (now `editable`); `alias type` transparent aliases (so `alias` has one meaning and `type X = …` is always nominal); `within` (a stored loan says `from r` like a returned one); `give` in single-expression match arms (only block arms need it); empty `fn[]` capture lists; `::` paths; operator overloading; implicit `it`; postfix `?` for errors; `new` (constructors are named `create`, `of` or `from`); `return` in expression-bodied functions.

The formatter prints every inferred fact in canonical form, so a formatted file shows the closure kind, the loan modes and the modifiers the checker used; a reader of a formatted file never has to infer them. The one exception is a lambda's inferred `pure` and `noalloc`, which the formatter does not print back (one the author wrote is kept): the function type the lambda is passed to states what it must be, and the checker enforces it there (8.2, X-345).

## 2. A complete worked module

This module is the running example for the rest of the document. It is a small invoicing library in a package named `acme.billing`, in the file `src/invoices.ft`, so its module path is `invoices` and other packages import it as `acme.billing.invoices`.

```fortis
// accept
import fortis.text
import fortis.money { Money, Currency }

// A customer identifier. Distinct from every other Int-backed id.
export type CustomerId = Int<1..>

// A line on an invoice. All fields are exported, so the generated
// constructor Line(description: ..., quantity: ..., unitPrice: ...)
// is usable from other modules.
export class Line derive Equatable {
  export description: String
  export quantity: Int<1..10000>
  export unitPrice: Money

  // Read-only method: `self` borrows read-only by default.
  export pure try func total(self) Money {
    return Money.multiply(self.unitPrice, self.quantity) or fail
  }
}

// Invoice status is a closed enum with payloads.
export enum InvoiceStatus derive Equatable {
  Draft
  Issued(at: Timestamp)
  Paid(at: Timestamp, reference: String)
  Voided(reason: String)
}

// Declared failure kinds with stable codes.
export error EmptyInvoice {
  code: "billing.invoice.empty"
  message: "An invoice must have at least one line"
}

export error CurrencyMismatch {
  code: "billing.invoice.currency_mismatch"
  payload { expected: Currency, found: Currency }
  message: "Line currency ${found} does not match invoice currency ${expected}"
}

// An interface: anything that can be rendered to text.
export interface Renderable {
  pure func render(self) String
}

// A mixin contributes methods and requires methods, never fields.
export mixin Summarised {
  requires pure func title(self) String
  requires pure func lineCount(self) Int

  export pure func summary(self) String {
    return "${self.title()} (${self.lineCount()} lines)"
  }
}

export class Invoice implements Renderable with Summarised {
  export customer: CustomerId
  export currency: Currency
  lines: List[Line]            // module-private field
  status: InvoiceStatus

  // Associated constructor function: no `self`, called as Invoice.create(...).
  export pure try func create(customer: CustomerId, currency: Currency, lines: owned List[Line]) Invoice {
    if lines.isEmpty() {
      fail EmptyInvoice()
    }
    for line in lines {
      if line.unitPrice.currency != currency {
        fail CurrencyMismatch(expected: currency, found: line.unitPrice.currency)
      }
    }
    return Invoice(customer: customer, currency: currency, lines: move lines, status: InvoiceStatus.Draft)
  }

  export pure func title(self) String => "Invoice for customer ${self.customer}"
  export pure func lineCount(self) Int => self.lines.count()

  // Mutating method: the receiver is an exclusive loan.
  export pure func issue(editable self, now: Timestamp) Void {
    self.status = InvoiceStatus.Issued(at: now)
  }

  export pure try func total(self) Money {
    let mut sum = Money.zero(self.currency)
    for line in self.lines {
      let lineTotal = line.total() or fail
      sum = Money.add(sum, lineTotal) or fail
    }
    return move sum
  }

  export pure func render(self) String {
    let mut out = text.Builder.create()
    editable out.appendLine(self.summary())
    for line in self.lines {
      editable out.appendLine("${line.quantity} x ${line.description}")
    }
    let statusText = match self.status {
      Draft => "draft"
      Issued(at: issuedAt) => "issued ${issuedAt}"
      Paid(at: paidAt, reference: ref) => "paid ${paidAt} (${ref})"
      Voided(reason: why) => "void: ${why}"
    }
    editable out.appendLine(statusText)
    return out.finish()
  }
}

// A free function using a closure with an explicit capture list and kind.
export pure func countExpensive(invoice: Invoice, threshold: Money) Int {
  let isExpensive = pure fn[alias threshold](line: Line) Bool {
    return Money.greaterThan(line.unitPrice, threshold)
  }
  return invoice.lines.countWhere(isExpensive)
}

// Defer runs at every exit of the block, before locals are dropped; its body
// is checked at each exit, so `audit` may be used freely in between.
export recovers func withAudit(audit: editable List[String], invoice: Invoice) Money? {
  defer {
    editable audit.push("checked ${invoice.customer}")
  }
  let total: Money? = invoice.total() catch e {
    report e
    recover null
  }
  return move total
}

test "total sums lines" {
  let lines = List.of(
    Line(description: "Widget", quantity: 2, unitPrice: Money.of(Currency.GBP, 10d)),
    Line(description: "Gadget", quantity: 1, unitPrice: Money.of(Currency.GBP, 5d)),
  )
  let invoice = Invoice.create(customer: CustomerId(42), currency: Currency.GBP, lines: move lines) or fail
  expect (invoice.total() or fail) == Money.of(Currency.GBP, 25d)
}

test "quantity boundary" cases {
  "minimum": { quantity: 1, expected: 10d }
  "ten":     { quantity: 10, expected: 100d }
} as row {
  let line = Line(description: "x", quantity: row.quantity, unitPrice: Money.of(Currency.GBP, 10d))
  expect (line.total() or fail).amount == row.expected
}
```

An application in another package uses this library through an import and an entry point:

```fortis
// accept — src/app.ft in package acme.invoicing-cli; project.yaml has `entry: app`
import fortis.host { Host }
import acme.billing.invoices { Invoice, Line, CustomerId }
import fortis.money { Money, Currency }

export try func main(args: List[String]) Void {
  let caps = Host.capabilities()               // the only source of I/O authority (section 11.6)
  let lines = List.of(Line(description: "Consulting", quantity: 3, unitPrice: Money.of(Currency.GBP, 120d)))
  let mut invoice = Invoice.create(customer: CustomerId(7), currency: Currency.GBP, lines: move lines) or fail
  editable invoice.issue(Timestamp.epoch())
  caps.console.print(invoice.render())
}
```

The rest of this document explains every construct used above.

## 3. Lexical structure

### 3.1 Source text

Source files are UTF-8 with the extension `.ft`. Identifiers are ASCII in v1: a letter or underscore followed by letters, digits or underscores. Unicode is permitted in string literals, comments and data. A byte-order mark is rejected. Source text is an untrusted input with quotas (PROG-SEC-01): a file over 16 MiB or 2,000,000 tokens, or nesting brackets, blocks, prefix operators or string interpolations more than 256 deep, is `E-LIM-002` and is not parsed.

### 3.2 Lines, statements and continuation

A newline ends a statement unless one of the following holds:

- the newline is inside `(`…`)`, `[`…`]` or `{`…`}` that belongs to the same expression or argument list;
- the last token on the line is one of the **trailing continuation tokens**: `,` `(` `[` `=` `=>` `+` `-` `*` `/` `%` `<<` `>>` `&` `|` `^` `&&` `||` `??` `==` `!=` `<` `<=` `>` `>=` `..` `..<` `.`; the `<` that opens a refinement and the `>` that closes one are distinct tokens from the comparison operators (the lexer knows it is in a refinement, Appendix lexer note 2) and are **not** continuation tokens, so `type Score = Int<0..100>` ends its line normally;
- the first token on the next line is one of the **leading continuation tokens**: `.` `or` `catch` `??` `else` `)` `]` `with` `where` `implements` `derive` `from`.

**Parsing decisions.** The grammar is parsed with a bounded lookahead over token classes and no symbol table (LANG-03-03). Three decisions resolve the places where the grammar alone would allow more than one reading, and a generated parser must make exactly these (X-94): (P1) the lexer emits one `NL` for any run of newlines, blank lines and comment-only lines; (P2) an option or repetition whose body can start is taken, so operator and postfix chains are as long as possible and an expression-bodied lambda or match arm extends to the end of the enclosing expression; (P3) where alternatives overlap they are tried in grammar order and the first that parses wins, within the current statement: a statement that starts with a place is an assignment when an assignment operator follows the place and an expression otherwise, a `for` header is read as `move place`, then a range, then an expression, `let x = move p` is the plain `let`, a parenthesised type is tried before a tuple type, and `.name(` or `?.name(` is a method call.

Braces are of two kinds. A **body brace** opens a statement block (after a function head, `if`, `else`, `while`, `for`, `loop`, `defer`, `scope`, a match arm's `=>`, `catch e`, or an inner block statement) or a declaration body (`class`, `enum`, `interface`, `mixin`, `error`, `contract`, `aspect`, `observation`, `implement`, `match`, a `cases` table). Inside a body, items are separated by one or more newlines; newlines directly after the `{` and directly before the `}` are permitted and ignored, so a body may be empty, multi-line, or written on one line as `{ statement }`. An **expression brace** (record literal, record type) is an ordinary delimiter under the first rule above. The kind is decided by the token before the `{`: after `=`, `(`, `,`, `:`, `[`, `??`, `==`, `!=`, a compound assignment operator, `return`, `give`, `fail` or `recover` it is an expression brace; after anything else, including the name or parameter list of a declaration head, it is a body brace. The first rule applies to the innermost enclosing delimiter: a newline inside a body brace that is itself inside `(` or `[` still ends a statement, so a multi-line closure body in an argument list is written like any other body. A record type or record literal tolerates newlines directly after its `{` and directly before its `}` whichever kind the lexer assigned, and `,` already continues a line, so a record whose fields each sit on one line parses under either classification. The classification is visible only where a record's `{` is classified as a body brace, which happens for a record type directly after `)` (a return type) and a record literal directly after `=>` (an expression body): inside such a record a field broken across lines is a parse error, so it is written on one line or in parentheses.

There is no automatic semicolon insertion and no semicolon token. A `;` anywhere outside a string, a comment or a raw template region (section 16) is a parse error (`E-SYN-001`). A token sequence that is not a sentence of the grammar and matches no more specific syntax code is `E-SYN-000`. `return`, `fail` and `give` take their operand on the same logical line; a bare `return` on its own line returns `Void`.

### 3.3 Identifier categories by case

The first character of an identifier decides its grammatical category. This rule is lexical, needs no symbol table, and is what lets the parser distinguish generic application from indexing and refinement from comparison.

| First character | Category | Examples |
|---|---|---|
| Uppercase letter | type, interface, mixin, enum, enum variant, contract, error kind, observation schema | `Invoice`, `List`, `Draft`, `FastCalculation` |
| Lowercase letter or `_` | value, function, method, field, parameter, module alias, label | `invoice`, `total`, `text`, `outer` |

Module path segments are lowercase (`billing.invoices`). A package name is a dotted sequence of lowercase segments (`acme.billing`). Constants are ordinary lowercase bindings declared with `const`.

### 3.4 Keywords

Keywords are reserved: they cannot be used as identifiers in any declaration or binding position (`E-SYN-002`). The one relaxation is member position: after `.`, and as the name of a method or associated function, a keyword is an ordinary name (the grammar's `memberName`), which is what lets the library spell `String.from(view)` and `Customer.where(...)`. The lexer never reclassifies a keyword; the grammar admits it where `memberName` appears. The complete inventory, with the section that defines each word, is Appendix A. Kits add **declaration heads** (`application`, `model`, `component`, `context`, …); these are also reserved by the core grammar so that no module using any kit can use them as identifiers (section 16).

### 3.5 Literals

| Literal | Form | Type |
|---|---|---|
| Integer | `0`, `42`, `1_000_000`, `0xFF`, `0b1010`, `0o17` | from context; default `Int`. The literal's value must fit the target type and refinement, checked at compile time (`E-REF-001`, a violated refinement: the literal's interval is exact). |
| Float | `1.5`, `2e10`, `3.0f32` | `Float`; suffix `f32` gives `Float32`. A float literal needs a `.` or an exponent. |
| Decimal | `10d`, `19.99d` | `Decimal` |
| Boolean | `true`, `false` | `Bool` |
| String | `"text ${expr}"` | `StringView from static` for literals without interpolation (static storage); `String` (allocates) when interpolation is present: a fresh value that a call may borrow as a `StringView` for the statement, but that a binding annotated `StringView` cannot hold (`E-TYP-040`); every interpolated expression must be `Displayable` (`E-TYP-040` naming `Displayable` as the expected type) |
| Raw string | `"""…"""` | `StringView from static`; no interpolation, no escapes |
| Byte string | `b"abc"` | `BytesView from static` |
| Duration | `250ms`, `3s`, `2min`, `1h` | `Duration` |
| Null | `null` | the absent case of any `T?` |

Underscores may separate digit groups. A literal of a trivial type is a fresh value and never needs an ownership marker.

### 3.6 Strings and interpolation

`${expression}` inside a double-quoted string interpolates. The interpolated text is on one line and contains no comment. The expression is evaluated exactly once, left to right with the rest of the string, and must have a type implementing `Displayable`. An interpolated expression cannot be a fallible call or an `await`; write the call on its own line and interpolate the binding. Interpolation takes an implicit read loan of the places it names. A backslash escapes the character after it: `\n`, `\t`, `\r` and `\0` are the line feed, the tab, the carriage return and the NUL scalar (X-597, D-570), and any other escaped character stands for itself, so `\${` is the text `${` and opens no hole: a literal whose every `${` is escaped has no interpolation and is a `StringView from static` (3.5, 6.6). Escaping for HTML or other contexts is never part of string interpolation; templates in the Web UI kit apply contextual escaping separately.

### 3.7 Comments

There is one comment form: `//` to the end of the line. There are no block comments and no separate documentation-comment token. A run of `//` lines immediately above a declaration, with no blank line between the run and the declaration, is that declaration's documentation; the documentation tool extracts it. A fenced `fortis` code block inside such a run is compiled as an example and must carry a `// accept`, `// reject`, `// execute` or `// illustrative` marker on its first line (section 0). Those markers are recognised by the documentation tool and are never attached to a declaration as documentation, so a marker directly above a declaration does not become its description.

| Rule | Statement | Acceptance evidence |
|---|---|---|
| LANG-03-01 | A `;` outside strings, comments and raw template regions MUST be a parse error. | Negative lexer corpus. |
| LANG-03-02 | Newline termination and continuation MUST follow the token lists in 3.2 exactly. | Continuation corpus with every listed token. |
| LANG-03-03 | Identifier category MUST be decided by the first character as in 3.3. | Parser ambiguity corpus (no symbol table). |
| LANG-03-04 | Every keyword in Appendix A MUST be reserved in every declaration and binding position except the name of a method or associated function; member position after `.` and that name position MUST admit it as a name (`memberName`). | Keyword-as-identifier negative tests. |
| LANG-03-05 | Literal typing MUST follow 3.5; an out-of-range literal MUST be rejected at compile time. | Literal corpus. |
| LANG-03-06 | Interpolated expressions MUST be evaluated once, in order, and MUST NOT be fallible or suspending. | Side-effect order and negative tests. |

## 4. Modules and packages

### 4.1 Packages

A **package** is the unit of distribution, versioning and visibility. It is a directory with `project.yaml`, a lockfile and `src/`. The manifest names the package (`name: acme.billing`), its version, its dependencies, its public modules (`exports:`) and, for an application, its entry module (`entry:`). Packages are nominal: two different versions of the same package in one build are different packages, and their types never interchange.

```yaml
schema: fortis.project/v1
name: acme.billing
version: 1.4.0
exports: [invoices, money]
dependencies:
  fortis: "1.0"
```

The lockfile `fortis.lock` (written by `fortis lock`) pins what a build depends on (PROG-ARC-04). It names the root package's identity, the digests of the compiler, the standard library and the Rust toolchain, and the identity (`name@version`) and content digest of every package the manifests reach, so two versions of one package are two entries. When a package has a lockfile, resolution uses only it. A dependency a manifest names that the lockfile does not pin at that version, a pinned package no manifest reaches, or an unreadable lockfile is `E-NAM-020`. A package, compiler, standard library or Rust toolchain whose digest differs from the pinned one is `E-NAM-021`. Each record names the divergence. Nothing is fetched to resolve a lockfile. `fortis check` verifies the package pins and the compiler and standard-library digests, which it computes without running anything; a native build also verifies the Rust toolchain (X-133).

A dependency on a standard package (`fortis` or `fortis.…`) must name a package of the published compatibility set (`library/fortis/compatibility.yaml`) at the set's version, because kits are versioned separately and tested only together within a set (PLAT-01-01); a version outside the set, or a name that is not in it, is `E-NAM-031`. A build target's feature set decides which standard modules exist (Platform Specification 1.5): `fortis check --target T` and `fortis build --target T` reject an import of a module the target lacks with `E-NAM-030`, naming the module, the feature set it needs and the target's, before any native compilation (PLAT-01-03). A package that ships a Cargo build script is refused by a native build with `E-NAM-032` (PROG-CMP-06). A program that links two versions of one package checks with the versions' types kept apart and runs on every backend, each version's functions and types running where they are named; so does a program in which two packages, or two modules, declare the same name (X-136). A type's nominal identity is its name, its declaring package and version, and its declaring module: two modules' types of one name are different types, and a type written through a module alias (`one.Node`) is that module's type. A single-file program also has its own module identity. Identity is retained at every nesting depth, including type arguments, optional, tuple, record and function types, generic substitutions and newtype representations; the same spelling never makes a program type a prelude type. Assignment, argument and result checking use that identity (`E-TYP-040` for a mismatch), as do implementation coherence, conversion pairs, interface requirements and patterns. Built-in operations and backend representations belong to the built-in declaration, not to its spelling (X-640). A manifest over 1 MiB or 10,000 lines is `E-LIM-003`, a lockfile over 8 MiB or 100,000 lines `E-LIM-004`, and JSON the toolchain reads (caches, saved fuzz inputs) over 256 MiB or nested more than 128 deep `E-LIM-005` (PROG-SEC-01). A failure inside the toolchain itself is never a crash of the command: a checker defect is `E-INT-001`, whose `reproduction` names a directory holding the input minimised to what still fails and the command that reproduces it, a native build whose generated Rust does not compile is `E-INT-002`, at the Fortis function the code came from, and a native build that cannot run a tool it needs (cargo, rustc or the linker driver missing or failing before any generated code is compiled) is `E-INT-003`, naming the tool (PROG-CMP-05).

### 4.2 Modules

A **module** is one source file under `src/`. Its path is the file path relative to `src/`, with directory separators replaced by `.` and the extension removed: `src/invoices.ft` is `invoices` and `src/ledger/entries.ft` is `ledger.entries`. The import path seen by other packages is the package name followed by the module path: `acme.billing.invoices`, `acme.billing.ledger.entries`. A directory is not itself a module. A module has no header line; its identity comes from its path.

Modules of one package may import each other freely, including cyclically for type declarations. Value initialisation across modules is impossible because modules have no initialisation code: a module may contain only declarations and `const` bindings whose initialisers are compile-time constant expressions. Importing a module therefore never executes anything.

### 4.3 Imports

An `import` statement appears only at the top of a module, before any declaration.

```fortis
// accept
import acme.billing.invoices                 // package acme.billing, module invoices; binds the alias `invoices`
import acme.billing.invoices as inv          // binds the alias `inv`
import acme.billing.invoices { Invoice, Line } // binds the named members directly
import fortis.text                           // binds `text`
```

An import path is a package name followed by a module path. The split is decided by the manifest: the longest prefix of the path that names the current package or one of its declared dependencies is the package, and the remainder is the module path; if no prefix matches, the import is an error naming the undeclared package. A package may declare a **root module** in its manifest (`root: data`), which an import of the bare package name denotes (`import fortis.data { Db }`); without one, an empty remainder is an error. Package names and the module paths of other packages a project depends on must not be prefixes of one another in a way that makes the split ambiguous; the toolchain rejects a dependency set in which they are (`E-NAM-010`). The core package `fortis` has the modules `prelude`, `text`, `collections`, `numeric`, `time`, `money` and `host`, and every kit package is named `fortis.<kit>` with a kit name that is not one of those module names. Only modules listed in the exporting package's `exports:` can be imported from another package. Inside a package, any module can be imported by its siblings.

A plain import binds the final path segment as a module alias; members are accessed as `alias.member`. A braced import binds the listed members directly and nothing else. Importing the same name twice, or importing a name that collides with a local declaration, is an error. Imports are unused-checked: an unused import is a warning at the baseline lint level and an error at the strict lint level.

The standard package is `fortis`. Its module `fortis.prelude` is imported implicitly into every module and provides the core types (`Int`, `String`, `List`, `Map`, `Outcome`, the optional operators, …), the built-in interfaces of section 6.10 and nothing else. Every other standard module needs an import. The implicit prelude is a fallback: a module may declare a type or interface named `Bool`, `Int`, `String`, `Hashable` or any other prelude name, and an unqualified use then names that declaration. An explicit module alias (`import fortis.prelude`, then `prelude.Bool`) still names the prelude declaration. This differs from an explicit braced import, whose collision with a local declaration is rejected (X-640; D-592, D-591).

### 4.4 Visibility

There is one visibility modifier, `export`. A declaration without `export` is private to its module. An exported declaration is visible to every module that imports the module. Whether it is visible *outside the package* depends on the manifest: only modules listed in `exports:` are importable from other packages, so an `export` in an unlisted module reaches sibling modules only.

Fields, methods and associated functions of a class are private unless marked `export`. A use of a declaration or member that is not visible in the using scope is rejected at the use site (`E-NAM-004 not visible`). An exported signature may name only exported types: a non-exported class, enum, newtype or interface in an exported function, method, field or constant signature is rejected (`E-TYP-030`, the same code as for structural types in 6.9). An exported field is readable through any loan and writable through an `editable` loan. The generated constructor of a class (section 9.2) is available in a scope only if every field of the class is visible in that scope. Enum variants, interface members and mixin requirement declarations have no visibility of their own: they are visible wherever their enum, interface or mixin is. A mixin requirement does not export the class method that meets it (9.6, X-478); a method a mixin provides keeps its written visibility when contributed to the class.

### 4.5 Names and resolution

Each module has one namespace containing its declarations, imported aliases and imported members. A name resolves to the innermost enclosing block that declares it, then to the module namespace. Qualified names have the forms `module.member`, `Type.associated`, `Enum.Variant` and `value.field` or `value.method(...)`. There is no `::`. A type is not a value: a type name in an expression is a constructor call, the qualifier of an associated member or a variant, or a variant the expected enum type names (9.7); anything else is `E-TYP-040`. `Void` alone is also its one value. It has no constructor: `Void()` is `E-TYP-040`; use a block lambda `fn(n) { }` or a function returning nothing when a callback returns `Void` (X-675, D-420).

Shadowing a visible binding with a `let` is an error (`E-NAM-003`). Fields are always accessed through a receiver (`self.lines`, `invoice.lines`); a bare field name is not in scope inside a method.

### 4.6 Entry point

An application package names an entry module in `project.yaml` (`entry: app`). That module must declare a function named `main` with one of these heads, where `…` is the parameter list described below:

```fortis
// illustrative
func main(…)
try func main(…)
async func main(…)
async try func main(…)
```

`main` returns nothing: its result type is `Void`, written or left out (8.1, X-345). Each form may also carry `recovers`, which a `main` whose summary has that effect writes like any function (8.1, 11.2; X-155): it is transitive, so without it no program could recover a failure anywhere `main` reaches, and it changes nothing a caller relies on, since `main` has none. An `async` entry runs on the core cooperative executor as the root task; its completion ends the process. `export` may be written before any form and is implied when it is not: the entry module's `main` is the entry whether or not it is exported (X-345).

**Parameters, supplied by type.** The runtime supplies each parameter of the entry `main` by its type, so the parameters say what the program needs and nothing else (X-345). A parameter is `name: T` with no mode, default or `...`, in any order and under any name, where `T` is either `List[String]` (the prelude's), which receives the command-line arguments, or one of the ten capability types of 11.6, imported from `fortis.host`, which receives that capability. Each type appears at most once, and a `main` that needs nothing takes no parameters. A parameter of any other type is rejected (`E-NAM-005`), as is a second parameter of a type already taken (`E-NAM-006`). `main` owns its capability parameters, as it owns a value it created: it lends them to the functions it calls, and it may move one, for example into a `Shared` (`Shared.create(move console)`, 7.8). The command-line arguments are lent to `main`, as before.

```fortis
// accept
import fortis.host { Console, FileSystem, Path }
try func main(console: Console, files: FileSystem) Void {
  let path = Path.parse("greeting.txt") or fail
  files.writeAll(path, "hello from main") or fail
  console.print(files.readText(path) or fail)
}
```

The v0.0.1 form, `export func main(args: List[String]) Void` with `Host.capabilities()` called in the body, stays accepted: `args` is one parameter of that list, and a `main` that takes no capability parameter may still create its capabilities with one direct call of `Host.capabilities()` (11.6). A `main` that takes a capability parameter has had its capabilities created by the runtime, so it may not call `Host.capabilities()` (`E-EFF-010`). The formatter rewrites the v0.0.1 form to this one wherever the rewrite keeps the program's meaning: the capabilities the body reaches through `caps.console` and its siblings, or through a `Capabilities(...)` destructuring, become parameters named after their fields or bindings, `args` is dropped when nothing reads it, and the `export` is dropped.

The **entry `main`** is that function: the `main` of the manifest's entry module, or of a single-file program. It obtains I/O authority through its capability parameters, or by calling `Host.capabilities()` from `fortis.host` exactly once, directly in its own body (section 11.6; the capability types and `Host.capabilities()` are part of the core runtime inventory, section 15). Only the root program's entry `main` receives capabilities (X-271): a function named `main` anywhere else is an ordinary function, and every other function receives a capability as an ordinary parameter. Only the runtime calls the entry `main`, once: Fortis code never refers to it (`E-EFF-010`). If a `try main` fails, the runtime prints exactly one report of the failure to standard error and exits with status 1. That report, like every record written by `report`, is a structured failure record (code, payload, receipt) rendered as text on a terminal and as one JSON line otherwise (Delivery Programme 7.4). There is no `or die` operator.

| Rule | Statement | Acceptance evidence |
|---|---|---|
| LANG-04-01 | Module identity MUST be derived from the file path; modules MUST contain only declarations and constant bindings. | Module with statement at top level rejected. |
| LANG-04-02 | Importing a module MUST NOT execute any code. (old MOD-001) | Module initialisation tests. |
| LANG-04-03 | Only modules listed in the exporting package's `exports:` MAY be imported across packages. | Cross-package negative tests. |
| LANG-04-04 | Different versions of one package, and different modules, MUST have distinct nominal types. | Diamond-dependency corpus; same-name types of two modules. |
| LANG-04-05 | Shadowing a visible binding MUST be rejected. | Nested block corpus. |
| LANG-04-06 | The entry point MUST be the `main` of the manifest's entry module, exported or not, with a head from 4.6 (the four forms, each optionally `recovers`); each of its parameters MUST be `List[String]` or a capability type, each type at most once (`E-NAM-005`, `E-NAM-006`), and the runtime MUST supply each by its type. | Missing/wrong entry fixtures; a `recovers` entry; capability-parameter fixtures (unknown, duplicate, moved into `Shared`) executed on every backend. |

## 5. Statements and expressions

### 5.1 Blocks and bindings

A block is `{` statements `}`. A block introduces a scope. Leaving a block by any route (falling off the end, `return`, `fail`, `break`, `continue`, `give`) first ends every loan whose holding binding was declared in the block (section 7.3), then runs the block's registered `defer`s in reverse order of registration, then drops the block's owned locals in reverse order of initialisation.

`let name = expression` declares an immutable binding; `let mut name = expression` declares a mutable one. `let Pattern = move place` destructures a class, tuple or record (section 9.8) into new bindings, so its pattern matches every value of the place's type: a pattern that can fail to match, a variant, a literal, `null`, `present(…)`, `success(…)`, `failure(…)` or `is …`, at any depth, is `E-MAT-004`, and `match` handles the other cases (D-243). The binding is visible from the next statement to the end of the enclosing block. A `let` without an initialiser is not permitted. The type is inferred from the initialiser; `let name: Type = expression` states it explicitly and is required when the initialiser is a literal that should take a refined or wider type.

An inner `{ … }` block is itself a statement; its only purpose is to end loans and drop locals early (section 7.3).

`name = expression` is assignment. It is well-formed only when `name` is a visible `let mut` binding, or a field path whose root is a `let mut` binding, an `editable` parameter or an `editable` loan. Assigning to an immutable binding, a parameter or an unknown name is an error. Assignment is a statement, not an expression, and does not chain.

```fortis
// reject: E-NAM-001, E-NAM-002 (at the marked lines)
let a = 1
let mut b = a          // trivial copy, no marker
b = b + 1              // proof: a is 1, b is 2, no overflow
// reject: E-NAM-001 — `c` is not declared
c = 3
// reject: E-NAM-002 — `a` is immutable
a = 2
```

`const name = expression` declares a module-level constant whose initialiser is a **constant expression**: a literal, a `const`, arithmetic over those, an enum value, a record or variant-map literal of constant expressions, the name of a function or declaration, a call to one of the `of` constructors (`List.of`, `Routes.of` and the other `Type.of(…)` constructors the `fortis` package marks as constant-foldable) with constant arguments, or `Type.default()` of a type whose `Default` is derived. Constants are built once at load with no code of the program running; they are not captures and need no marker.

### 5.2 Conditionals

```fortis
// illustrative
if condition {
  …
} else if condition {
  …
} else {
  …
}
```

The condition is a `Bool` expression; nothing else is accepted (`E-TYP-010`). `if` is a statement. It is also an expression when it has an `else` and every branch block ends with `give value`; the branch types must unify to one type.

```fortis
// accept
let label = if count == 0 { give "none" } else { give "some" }
```

Refinement facts established by the condition apply inside the branch (section 6.3). The `else` branch sees the negation.

### 5.3 Loops

```fortis
// illustrative
while condition { … }
for item in iterable { … }
for i in start..<end { … }      // also start..end (inclusive)
loop { … }
```

`while` evaluates its `Bool` condition before each iteration. `for item in iterable` requires the iterable's type to implement `Iterable[T, I]` for some cursor `I`; `item` is bound once per iteration, immutably. Iterating a collection place by name (`for line in self.lines`) takes a read loan of the place for the duration of the loop and binds `item` as a read loan of each element; iterating a `move` of the place consumes it through `Consumable` and binds owned elements. `for i in a..<b` iterates integers; when `a` and `b` are constants or interval-tracked places, `i` has the refined type `Int<a..<b>` (section 6.3). The index binding is immutable and may not be assigned.

`loop` repeats until `break`. `break` and `continue` apply to the innermost loop unless given a label: `outer: for … { … break outer }`. Labels are lowercase identifiers followed by `:` before `while`, `for` or `loop`. `break` carries no value in v1.

`break`, `continue`, `return` and `fail` leave every block between the statement and the target; each block is exited in order, innermost first, with the exit sequence of 5.1.

A consuming loop with a linear element type transfers an obligation to its item binding on every iteration, including when the element type is a bounded `T: linear`. Every path to the end of the body or to `continue` must consume that item (`E-OBL-001`, naming the item); `_` cannot discard it. The hidden consuming cursor also owns the unvisited remainder. There is no operation in v0.1 that hands that remainder out of a `for` loop: an exit crossing the loop (`break`, `return`, or a `give` or `recover` targeting an enclosing expression) is `E-OBL-001`, even after consuming the current item. An unwind or cancellation point (`fail`, `or fail`, `await`, or `checkpoint`) is `E-OBL-005` while this remainder is live; the hidden cursor has no `Abandonable` exemption. A `continue` of this loop retains its remainder, but a labelled exit crossing an inner consuming loop must also respect that inner loop's remainder. These rules apply equally to standard collections, their forwarded representations and user `Consumable` implementations (X-620). A `ListView` only lends elements and is not `Consumable`, even when the view itself is moved. The calls to a user implementation's `consume` and `next` contribute their ordinary call effects (11.3). A user consuming cursor whose type is linear is rejected (`E-OBL-003`): `next()` returning `null` does not consume that cursor or prove that its owned fields are empty, and `Consumer` has no consuming terminal operation. Transfer a bounded class's contents into a standard collection and loop over that collection instead (X-622).

The `Iterable` and `Iterator` interfaces are:

```fortis
// accept
interface Iterable[T, I] where I implements Iterator[T] {
  pure func iterate(self) I from self               // I is a standard cursor: an owned value that stores a read loan of self
}
interface Iterator[T] {
  pure func next(editable self) (alias T from self)?   // a read loan of the next element, drawn from the cursor's stored loan
}
interface Consumable[T, I] where I implements Consumer[T] {
  func consume(owned self) I                        // an owned cursor that owns the collection
}
interface Consumer[T] {
  func next(editable self) T?                       // the next element, owned
}
```

A collection names its concrete cursor type as the second parameter (`List[T] implements Iterable[T, ListCursor[T]]`), which is how the result type is static without associated types. Cursors are standard library types (7.7): a user collection implements `Iterable` by returning the cursor of the standard collection it holds, and a `for` over such a class walks that cursor on every backend (D-545; `Deque` and `SortedSet` in 6.7 are two). `for item in p` desugars to `let mut cursor = alias p.iterate()` followed by a `loop` that calls `editable cursor.next()` until `null`. The cursor is an **owned loan-carrying value** (7.7): the binding owns it, so `editable cursor.next()` loans the cursor itself (the child-loan rule applies to loan-holding bindings, not to owners of loan-carrying values); the read loan of `p` it stores stays a read loan for the binding's lifetime; advancing the cursor writes the cursor's own storage and is therefore footprint-free in a `pure` function (11.3); and `item` is the read loan of the element that `next` returns, which lasts for that iteration. The lowering is `fn iterate<'a>(&'a self) -> ListCursor<'a, T>` and `fn next(&mut self) -> Option<&'a T>`, the element reference carrying the cursor's stored lifetime `'a`, not the exclusive borrow of the call, so the exclusive loan of the cursor ends when `next` returns and two consecutive `next` results may both be live (Appendix C). `for item in move p` uses `Consumable` instead and binds owned elements. An `Iterable` implementation alone does not permit consuming iteration, and a `Consumable` implementation alone does not permit reading iteration (`E-TYP-040`, D-589). The compiler may elide the cursor for standard collections; the observable order is the collection's documented order. The compiler may elide the iterator for standard collections; the observable order is the collection's documented order.

### 5.4 Function exits

- `return expression` leaves the enclosing function with a success value. A function whose success type is `Void` may use bare `return` or fall off the end of its body. A function with a non-`Void` success type must end every path in `return`, `fail` or a diverging call; there is no implicit value. A path that reaches the end of such a body is `E-TYP-110`; the same code reports a `match` arm block or `if`-expression branch in expression position that does not end in `give`.
- `fail failure` leaves the enclosing `try` function with a failure. It is an expression of type `Never`, so it may appear where any type is expected, including as a match arm. `fail` outside a `try` function is an error (`E-ERR-001`).
- `give expression` ends the enclosing `match` arm block or `if` expression branch with a value; a `catch` block ends with `recover`, never `give`.
- `recover expression` ends a `catch` block with a replacement success value (section 10.4).

`return` and `fail` always refer to the enclosing *function*, never to an enclosing closure body being written inline as an argument: inside a lambda they refer to the lambda. `give` and `recover` never cross a function or lambda boundary. A `give` outside a match arm or `if`-expression branch, a `recover` outside a `catch` block, and a `break` or `continue` outside a loop are `E-TYP-120`.

### 5.5 Expression statements and `discard`

An expression used as a statement must have type `Void`, `Never`, or be explicitly discarded: `discard expression`; an unused non-`Void` value without `discard` is `E-TYP-140`. Discarding an `Outcome`, a `Future`, a `Task` or any other linear value (section 6.12) is an error (`E-OBL-001`). `discard` exists so that an unused non-linear result is visibly intentional. The single exception is a never-polled future with a non-linear environment (12.3).

A function whose result most callers do not need says so once, on its declaration, with the core aspect `@ignorable` (11.8; X-345): a call of it used directly as a statement (`found.note(line, problem, detail)`, with or without a marker on its receiver) may drop a non-linear result without `discard`. Writing `discard` there stays accepted. Every other unused result still needs `discard`, and a linear result can never be dropped: `@ignorable` on a function whose result is linear (a `try` or `async` function, or a linear success type) is rejected (`E-TYP-150`), and a call whose instantiated result is linear is `E-OBL-001` as before.

```fortis
// accept
@ignorable
func remember(seen: editable Set[Int], n: Int) Bool {
  return editable seen.add(n)
}

func collect() Int {
  let mut seen: Set[Int] = Set.default()
  remember(editable seen, 3)
  discard remember(editable seen, 3)
  return 1
}
```

### 5.6 Operators and precedence

All binary operators are left-associative except where stated. Higher rows bind tighter.

| Level | Operators | Associativity | Notes |
|---|---|---|---|
| 1 | `a.b` `a.b(…)` `a[i]` `a(…)` `a?.b` `a?.b(…)` | left | postfix |
| 2 | `-a` `!a` `await a`; `copy p` `alias p` `editable p` `move p` | right | unary; the markers take a place (optionally followed by a call on it), never an expression, so `await editable db.insert(x)` is `await` of the marked call and `copy await f()` is a parse error |
| 3 | `a as T` | left | conversion (6.13, X-475); `a as Decimal * b` is `(a as Decimal) * b`, and `n as Int as Decimal` converts left to right; `T` is a type name, so a `<` directly after it opens a refinement (`n as Int<0..100>`): a converted value is compared in parentheses, `(a as Decimal) < b` |
| 4 | `*` `/` `%` | left | integer operands need proof (6.4) |
| 5 | `+` `-` | left | |
| 6 | `<<` `>>` | left | shift amount must be proved in range |
| 7 | `<` `<=` `>` `>=` | none | comparisons do not chain |
| 8 | `==` `!=` | none | |
| 9 | `&` | left | bitwise and |
| 10 | `^` | left | bitwise xor |
| 11 | `\|` | left | bitwise or |
| 12 | `&&` | left | short-circuit |
| 13 | `\|\|` | left | short-circuit |
| 14 | `a ?? b` | right | optional coalescing; `b` evaluated only when `a` is `null` |
| 15 | `a or fail`, `a or recover b`, `a catch e { … }` | right | outcome handling; `b` and the catch body are evaluated only on failure; an `Int` arithmetic `a` is a checked region (6.4) |
| 16 | `a..b` `a..<b` | none | ranges; in `for` headers, refinement types and range patterns (9.8) |

Outcome operators (level 15), `??` (level 14) and the Boolean short-circuit operators (levels 12–13) may not be mixed in one expression without parentheses: `a || b ?? c` and `a ?? b || c` parse and are rejected on the parse tree (`E-SYN-004`); `f() or fail ?? 0` is not a sentence of the grammar at all, because `or fail` ends the expression, and is the generic parse error (`E-SYN-000`). `await f() or fail` parses as `(await f()) or fail`. `copy await f()` is a parse error: markers take a place, and `await f()` is not a place; write `let v = await f()` first. Parentheses group an expression and change nothing else it means, so a marked place keeps its marker through them: `f((move d))` and `f(((move d)))` are, to the checkers and to every backend, the very call `f(move d)` is, and each moves `d` (D-447). The call a marker may be followed by is a call on the place: a method call, or a call of a closure the place holds. A function's name is not a place, so a marker written on a call of a function is refused (`E-OWN-001`): the call is written plain and the markers sit on its arguments, `first(alias xs)` rather than `alias first(alias xs)`, and `make(console, "m")` rather than `move make(console, "m")`, a fresh result needing no marker at all (7.5, 7.6; D-382).

### 5.7 Evaluation order

Operands and arguments are evaluated left to right, exactly once. The receiver of a method call is evaluated before the arguments. Short-circuit operators evaluate their right operand only when needed. `move x || move x` is rejected because the second `move` uses a moved place on a reachable path (section 7).

A call `f(a1, …, an)` executes in four steps:

1. **Evaluate.** Receiver and arguments are evaluated left to right. An argument that is a marked place (`editable p`, `move p`, `copy p`) or an unmarked ownership-bearing place **reserves** a loan or move of that place but does not activate it. An argument that is an expression (a nested call, arithmetic, a literal) is evaluated to a value here; every loan taken *during* that evaluation (for example the receiver loan of `buffer.length()`) ends when the value is produced, unless the value itself **carries** a loan, in which case the carried loan is reserved like a marked place. Three kinds of value carry loans: a `from` result, a view, and a future (an `async` call's result carries every loan its arguments took, 7.3). A loan-carrying value keeps its loans wherever it is moved, into a binding, a `launch`, or another call, and a synchronous call's release step never ends a loan carried by one of its arguments. A reservation is broken by any later argument that moves, assigns or exclusively loans the reserved place or a prefix of it; this is an error at the breaking argument (`E-OWN-012`).
2. **Activate.** All reservations become active at once, after the last argument has been evaluated. Conflicts among them, or with loans already active from enclosing statements, are reported here.
3. **Invoke.** The callee runs.
4. **Release.** Loans activated for the call end **when the call returns**, unless the result carries them: a `from` result (7.6), a view, or the future of an `async` callee (7.3), in which case the loan is held by the value and by the binding that receives it. Because release happens before the enclosing statement finishes, `sum = Money.add(sum, lineTotal) or fail` is legal: the read loan of `sum` taken for the call has ended by the time the assignment drops the old value and stores the new one (7.9).

```fortis
// reject: E-OWN-012 (at the marked lines) — the read loan for buffer.length() ends in step 1; the editable loan activates in step 2
update(editable buffer, buffer.length())
// reject: E-OWN-012 — the second argument moves a place reserved by the first
update(editable buffer, consume(move buffer))
```

Temporaries created in step 1 are dropped at the end of the full statement, in reverse order of creation.

### 5.8 Calls, named arguments and defaults

Arguments may be positional or named (`name: value`). Positional arguments come first and bind in declaration order; named arguments follow in any order. Each parameter is bound once. A **pun** writes a named argument whose value is the local of the label's name once: `wanted` stands for `wanted: wanted` (X-345). A bare local name is a pun where a positional argument cannot stand: after a named argument or another pun, and anywhere in a class's generated constructor, whose arguments are all named (9.2). Before the first named argument of any other call a bare name is positional, as before, and binds the parameter in its position. A pun is matched by its label as the written form is, so a name that is no parameter or field, or one bound twice, is `E-TYP-130`; it passes the place unmarked, so where the parameter or field takes ownership of an ownership-bearing local the marker is written and the argument is not a pun (`name: move name`). The formatter writes the pun wherever it means the same. A positional argument after a named one, a name that is not a parameter, a parameter bound twice and an argument beyond the parameter list are `E-TYP-130`; a parameter without a default that no argument binds is `E-TYP-050`, the code that also reports a constructor call missing a field. Parameter names are part of a function's public signature: renaming an exported parameter is a breaking change. A parameter may declare a default: `func page(size: Int<1..100> = 20)`. A default is a pure, non-allocating constant expression re-evaluated at each call that omits the argument. User functions have no variadic parameters; a function that takes many values takes a `ListView[T]` and the caller passes `List.of(…)` or an existing collection. The `Type.of(…)` constructors in the `fortis` package are the one variadic form, implemented as compiler intrinsics that accept any number of arguments of the element type.

| Rule | Statement | Acceptance evidence |
|---|---|---|
| LANG-05-01 | `let` MUST declare, and a `let` pattern MUST match every value of its type (`E-MAT-004`); `=` MUST only assign to a visible mutable target. | Declaration/assignment corpus; refutable-`let` corpus. |
| LANG-05-02 | Every block exit MUST end the block's loans, run defers LIFO, then drop locals in reverse initialisation order. | Exit-order execute tests for each exit kind. |
| LANG-05-03 | `if`, `while` conditions MUST be `Bool`. | Negative corpus. |
| LANG-05-04 | A ranged `for` MUST refine its index as in 5.3; the index MUST be immutable. | Refinement execute tests. |
| LANG-05-05 | Non-`Void` functions MUST end every path in `return`, `fail` or `Never`. (old SYN-001 part) | Missing-return corpus. |
| LANG-05-06 | Precedence and associativity MUST follow 5.6; mixing the listed operator families without parentheses MUST be a parse error. | Precedence corpus. |
| LANG-05-07 | Evaluation MUST follow 5.7 and be identical on both backends. (old SYN-004) | Side-effect order differential tests. |
| LANG-05-08 | Discarding a linear value MUST be rejected; discarding any other non-`Void` value MUST require `discard`, except the result of a call statement of a function declared `@ignorable` (5.5). | Discard corpus; `@ignorable` fixtures. |
| LANG-05-09 | Named arguments MUST follow positionals, bind each parameter once, and be part of the exported signature; a pun MUST bind the parameter or field of its name, exactly where 5.8 allows one (X-345). | Signature-diff tests; pun corpus. |

## 6. Types

### 6.1 Overview

Fortis is nominally typed. A type is one of: a built-in type; a `type` newtype; a class; an enum; an interface used as a `dynamic` type; a function type; a tuple or anonymous record; a refinement of a built-in; a generic instantiation of any of these; a loan or view type. There is no top type, no `Any`, no implicit conversion between distinct types and no runtime type mutation. A value used where a type it is not assignable to is expected (an argument, an initialiser, an assignment, a return, an operand) is rejected with `E-TYP-040`, naming both types, unless a more specific code applies (a non-`Bool` condition is `E-TYP-010`, an unproved refinement `E-REF-001`); sections 3.5, 9.4 and 10.1 give instances (X-152).

### 6.2 Built-in types

| Group | Types |
|---|---|
| Unit | `Void` (one value), `Never` (no value) |
| Boolean | `Bool` |
| Integers | `Int8` `Int16` `Int32` `Int64` `UInt8` `UInt16` `UInt32` `UInt64`; `Int` = `Int64`; `UInt` = `UInt64`; `Byte` = `UInt8` |
| Floating | `Float32` (binary32), `Float` (binary64) |
| Exact | `Decimal` (sign, 96-bit coefficient, scale 0..28), `BigInt` (heap, allocates) |
| Text | `String` (owned UTF-8), `StringView` (borrowed UTF-8 view), `Bytes` (owned), `BytesView` |
| Time | `Duration`, `Instant` (monotonic), `Timestamp` (wall clock, UTC) |
| Collections | `List[T]`, `Array[T, N]`, `ListView[T]`, `Map[K, V]`, `OrderedMap[K, V]`, `Set[T]`, `FixedList[T, N]`, `FixedMap[K, V, N]` |
| Compound | tuples `(A, B)`, anonymous records `{ name: A, other: B }`, variant maps `VariantMap[E, V]` written `{ Found: 200, NotFound: 404 }` (one entry per variant of `E`, checked for completeness) |
| Optional | `T?` |
| Outcome | `Outcome[T]`, `Failure`, `Report` |
| Concurrency | `Future[T]`, `Task[T]`, `TaskCompletion[T]`, `Shared[T]`, `Weak[T]`, `Mutex[T]`, `AtomicInt`, `AtomicBool` |
| Callable | `fn(Params) Ret`, `fn mut(Params) Ret`, `fn once(Params) Ret`, with modifiers (section 8.4) |

`N` in `Array[T, N]` and `FixedList[T, N]` is an integer literal. An array constructor uses the size in its resolved type, whether written or supplied by the expected type; a refinement inside its element type does not supply a size. Decimal, hexadecimal, binary, octal and underscore spellings have their ordinary integer meanings here (3.5; X-731). Functions generic over `N` are not supported in v1.

`List.of(a, b, c)` builds a list from zero or more arguments. It is declared with a **variadic last parameter**, `of(...items: T)`, a form allowed only in `trusted` bodiless signatures of the standard package: there are no user-defined variadic functions in v1, and a variadic parameter takes positional arguments only.

### 6.3 Refinements

A refinement restricts a built-in integer, text, byte or collection type by a predicate written in `<…>` after the type name. Refinement syntax is accepted only in type position.

| Form | Meaning |
|---|---|
| `Int<0..100>` | inclusive range |
| `Int<0..<100>` | exclusive upper bound |
| `Int<1..>` / `Int<..0>` | one-sided |
| `String<scalars: 1..120>` | Unicode scalar count |
| `Bytes<length: 0..65536>`, `List[T]<length: 0..100>` | element count |

A refinement is a **structural subtype** of its base and of any refinement whose range contains it: `Int<0..50>` is assignable to `Int<0..100>` and to `Int` without any operation. The reverse direction never happens implicitly. A refinement inside a type argument is proved like the type itself, and only there: a `List[Int]` where a `List[Int<0..5>]` is expected is `E-REF-001` (unproved) unless every element is proved, which for a value no literal built it never is, so the author validates element by element; and the type arguments of a collection, a view or any other container are invariant in their refinement, so a `List[Int<0..5>]` is not a `List[Int]` (`E-TYP-040`), whether lent, moved or copied, since through an alias with the wider element type a 9 could be stored (X-514; D-465). An optional follows its value's subtyping: `Int<0..3>?` is assignable to `Int?`, just as a present value may be injected, because an optional is a value with no way to write through it (X-685, the owner, 30 October 2026: "Yes, optionals widen"). Tuple elements and anonymous record fields follow their own assignability rules (6.9, X-686). These value rules do not permit widening inside a container type argument: `List[Int<0..3>?]` is still not `List[Int?]`. A refinement is well formed only in the forms the table lists: a range over an integer type; `scalars` over `String` or `StringView`; `length` over `Bytes`, `BytesView`, `List[T]` or `ListView[T]`. Each bound is an integer literal or a `const` of the base type resolved at compile time (X-512): a constant whose initialiser is an integer literal, another such constant, or `-`, parentheses and `+ - * / %` over them (products first, division as 6.4 divides), named plainly or through a module alias (`Int<0..Top>`, `Int<low..top * 2>`); an unannotated constant has its literal's type, `Int`. The range is not empty (`Int<5..1>`, `Int<0..<0>`), and it lies within the base's own range (`UInt8<0..300>` does not) or, for a measure, within `0..`. Anything else is rejected with `E-TYP-020`, read as "not a refinement of this base": a float base (`Float<0..1>`, not supported in v1), a base the table does not name (`Bool<0..1>`, `Map[K, V]<length: 0..3>`, a newtype, a type parameter), a measure the base lacks (`Int<scalars: 1..2>`, `String<length: 0..3>`), a bound not resolved at compile time (an undeclared name, a `const` of another type or of a non-integer value, a divisor of zero), an empty range, or a range outside the base (X-510).

**Facts and the interval domain.** The checker tracks, for every integer-typed place in a function, an interval `[lo, hi]` within the bounds of the base type. The rules are:

- A literal has the exact interval of its value. A parameter or field has the interval of its declared type. A call result has the interval of the callee's declared return type, except that the success value of a checked integer intrinsic (`Int.add(a, b)` and the other members of the checked family, 6.4) has the interval of the operation on its operands clipped to the type's range, because the call fails instead of overflowing (X-93); so has a checked operator (6.4, X-345).
- `+`, `-`, `*` combine intervals by interval arithmetic; `/` and `%` require the divisor's interval to exclude zero and produce the interval of the quotient or remainder; `<<` requires the amount's interval to lie in `[0, width-1]`.
- An `if`, `while` or `match` condition of the forms `p < c`, `p <= c`, `p > c`, `p >= c`, `p == c`, `p != c` where `p` is a tracked place and `c` is a literal, a `const` or another tracked place narrows `p` (and `c` when it is a place) in the true branch; `&&` narrows by both conjuncts; parentheses around either form preserve that narrowing (X-687); the `else` branch receives the negation of a single comparison, and so does the rest of the enclosing block when the true branch ends in `return`, `fail`, `break`, `continue` or a `Never` call (the same diverging-branch rule as null tests, 6.8), which is what lets a guard clause like `if value < low { return low }` narrow `value` afterwards. `||` and function calls do not narrow.
- At a join of control-flow paths, the interval is the hull of the incoming intervals. The result of an if-expression or `match` with integer branches of one base has that hull too, including literal branches; it does not choose the first branch's narrower interval (X-688, 6.10).
- At a loop header, every tracked place assigned anywhere in the loop body is widened to its declared type's full range, except the index of a ranged `for`, which has the interval of the range. Loop-carried facts therefore come from the declared type of the `let mut` binding, so a counter should be declared with a refined type: `let mut i: Int<0..1000> = 0`.
- **Invalidation.** Every fact about a place is discarded when the place, a prefix of it, or a field of it is passed as an `editable` argument or receiver, exclusively loaned, or reachable from an `editable` capture of a closure that is called; the place then has the interval of its declared type. Assignment to the place itself discards the old facts and establishes the interval of the assigned expression. Inside a loop, a place invalidated anywhere in the body is widened at the header like an assigned place.
- **Length facts.** In the true branch of `i < p.count()`, where `i` is a tracked integer place and `p` a collection place, `i` is known to be below `p`'s length; the index of a ranged `for i in a..<p.count()` is known to be below `p`'s length throughout the body when the body neither assigns nor exclusively loans `p` (X-93); `i <= p.count()` gives only `i ≤ length`, which does not permit indexing; `i >= p.count()` gives the bound in its *else* branch, while the *else* branch of `i > p.count()` gives only `i ≤ length` (X-260). These facts say nothing about the lower bound, which must come from `i`'s type or a separate `i >= 0` test, and are invalidated by the invalidation events on either place. With a lower bound of at least 0 and the strict upper bound, `list[i]` is accepted.
- **Length refinements** (`List[T]<length: 0..100>`) are properties of a value. Every reading operation of the base remains available, including indexing, methods and iteration; its type arguments are unchanged. Refining a view preserves its loan and its origin requirements (7.6, 7.7). A static index must be below the minimum length guaranteed by the refinement, or proved below the actual length by a length fact. Its maximum length alone proves no element exists. A place of such a type may not be loaned `editable` or mutated, including through an index (`E-REF-003`); mutate an unrefined collection and `validate` the result (X-691).
- **Remainder.** For `a % d` with `d`'s interval excluding zero and `m` the largest `|d|`: if `a`'s interval is within `[0, ∞)` the result is `[0, m-1]`; if within `(-∞, 0]`, `[-(m-1), 0]`; otherwise `[-(m-1), m-1]`.
- Assigning a value whose interval lies outside the target's declared refinement is an error (`E-REF-001 unproved refinement`), with the fix being an explicit `validate` or a wider declared type.

There is no solver beyond these rules. A value the rules cannot prove is converted: `n as Int<0..100>` is the pure fallible conversion of a base type into a refinement of it, of type `Outcome[Int<0..100>]`, failing with `refinement.out_of_range` (6.13, X-475); `UserId.validate(x)` for a nominal refined type, and `Int<0..100>.validate(x)`, are the same conversion spelled as a call, and they are the explicit fallback whenever the interval rules cannot prove a bound. The call spelling `T.validate(x)` is retired but remains accepted in v0.1; `fortis fmt` rewrites it to `x as T`, preserving the handling of its outcome (X-552).

```fortis
// accept
pure func clamp(value: Int, low: Int<0..100>, high: Int<0..100>) Int<0..100> {
  if value < low {
    return low
  }
  if value > high {
    return high
  }
  return value                          // interval of value here: [low, high] ⊆ [0, 100]
}
```

```fortis
// reject: E-REF-001 (at the marked line)
pure func sumTo(n: Int<0..1000>) Int<0..500500> {
  let mut total: Int<0..500500> = 0
  for i in 0..n {                       // i : Int<0..1000>
    total = total + i                   // reject: E-REF-001 — interval of total+i is [0, 501500]
  }
  return total
}
```

The second example shows the limit of the domain: the loop widens `total` to its declared range and the addition then exceeds it. The accepted repair accumulates into an unrefined `Int` with a checked operation and validates once at the end:

```fortis
// accept
pure try func sumTo(n: Int<0..1000>) Int<0..500500> {
  let mut total: Int = 0
  for i in 0..n {
    total = Int.add(total, i) or fail       // checked: no proof needed, fails on overflow
  }
  return Int<0..500500>.validate(total) or fail
}
```

### 6.4 Integer arithmetic

An unsuffixed integer literal takes its expected integer type through arithmetic operands and through the
value supplied by `recover` or `give`, including nested branches (X-591). This selects the literal's type; it
introduces no implicit conversion of an existing typed value.

Integer operators `+ - * / % << >>` and unary `-` are **proof-required**: the operation is accepted only when the interval rules of 6.3 show that overflow, division by zero, out-of-range shift and `MIN / -1` cannot occur. Otherwise the operation is rejected with `E-REF-002` naming the operand intervals and the checked alternative, or, where there is none, asking for a refined type or a test that proves it. The result type of a proof-required operation is the base type refined to the computed interval.

`fortis.prelude` declares three families of named operations on `Int`:

| Family | Example | Result | Declared on |
|---|---|---|---|
| Checked | `Int.add(a, b)`, `Int.sub`, `Int.mul`, `Int.div`, `Int.rem`, `Int.shiftLeft` | `Outcome[Int]`; fails with `numeric.overflow`, `numeric.divide_by_zero` or `numeric.shift_range` | `Int` only, in this release |
| Wrapping | `Int.wrappingAdd(a, b)`, … | total, two's complement wrap | `Int` only, in this release |
| Saturating | `Int.saturatingAdd(a, b)`, … | total, clamps to the type bounds | `Int` only, in this release |

The sized integer types (`Int8`, `Int16`, `Int32`, `UInt8`, `UInt16`, `UInt32`, `UInt64`) have none of the three families yet; theirs are planned with the complete standard library (v0.4, section 15). Until then an unproved operator on a sized type is made safe by a refined type or a test (X-345).

Division truncates toward zero; `%` takes the sign of the dividend; `>>` on signed types is arithmetic, on unsigned types logical. Compound assignment `a += b` is sugar for `a = a + b` and follows the same proof rule. Integer behaviour never depends on the build mode.

**Checked operators** (X-345). The left operand of an outcome operator (`or fail`, `or recover`, `catch`; 10.2) that is integer arithmetic is a **checked region**: the operators `+ - * / % << >>` and unary `-` reachable from it through those operators and parentheses, and not through a call's arguments, an index, a field access, a lambda or any other operator. In a checked region an operator on `Int` that the interval rules cannot prove safe is not rejected: it is the **checked operation**, `Int.add`, `Int.sub`, `Int.mul`, `Int.div`, `Int.rem` or `Int.shiftLeft` (unary `-x` is `Int.sub(0, x)`), and fails exactly as that function fails, with `numeric.overflow`, `numeric.divide_by_zero` or `numeric.shift_range` and the same payload, on every backend. Its success value has the interval of the operation clipped to `Int`'s range, as the function's does (6.3); an operator the rules prove safe stays a plain value. A region with a checked operation is an `Outcome[Int]`, and the outcome operator handles it by the rules of 10.2, as it handles a call: `or fail` needs a `try` function (`E-ERR-001`), and `or recover` or a `catch` that recovers adds `recovers`. Operands are evaluated left to right as everywhere (5.7), and the first checked operation that fails ends the region with its failure: `a * b + c or fail` fails at the multiplication and does not add. One handler covers the whole arithmetic because the outcome operators bind loosest (5.6), so `a * b + c or fail` is `(a * b + c) or fail`. In a `test` or `property` body, integer arithmetic outside such an operand is a checked region too, with `or fail` implied (14).

A region whose operators are all proved safe cannot fail, and handling it is `E-TYP-040`, as handling any value that is not an outcome is. An unproved operator outside a region stays `E-REF-002`, and its record names the checked form: in a binding (`let s = a + b`; write `let s = a + b or fail`), under a comparison (`a + b == c or fail`, whose region would be a `Bool`; write `(a + b or fail) == c`), in a call's argument (`f(a + b) or fail` handles the call; write `f(a + b or fail) or fail`), in a compound assignment (`x += 1`; write `x = x + 1 or fail`), and on the integer types other than `Int`, which have no checked family. An operand that is itself an outcome is not taken into a region (`a + f() or fail` is `E-TYP-040`; handle `f()` first), and `>>` has no checked operation, so an unproved `>>` is `E-REF-002` in a region too. The named functions stay: `Int.add(a, b) or fail` means what `a + b or fail` means where the operator is unproved, and the formatter writes the operator form where the two are equivalent.

### 6.5 Floating point and Decimal

`Float` and `Float32` operators are total IEEE 754 binary operations with round-to-nearest-even; there is no proof requirement and no overflow failure. The implementation must not reassociate, contract into fused multiply-add or otherwise alter results (strict IEEE is the only mode). `+ - * /` and `Float.sqrt` are bit-exact across backends and targets, with every NaN result canonicalised to the positive quiet NaN with zero payload before it is stored or compared, so that parity is defined on canonical bits. Transcendental functions live in `fortis.numeric.math`, document a tolerance in ULPs and are excluded from bit-exact parity claims.

`Float` and `Float32` do not implement `Equatable`, `Hashable` or `Comparable`, because NaN breaks their laws. `==` on floats is IEEE equality (NaN ≠ NaN) and is allowed; to use floats as map keys or sort them, convert with `Float.totalOrder(x)` (an `Int64` with the IEEE total-order mapping) or `Float.bitsEqual(a, b)`.

`Decimal` is a sign, a 96-bit unsigned coefficient and a scale 0..28, matching `rust_decimal` and .NET `decimal`. It has no NaN or infinity. There are no arithmetic operators on `Decimal` in v1 (the interval domain is integer-only): arithmetic uses the checked functions `Decimal.add`, `sub`, `mul`, `div`, `rem`, which return `Outcome[Decimal]` and fail with `numeric.overflow` or `numeric.divide_by_zero`. Result scale: `add`/`sub` use the larger operand scale; `mul` uses the sum of the scales; `div` uses the largest scale that fits; when a result needs more than 28 fractional digits or more than 96 coefficient bits it is rounded half to even and, if the integer part still does not fit, fails. `Decimal.round(value, scale, mode)` is explicit, with `mode` a `RoundingMode` (`HalfEven`, `HalfAwayFromZero`, `TowardZero`, `AwayFromZero`); `Decimal.rem` takes the larger operand scale and the sign of the dividend. Converting from `Float` requires `Decimal.fromFloat(x, scale, mode)` and is fallible: rounding is a choice the call writes, so a `Float` has no `as` conversion into an integer type or a `Decimal` (6.13, X-475). A `Decimal` displays with exactly its scale (`1.50d` as `1.50`); `Float.isNaN(x)` and `Float.sqrt(x)` (NaN for a negative argument) are in the prelude. A `Float` displays as the shortest digit string that reads back to the same value, in plain notation with a fractional part (`2e3` as `2000.0`, `0.1 + 0.2` as `0.30000000000000004`), and the non-finite values as `NaN`, `Infinity` and `-Infinity`; `Float32` displays likewise for binary32.

### 6.6 Text and views

`String` is an owned, growable, valid UTF-8 value with value identity: copying it with `copy` produces an independent string. `StringView` is a borrowed, immutable, valid UTF-8 slice with a `from` owner. `Bytes` and `BytesView` are the byte equivalents; both have `byteCount()` and `get(index) Byte?`, which reads one byte in place and is `null` outside the buffer, so a `noalloc` parser can walk a `BytesView`. A string literal is a `StringView from static` and may be used wherever a `StringView` is expected. Where a `String` is expected (a `String` field, parameter, `let x: String`, or a `match`/`if` expression whose other arms are `String`), a literal converts implicitly by allocating; this is the one implicit conversion in the language, it adds `alloc` to the summary, and it is therefore rejected inside `noalloc` code. `view as String` makes the allocation explicit (6.13, X-475); `String.from(view)` is retired, remains accepted in v0.1, and is rewritten by `fortis fmt` to `view as String` (X-552). `copy` applies to places only, never to a literal.

Indexing a string by integer is not provided. Byte length (`view.byteCount()`, O(1)), scalar count (`view.scalarCount()`, O(n) in bytes) and grapheme count (`fortis.text.graphemes`, O(n)) are separate operations; slicing and `scalarAt` are O(n) in bytes because UTF-8 scalars have variable width, and the `text.Builder` operations are amortised O(length appended) with `finish()` O(total length). Slicing is by scalar offsets through `view.slice(from, to)` returning `Outcome[StringView from view]`, failing on invalid boundaries; `view.scalarAt(i)` returns `Outcome[Scalar]`, failing past the end. `Scalar` (in `fortis.text`) is a trivial Unicode scalar value with `Scalar.of(text)` (the single scalar of a one-scalar view, a constant expression for a literal), `Scalar.digitValue(s)` and `Scalar.hexValue(s)` returning `Int<0..9>?` and `Int<0..15>?`. `text.Builder` accumulates text: `Builder.create()`, `append(view)`, `appendLine(view)`, `appendScalar(s)`, `finish() String`. Normalisation and Unicode case mapping would live in `fortis.text` and allocate; v0.1 has neither, only the ASCII case mapping below (X-596).

**Text essentials (X-479, X-595).** `String` and `StringView` both have the methods below, so a program trims and splits an owned string or a view alike; none of them can fail, and none indexes by integer. "Whitespace" is the Unicode `White_Space` property: U+0009 to U+000D, U+0020, U+0085, U+00A0, U+1680, U+2000 to U+200A, U+2028, U+2029, U+202F, U+205F and U+3000 (so U+001C to U+001F and U+200B are not whitespace). Positions are scalar offsets, the unit `slice` and `scalarAt` take. `n` is the text's length in bytes and `m` the argument's.

| Method | Result | Cost and effect |
|---|---|---|
| `trim()`, `trimStart()`, `trimEnd()` | `StringView from self`: the text without its leading and trailing, leading only, or trailing only whitespace | O(n); a view, no allocation |
| `find(needle: StringView)` | `Int<0..>?`: the scalar offset of the first occurrence, `null` if there is none; the empty needle is found at 0 | O(n·m) at most; no allocation |
| `contains(needle: StringView)` | `Bool`: `find(needle) != null` | as `find` |
| `startsWith(prefix: StringView)`, `endsWith(suffix: StringView)` | `Bool`; the empty prefix or suffix always matches | O(m); no allocation |
| `split(separator: StringView)` | `List[String]`: the pieces between the non-overlapping occurrences of the separator, left to right | O(n); allocates |
| `lines()` | `List[String]`: the lines of the text | O(n); allocates |
| `replace(old: StringView, new: StringView)` | `String`: a copy with every non-overlapping occurrence of `old`, left to right, replaced by `new` | O(n) plus the cost of `find`; allocates |
| `asciiUpper()`, `asciiLower()` | `String`: a copy with the ASCII letters mapped, every other scalar unchanged | O(n); allocates |
| `repeat(times: Int<0..>)` | `String`: the text written `times` times in a row | O(times·n); allocates |
| `String.join(parts: ListView[String], separator: StringView)` | `String`: the parts in order with the separator between neighbours | O(total length); allocates |

`split` keeps every piece, so a separator at either end, or two in a row, gives an empty piece there; a text without the separator is one piece (itself), the empty text is one empty piece, and an empty separator occurs nowhere, so the text is again its own single piece. `replace` with an empty `old` returns an unchanged copy for the same reason, while `find("")` is 0 and `contains("")`, `startsWith("")` and `endsWith("")` are true. A line ends at a line feed, and a carriage return directly before that line feed ends with it (`\r\n`); the text after the last terminator is a line only when it is not empty, so `"".lines()` is empty, `"a\n".lines()` is one line and `"\n".lines()` is one empty line, and a carriage return not before a line feed stays in its line. `String.join` of no parts is the empty string and of one part is that part. A text too large to allocate is an out-of-memory defect (10.8), as for any allocation. Parsing is not a text method: `text as Int or fail` is the conversion of 6.13.

`trim`, `trimStart` and `trimEnd` return views, so the result is a loan of its text and follows the rules of 7.6 and 7.7: it cannot outlive a local owner (`E-OWN-051`) and the owner cannot be moved while it is held (`E-OWN-011`). `split` and `lines` return owned strings and not views, because a `List` of views would store loans, which Language 1.0 does not have (7.7); each piece is an allocation, which the `alloc` effect of the method says (the effect data of `fortis.prelude` lists `split`, `lines`, `replace`, `asciiUpper`, `asciiLower`, `repeat` and `join`), and a call is rejected in `noalloc` code. A program that wants no allocation walks the text with `find` and `slice`. `find` answers a scalar offset, not a view or a pair, because the offset is the one position the model already has; it can be passed to `slice`, and it is `Int<0..>?`, so a missing needle is `null` and never a sentinel. Case mapping is ASCII only (X-596): full Unicode mapping depends on the Unicode version and the language, lengthens some texts (`ß`) and depends on context (the final sigma), so a program's output would depend on the toolchain it was built with; `asciiUpper` and `asciiLower` say what they do in their names, and a Unicode mapping may be added to `fortis.text` under the names `upper` and `lower`. The operations are the same on the interpreter, the VM and native code (the VM and native code share the runtime's implementation).

### 6.7 Collections

`List[T]` is an owned growable sequence. `Array[T, N]` is an owned fixed-size inline sequence. `ListView[T]` is a borrowed contiguous view `from` a `List`, `Array` or another view. `Map[K, V]` and `Set[T]` are hash containers requiring `K: Hashable, Equatable`; `OrderedMap[K, V]` requires `K: Comparable`. `FixedList[T, N]` and `FixedMap[K, V, N]` have inline capacity `N`, never allocate, and fail with `collection.full` when inserting beyond capacity. `Deque[T]`, a double-ended queue, and `SortedSet[T]`, a set kept in ascending order, are written in Fortis in `fortis.collections` and are imported from it (`import fortis.collections { Deque, SortedSet }`; X-576, X-577, below).

`Map` and `Set` use a deterministic hash function fixed per compiler release and iterate in first-insertion order (replacing a value keeps its key's position). Their iteration order is therefore deterministic for one build, identical on both backends, and may be used inside `pure` functions; code that needs an order independent of insertion uses `OrderedMap` or sorts.

Indexing `list[i]` is proof-required: `i` must have a lower bound of at least 0 from its type or a test, and an upper bound below the length, which the checker knows for `Array[T, N]`, for `List` values refined by `<length: …>`, and from a `i < list.count()` comparison that is still valid (6.3); an index the rules do not prove is rejected with `E-REF-002`, like an unproved operator (6.4; X-152). Otherwise use `list.get(i)` returning `T?` (by read loan) or `list.at(i)` returning `Outcome[T]`. The named operations of `List[T]` are `List.default()`, `List.of(...items: T)` (variadic, 6.2), `count()`, `push(editable self, item: owned T)`, `pop(editable self) T?`, `get`, `at`, `set(editable self, index: Int, value: owned T) Outcome[Void]` (failing past the end), `isEmpty()`, `first() (alias T from self)?`, `reversed()`, `concat`, `countWhere`, `map`, `filter`, the functional methods below, `iterate()`, `consumeEach(owned self, visit: fn mut(owned T) Void)` (10.7); `Array[T, N]` resolves `N` at its constructor type reference, including in a longer postfix chain or an interpolation hole; integer spellings use the ordinary literal rules (3.5, X-735). `Array[T, N]` has `Array.filled(value)`, `count()` (= `N`), indexing, `set`, `get(self, index: Int) (alias T from self)?` (a read loan, null outside [0, N)) and `asView(self) ListView[T] from self` (the array's elements as a view, which has every method `ListView` has: `arr.asView().map(f)`, `arr.asView().each(visit)`; X-578, D-542); `Map[K, V]` has `Map.default()`, `get(key) (alias V from self)?`, `set(editable self, key: owned K, value: owned V)`, `containsKey(key)`, `remove(editable self, key) V?`, `count()` and `isEmpty()`; `Set[T]` has `Set.default()`, `add(editable self, item: owned T) Bool`, `contains(item)`, `remove(editable self, item) Bool`, `count()` and `isEmpty()`. A `Map` is iterated by `for entry in map`, which binds a `(key, value)` tuple (`entry.0`, `entry.1`) by read loan in first-insertion order, and a `Set` by `for item in set`; neither has an `iterate()` method or a cursor type of its own in v0.1 (D-58, D-535). `get`, `containsKey` and `remove` only read their key, and `Set`'s `contains` and `remove` only read their item, so on a `Map[String, V]` or a `Set[String]` the key is a `StringView` (X-345, X-479): `counts.get(word)` takes a view or a `String` alike, with no `copy` to make a key that is thrown away, and a view hashes and compares equal to the `String` with the same text. `set` and `add` still take an owned key, so the allocation shows where it is real.

Eager transformation methods (`map`, `filter`, `countWhere`) take a `pure fn` callback and allocate a new collection where they return one (`filter`, `reversed` and `concat` copy elements and so require `T implements Copyable`); a lambda literal whose inferred summary is pure qualifies (11.3). No collection operation performs hidden I/O. Complexity: `count`, `get`, `at`, `set`, `pop`, `first` and `isEmpty` are O(1); `push` is amortised O(1); `List.of`, `reversed`, `concat`, `countWhere`, `map` and `filter` are O(n) plus the callback's cost per element; a cursor step is O(1); `Map.get`, `Map.containsKey`, `Map.set`, `Set.add` and `Set.contains` are expected O(1) (`set` and `add` amortised); `Map.remove` and `Set.remove` are O(n) in the entries after the one removed, because those keep their insertion order and move up one place; `fortis.collections.sorted` is O(n log n).

**Hash-container keys (X-615; D-549).** Every written or inferred `Set[T]` and `Map[K, V]` requires its element or key type to implement both `Hashable` and `Equatable`; an unmet requirement is `E-TYP-090`, even for an empty container. The check applies inside nested types, to fields, parameters and results, and after generic instantiation. In generic code the requirement must follow from the declared `where` bounds: `where K implements Hashable` suffices because `Hashable` requires `Equatable` (6.10); writing both bounds is also accepted. A class has no interface `where` bound in v0.1 (`where T: linear` declares linearity only, 6.12), so a field `Map[K, V]` whose `K` is the class's unbounded parameter is rejected; a concrete key type or a type parameter for the whole container is used instead. Triviality and copyability (6.11) do not imply either interface; tuples and anonymous records do not satisfy interfaces (6.9), so they are not keys. In particular an enum without an implementation or derivation of these interfaces is not a key merely because its payloads are trivial. The existing scalar and text implementations and newtype forwarding (6.9) are unchanged; a class or enum may derive `Equatable, Hashable` when its fields or payloads satisfy those interfaces. These requirements belong to the standard containers, not to a user type that happens to be named `Set` or `Map`.

**Collections in a functional style (X-479).** The owner asked for "good collection types and wrappers, including functional designs as well as traditional functions", such as "an 'each' method which takes a lambda and passes the lambda each item as its parameter". `List[T]` and `ListView[T]` have the methods below beside the loops and cursors above, which stay; `Map` and `Set` have `each`, and the helpers of `T?` and `Outcome[T]` follow in 6.8 and 10.2. They are written in Fortis in the standard library, with a `for` loop over `self`, so every backend has them and none needs an intrinsic. Each one's callback is a `pure fn` where the method only computes (`map`, `fold`, `any`, `groupBy`, …), as `map` and `filter` always were, and a lambda that does I/O cannot be passed to one (`E-TYP-040`); `each` and `tryEach` take a callback that may have effects, `editable fn mut`, so a `fn mut` lambda that changes a captured binding qualifies, and they are not `pure`. Callback parameters contribute the effects their function type permits (11.3), so a `pure` function cannot call `each` or `tryEach`, whatever lambda it passes (`E-EFF-001`). The callback sees each element by read loan, so a method that builds a new value from it copies it, and says so in its signature: every method that returns elements (`take`, `skip`, `enumerate`, `zip`, `chunked`, `sortedBy`, `groupBy`, `keys`, `values` and the set operations) requires `Copyable` elements, as `filter` does, while a method that only looks at them (`each`, `fold`, `any`, `all`, `find`, `indexWhere`, `contains` with `Equatable`) does not, so a list of resources can be visited, folded and searched. A method that allocates says so in its effect, and a `noalloc` function cannot call it.

```fortis
// illustrative
// List[T] and ListView[T]
func each(self, visit: editable fn mut(T) Void)
try func tryEach(self, visit: editable fn mut(T) Outcome[Void])
pure func map[U](self, f: pure fn(T) U) List[U] where U: linear
pure try func tryMap[U](self, f: pure fn(T) Outcome[U]) List[U]
pure func filter(self, keep: pure fn(T) Bool) List[T] where T implements Copyable
pure func fold[A](self, initial: owned A, step: pure fn(A, T) A) A
pure try func tryFold[A](self, initial: owned A, step: pure fn(A, T) Outcome[A]) A
pure func any(self, holds: pure fn(T) Bool) Bool
pure func all(self, holds: pure fn(T) Bool) Bool
pure func find(self, holds: pure fn(T) Bool) (alias T from self)?
pure func indexWhere(self, holds: pure fn(T) Bool) Int?
pure func contains(self, target: T) Bool where T implements Equatable
pure func sortedBy[K](self, key: pure fn(T) K) List[T] where T implements Copyable, K implements Comparable
pure func enumerate(self) List[(Int, T)] where T implements Copyable
pure func zip[U](self, other: ListView[U]) List[(T, U)] where T implements Copyable, U implements Copyable
pure func take(self, n: Int) List[T] where T implements Copyable
pure func skip(self, n: Int) List[T] where T implements Copyable
pure func flatMap[U](self, f: pure fn(T) List[U]) List[U] where U: linear
pure func chunked(self, size: Int<1..>) List[List[T]] where T implements Copyable
pure func groupBy[K](self, key: pure fn(T) K) Map[K, List[T]] where T implements Copyable, K implements Copyable, K implements Hashable, K implements Equatable
// Map[K, V]
func each(self, visit: editable fn mut(K, V) Void) where K implements Hashable
pure func keys(self) List[K] where K implements Hashable, K implements Copyable
pure func values(self) List[V] where K implements Hashable, V implements Copyable
// Set[T]
func each(self, visit: editable fn mut(T) Void) where T implements Hashable
pure func union(self, other: Set[T]) Set[T] where T implements Hashable, T implements Copyable
pure func intersection(self, other: Set[T]) Set[T] where T implements Hashable, T implements Copyable
pure func difference(self, other: Set[T]) Set[T] where T implements Hashable, T implements Copyable
```

The `try` methods take a callback that returns an outcome, which is what a lambda whose body is a call of a `try` function, or a fallible `as` (6.13), has when it is written as it reads: `lines.tryMap(fn(line) => line as Int) or fail`, `rows.tryEach(fn mut[editable store](row) => store.save(row)) or fail`, `prices.tryFold(0 as Pence, fn(sum, p) => addPence(sum, p)) or fail`. They stop at the first failure, in element order, and return it; the later elements are not visited, and a list `tryMap` was building is dropped. They are `try` functions, so their call is handled on the line (10.2): unhandled, the outcome is `E-OBL-001`; `or fail` outside a `try` function, `E-ERR-001`. A callback that returns a plain value is no `try` callback: it is `E-TYP-040`, which names the `Outcome[…]` the callback must return ("must return an outcome, Outcome[U]; the callback returns Int"), in both checkers, where the result type used to be reported unfixed as `E-TYP-160` (X-579, D-536).

Meanings and costs, in the order above (n is `count()`, m the other collection's, and "plus the callback" means plus its cost per call). `each`, `tryEach`, `any`, `all`, `find`, `indexWhere`, `contains`, `fold` and `tryFold` are O(n) plus the callback and allocate nothing; `any`, `all`, `find`, `indexWhere`, `contains` and the `try` methods stop early. `find` lends the first element the callback accepts, null when none does; `indexWhere` gives its index. `fold` passes the accumulator by read loan and takes the next one from the callback's result, so it is for values the callback builds anew (a sum, a longer text); accumulating into a collection is `each` over a `let mut`. `tryMap`, `flatMap`, `enumerate`, `zip` (as many pairs as the shorter list has, O(min(n, m))), `take` and `skip` (`take(n)` is empty for an `n` of 0 or less and everything for an `n` past the end, `skip(n)` the reverse, so neither fails), `chunked` (lists of `size` elements, the last one shorter; the size is an `Int<1..>`, so `chunked(0)` is `E-REF-001` and a size not shown to be at least 1 needs a test) allocate and are O(n) plus the callback. `sortedBy` is a stable merge sort over the elements' keys, which it computes once each: O(n log n) comparisons plus n calls of the key function, with O(n) extra space; elements with equal keys keep their order. `groupBy` gathers copies under their key in a `Map` whose keys are in the order they first appear, each group in the original order: expected O(n) plus n key calls. `Map.each` and `Set.each` visit in first-insertion order, O(n) plus the callback; `keys` and `values` copy, O(n); `union` gives self's items and then other's items that self lacks, `intersection` and `difference` keep self's order, and each builds a new set in expected O(n + m). `fortis.collections.sorted` is unchanged; the module also holds `Deque` and `SortedSet`, which follow.

**A double-ended queue and a sorted set (X-479, X-576, X-577).** The owner chose "more collection types: a queue or deque, a sorted set, plus helpers" (29 Oct 2026). Both are classes written in Fortis in `fortis.collections`, over `List`, so the interpreter, the VM and native run them with one intrinsic between them (`Deque.iterate`, below), and both are named by an import. Their methods follow the rules above: a callback that only computes is a `pure fn`, `each` and `tryEach` take `editable fn mut`, the methods that return items require `Copyable`, those that only look need none, and a method that allocates has an `alloc` row in the effect data (11.3), so a `noalloc` function cannot call it. Neither type implements `Equatable`, `Hashable` or `Displayable` in v0.1, and neither is `Copyable`: `copy` of one is `E-OWN-002`, as for any generic class, whose copyability is judged on its declaration where `T` is not known to be `Copyable` (D-548); `toList()` makes copies of the items; `fromList(items: owned List[T])` consumes its list and transfers its items (D-621). Neither declares `T: linear` (6.12): the element type is non-linear, and a queue or set of linear items is to be rejected like any other generic class instantiated at a linear type, as the checkers enforce for every user generic type (D-547). Variadic `of(...)` is the standard package's compiler form for `List.of` alone (6.2), so both are built from a list, `Deque.fromList(items)` and `SortedSet.fromList(items)`, not from arguments.

```fortis
// illustrative
// Deque[T]: the first item is at the front, the last at the back
pure func default() Deque[T]
pure func fromList(items: owned List[T]) Deque[T]
pure func count(self) Int
pure func isEmpty(self) Bool
func pushBack(editable self, item: owned T)
func pushFront(editable self, item: owned T)
func popFront(editable self) T?
func popBack(editable self) T?
pure func get(self, index: Int) (alias T from self)?
pure func peekFront(self) (alias T from self)?
pure func peekBack(self) (alias T from self)?
pure func iterate(self) ListCursor[T] from self
func each(self, visit: editable fn mut(T) Void)
try func tryEach(self, visit: editable fn mut(T) Outcome[Void])
pure func map[U](self, f: pure fn(T) U) Deque[U]
pure try func tryMap[U](self, f: pure fn(T) Outcome[U]) Deque[U]
pure func filter(self, keep: pure fn(T) Bool) Deque[T] where T implements Copyable
pure func fold[A](self, initial: owned A, step: pure fn(A, T) A) A
pure try func tryFold[A](self, initial: owned A, step: pure fn(A, T) Outcome[A]) A
pure func any(self, holds: pure fn(T) Bool) Bool
pure func all(self, holds: pure fn(T) Bool) Bool
pure func find(self, holds: pure fn(T) Bool) (alias T from self)?
pure func indexWhere(self, holds: pure fn(T) Bool) Int?
pure func contains(self, target: T) Bool where T implements Equatable
pure func toList(self) List[T] where T implements Copyable
// SortedSet[T]: the members are in ascending order, each once; every method that compares needs `T implements Comparable`
pure func default() SortedSet[T] where T implements Comparable
pure func fromList(items: owned List[T]) SortedSet[T] where T implements Comparable
pure func count(self) Int
pure func isEmpty(self) Bool
func add(editable self, item: owned T) Bool where T implements Comparable
pure func contains(self, item: T) Bool where T implements Comparable
func remove(editable self, item: T) Bool where T implements Comparable
pure func first(self) (alias T from self)?
pure func last(self) (alias T from self)?
pure func get(self, index: Int) (alias T from self)?
pure func iterate(self) ListCursor[T] from self
func each(self, visit: editable fn mut(T) Void)
pure func fold[A](self, initial: owned A, step: pure fn(A, T) A) A
pure func any(self, holds: pure fn(T) Bool) Bool
pure func all(self, holds: pure fn(T) Bool) Bool
pure func find(self, holds: pure fn(T) Bool) (alias T from self)?
pure func toList(self) List[T] where T implements Copyable
pure func between(self, low: T, high: T) List[T] where T implements Comparable, T implements Copyable
pure func union(self, other: SortedSet[T]) SortedSet[T] where T implements Comparable, T implements Copyable
pure func intersection(self, other: SortedSet[T]) SortedSet[T] where T implements Comparable, T implements Copyable
pure func difference(self, other: SortedSet[T]) SortedSet[T] where T implements Comparable, T implements Copyable
```

*The queue.* A `Deque[T]` keeps its items in two lists, `front` with the first item last and `back` with the last item last, so `pushFront`, `pushBack` and the two pops are each a `push` or a `pop` of a list. A pop at an end that has no items takes half of the other end's items over (a rebalance) before it pops; the moves of a rebalance are paid for by the pushes that made it necessary, so `pushBack`, `pushFront`, `popFront` and `popBack` are amortised O(1) at both ends and O(count) in the worst call, no item is ever copied, and a queue holds resources. `count`, `isEmpty`, `get`, `peekFront` and `peekBack` are O(1) and lend their item by read loan, as `first()` does; `get(i)` counts from the front and is null outside [0, count). `popFront` and `popBack` return the item owned, or null when the queue is empty. A `for` over a queue visits the items from the front to the back (the `Iterable` implementation below); `each`, `map`, `filter`, `fold`, `any`, `all`, `find`, `indexWhere`, `contains`, `toList` and the `try` forms visit in that order too, `map` and `filter` give queues, `find` lends, `indexWhere` counts from the front, and the costs are those of the `List` methods of the same name. A queue that ends while it holds items destroys them in no order the language promises (they are cleaned up once each); one that is drained with `popFront` or `popBack` cleans each item up when the binding that received it ends.

*The sorted set.* A `SortedSet[T]` keeps its members in a list in ascending order, each once, ordered by the `compare` of `Comparable` (`<`). `contains` and the search inside `add` and `remove` are a binary search over `get`, O(log n); making room for a new member or closing the gap a removal leaves moves the members after it, one `pop` and one `push` each, so `add` and `remove` cost O(log n) comparisons plus O(n) moves in the members after the one added or removed, as `Map.remove` does (D-543, D-546); `fromList` adds its items one by one, so it is O(n) times that. `add` says whether the item was new and a duplicate changes nothing (two members never compare equal); `remove` says whether the item was a member. `first`, `last` and `get(i)` lend the smallest, the largest and the `i`th member in O(1). A `for` over a set visits the members from the smallest to the largest (the `Iterable` implementation below), as `each`, `fold`, `any`, `all`, `find` and `toList` do. `between(low, high)` copies the members from `low` up to, but not including, `high`, in ascending order: O(log n) to find `low` and O(k) for the `k` copied, and empty when `high` is not above `low`. `union`, `intersection` and `difference` merge two ascending lists in one pass, O(n + m), and give sets of copies. A class that `derive`s `Comparable` orders by its fields in declaration order (9.2), so it can be a member.

*`for` over a class that implements `Iterable` (X-576, D-545).* Both types implement `Iterable[T, ListCursor[T]]`. A `for` over any class that implements `Iterable[T, I]` calls its `iterate()` and walks the cursor it returns, binding each item by read loan, on the interpreter, the VM and native alike; until now the checkers accepted such a loop and every backend stopped on it (D-545). A library collection that keeps its items in one list returns that list's cursor (`SortedSet`); `Deque.iterate` is a trusted intrinsic, the one this part adds, because a class may not store a cursor (7.7) and a queue's items are in two lists: it makes a standard cursor that walks the items of `front` from the last to the first and then those of `back`, which costs O(count) to make on the interpreter and the VM, which list the items, and O(1) on native, and O(1) per step everywhere.


A write to an index place of a `List[T]`, `Array[T, N]` or `FixedList[T, N]` checks the assigned value against `T`, including its refinements and optional members, exactly as `set` checks its value argument (X-711; `E-REF-001` for an excluded value). This does not add new place syntax (7.2).

### 6.8 Optional values

`T?` is either `null` or a present `T`. A value of type `T` is accepted where `T?` is expected (present injection is implicit and free); nothing converts in the other direction. `a ?? b` yields `a`'s value when present, otherwise `b` (evaluated lazily). `a?.field` is `null` when `a` is `null`. `match` over `T?` uses the patterns `present(x)` and `null`. Null tests narrow flow-sensitively: after `if value != null { … }` the place `value` has type `T` inside the branch; after `if value == null { … }` whose block ends in `return`, `fail`, `break`, `continue` or a `Never` call, `value` has type `T` for the rest of the enclosing block; and in `a != null && e`, `a` has type `T` inside `e`. Narrowing ends at any invalidation event on the place (6.3). `T?` is never an error and never a Boolean: `if maybe { }` is rejected (`E-TYP-010`).

**`map` on an optional (X-479).** A `T?` has one method, `map[U](self, f: pure fn(T) U) U? where U: linear`: `maybeSize.map(fn(s) => s.label()) ?? "none"` applies `f` to the present value, lent by read loan, and gives its result as the present result; `null` stays `null` and `f` is not called. It is the one method called with `.` on a `T?` without a null test; `?.` calls a method of the present value (below). The receiver is an ordinary read argument: a place is lent for the call, and the callback's `T` is the value, not a loan of it. `map` is the only helper because the operators already do the rest: `??` supplies a value for `null`, `?.` reaches into a member, a `match` or a null test narrows, and `map` applies an arbitrary function to the value. A `filter`, a `flatMap` or an `each` on `T?` would only be a second way to write a null test (`E-TYP-040` stays for them). The standard library holds it as `Optional.map(value, f)` (the call `maybe.map(f)` is that call with the receiver first), and `Optional` is not a type a program names.

**Optional method calls (X-630, X-632).** `a?.m(args)` evaluates `a` once. If it is null, the result is null and no argument is evaluated; otherwise it calls `m` on the present value and injects its result into an optional. A result `U` becomes `U?`; a result already `U?` stays `U?`. Postfix precedence is unchanged: chains such as `a?.b()?.c()` and `a?.field?.m()` apply this rule at each step. Mixin and interface methods resolve as on the present value after a null test. In v0.1 the method must have a read receiver (`self`) and must not be `try`; an editable, consuming or `try` method is `E-TYP-040`, with the correction to test for null and call the method in that branch, using its receiver mode and handling its outcome. An argument must not consume or discharge an obligation that exists outside that argument, including an outcome or a `T: linear` value (X-710, X-730; `E-OBL-001` at the argument). This includes `move`, `or fail`, `or recover`, returning from `catch`, `match`, `discard` and passing the value on, at any depth inside the argument or a chain of optional calls, including a move into a closure capture. A value created and handled wholly inside the argument remains permitted: it is never created on the skipped path. A closure body is checked as its own activation; moving an existing obligation into its capture is still forbidden. Test for null with `match` or `if let` and call in the present branch, consuming the obligation on the null branch too. Non-linear owned arguments remain permitted. A call imports the method's effects (11.3), even when its receiver may be null. A field projection used as the receiver of a following postfix call lends that field from its owner, like an ordinary field receiver; a standalone optional field read keeps its existing value rule. A result carrying a loan, including `(alias T from self)?` and `StringView from self`, carries the same owner as the receiver, with the same lifetime, escape and conflict rules as the ordinary call after a null test (7.6, 7.7). The formatter preserves both `?.m()` and `map(fn(x) => x.m())` as written.

### 6.9 Tuples, anonymous records and newtypes

`(A, B)` is a tuple; fields are `t.0`, `t.1`. `{ name: A, other: B }` is an anonymous record type; a value is written `{ name: x, other: y }`; record types carry no field defaults. `VariantMap[E, V]` is a map from the variants of an enum `E` to values of `V`, written with the variant names as keys (`{ Found: 200, NotFound: 404 }`) and checked for one entry per variant. A type may be parenthesised for grouping, which is how an optional callable is written: `(fn(Int) Bool)?` is an optional function, while `fn(Int) Bool?` is a function returning an optional. Tuples, records and variant maps are structural, local conveniences: they may not appear in an exported signature (`E-TYP-030`) except as aspect parameters and in the signatures of the `fortis` package itself (`join` returns `(A, B)`, 12.5), and do not satisfy interfaces.

Tuple assignability requires the same number of elements and checks each corresponding element. Anonymous record assignability requires exactly the same field names and checks the corresponding field types; field order does not change the type. The runtime representation distinguishes corresponding field types recursively, including optional layers; refinements retain the representation of their base type (X-732). Each element or field follows its own assignability rules, including refinement widening where its type permits it, and an unproved refinement is `E-REF-001`. A mismatched shape or element type is `E-TYP-040`. These checks apply in every value position (X-686). Read loans, owned moves and copies keep this covariant walk. An `editable` loan compares corresponding fields and elements invariantly, recursively through optional, record, tuple, container and function-signature members (X-712): a writable place cannot be widened, for the same reason that optional widening (6.3, D-636) cannot widen a container element. A different field or element type in such a loan is `E-TYP-040`. Container type arguments remain invariant.

`type Name = Representation` declares a nominal newtype with a private representation; it is the only form `type` has, so a reader never has to ask whether a `type` is transparent. `CustomerId` and `OrderId` over the same `Int<1..>` never interchange: a `CustomerId` where an `OrderId` is expected, or where its representation is (`let n: Int = id`), is `E-TYP-040`, as is a value of any other type where the newtype is expected. A value is constructed by `raw as Name` (pure, fallible unless the representation is proved), by its accepted v0.1 call spelling `Name.validate(raw)`, or by `Name(x)`, which is checked as a call of `func Name(value: Representation) Name`, with `value` taken `owned` where the representation is ownership-bearing: `x` is a literal the checker proves in range (`Name(42)`), a value of the representation type, or a value whose interval the rules of 6.3 prove within the representation's refinement. A literal or value the rules do not prove in range is `E-REF-001` (`Percent(140)` for `type Percent = Int<0..100>`; `Small(n)` for an `Int` nothing has narrowed), and a value of another type is `E-TYP-040` (`Small("five")`) (X-511). There are no transparent type aliases in v1. A newtype implements `Equatable`, `Hashable`, `Comparable` and `Displayable` exactly when its representation does, whatever the representation (display shows the underlying value), so `CustomerId` can be interpolated and used as a map key without a declaration, and so can a `type Code = String<scalars: 1..10>`; a newtype over a trivial representation is itself trivial (6.11) (X-511).

**Newtypes in and out (X-552).** For `type Pence = Int<0..>`, `raw as Pence` takes the representation and returns `Outcome[Pence]` when its range is unproved, failing with `refinement.out_of_range`; `42 as Pence` and a value proved non-negative return `Pence` directly, under the construction judgement above. Inside the declaring module, `p as Int` unwraps a `Pence` totally and has type `Int<0..>`, keeping the representation's refinement even when the target writes only its base. Outside that module, the same unwrap is `E-NAM-004` (not visible). An unwrap is one step to the declared representation, never a search through nested newtypes. An ownership-bearing representation requires `move` or `copy` for a place in either direction, following the construction and move rules of 7.5; a conversion cannot turn a loan into an owner. Newtypes declare no conversion members. The retired `Name.validate(raw)` call keeps its outcome result in v0.1; a borrowed `Copyable` representation is copied, with the allocation effect of `copy`, while a non-copyable representation must be passed owned. Borrowing a resource cannot create a second owner (D-518, X-553).

A representation that is linear (6.12), including a collection of linear elements, may be wrapped only when the representation refinement is proved. A refinement preserves the linearity of its base, so refining a collection never removes its elements' obligations. An unproved `as` or retired `validate` call is `E-OBL-001`: its failure would discard the representation without consuming its obligations (10.7). A proved `as` remains total, a proved `validate` retains its outcome result, and an unwrap still transfers the obligations to the representation (X-554, D-522).

**Collection newtypes (X-479, X-581).** A newtype whose declared representation is a collection of 6.7 offers that collection's instance methods, with the representation's type arguments and result types. A reading method (`self`) is visible outside the declaring module; an editing or consuming method (`editable self`, `owned self`) is visible only inside it, and a call outside is `E-NAM-004`, naming the method. Reading iteration (`for item in basket`) follows the representation too; consuming iteration is private like a consuming method. A method returning `List[T]` returns `List[T]`, never the newtype. A representation of `ListView[T]` keeps its loan and region when wrapped; construction and conversion preserve the argument’s loan, and a function returning the nominal view must name its origin. Receiver loans and refinements keep their ordinary rules, including the prohibition on editing a length-refined collection (6.3). This rule looks through a refinement of a collection, but not through another newtype. It exposes no associated functions, fields, indexing, conversion or interchange with the representation; 6.9's construction rules and private unwrap (X-552) are unchanged.

### 6.10 Generics and built-in interfaces

Type parameters are written `[T]` on functions, classes, enums and interfaces. Constraints use `where`:

```fortis
// illustrative
export pure func largest[T](items: ListView[T]) T? where T implements Comparable, T implements Copyable {
  …
}
```

A generic function is compiled once per instantiation, so one that calls itself at an ever larger type (`nest(Box(value: copy x))` inside `nest[T]`) has no last instantiation; a native build stops with `E-LIM-001`, naming the instantiation-depth budget (32) and the chain, before lowering (X-134). `where` follows the return type (and a `from` clause when present) and precedes `{` or `=>`. A generic body is checked once against its bounds; a use at a type that satisfies the bounds cannot fail to compile. Inside the body a `T` is ownership-bearing whatever it is instantiated at: a parameter `x: T` is a loan, so returning it, storing it or passing it to an `owned` position is judged as for any loan (7.5): the place needs a marker (`E-OWN-001`), `copy x` is allowed only with `T implements Copyable` (`E-OWN-002` without it), and a loaned `T` a call returns (`xs.get(0)` in `first[T]`) cannot be made owned at all without that bound (`E-OWN-002`), as 6.12 says a function that only takes `T` by loan never owns one (X-513; D-352). The native lowering gives a type parameter the Rust bounds its `where` clause names (`Copyable` is `Clone`, `Equatable` is `PartialEq`, `Comparable` adds `PartialOrd`, `Hashable` adds `Eq` and `Hash`, `Displayable` is the display trait, a program's interface is its trait) and no other, which is what makes a use at a type that satisfies the bounds build (X-515; D-396). Type arguments at a call are always inferred; there is no `f[Int](x)` syntax in v1 (a lowercase name followed by `[` is an index). A type argument is fixed by the call's arguments, or by the call's expected type: an annotated binding, a return, a parameter, the success type a `catch` recovers to, or the type a `match` or if-expression gives. An argument of the parameter's type fixes it; arguments that disagree are joined to their common type, two integer refinements of one base to the hull of their intervals and a narrower type to the wider one that contains it (`pick(small, big)` with `small: Int<0..3>` and `big: Int<0..9>` is an `Int<0..9>`), and are `E-TYP-040` when they have none; `null`, the absent case of any `T?`, fixes nothing (`orElse(null, 3)` is an `Int`). A parameter that only the arguments fix and that the result names inside a type argument takes an integer argument's base type, as an unannotated `let` widens its initialiser (5.1), the interval staying a fact of the argument: `List.of(i)` with `i: Int` known to be 4 is a `List[Int]`, not a `List[Int<4..4>]` that the invariance of type arguments (6.3) would set apart from `List[Int]`, and `List.of(small, big)` is a `List[Int]` too, a `List[Int<0..9>]` being written as the expected type (`let xs: List[Int<0..9>] = List.of(small, big)`); a result that is the parameter itself (`pick`, `T?`) keeps the argument's interval, since the value passes through. Where the call has an expected type, it fixes the parameters it names first and every argument is checked against them, which is how `let xs: List[dynamic Shape] = List.of(Circle(…), Square(…))` boxes each element as it is made; a parameter the expected type does not name is the arguments' alone (X-513, after the owner's answer to Q3 of the v0.1 plan). A generic class's constructor takes its type arguments from the written type, else from the expected type (`let p: Pair[Int, String] = Pair(a: 1, b: "x")`), and checks each argument against them (D-66). A call that leaves a type argument unfixed (`let names = List.default()`) is `E-TYP-160`, repaired by annotating the receiving binding. Static dispatch is the default and the backend monomorphises. Higher-kinded types, recursive associated types and overloading are not supported.

**Language 1.0 limit: no associated call on a type parameter (X-180).** Inside a generic body, a type parameter's associated functions cannot be called: `T.default()`, `T.fromText(text)`, `T.fromBytes(bytes)`, `T.from(source)` and an interface's own associated function (`T.create()`, also written with type arguments, `T[Int].create()`) are `E-TYP-170`, in a function, a method, a class's `cleanup`, field defaults and constants, and a parameter's default alike. Calling one needs the type argument at run time, which Language 1.0's VM and definitional interpreter do not carry; the full type-passing implementation lifts the limit in a later release. A generic body works with the values of `T` its caller passes: a value to fall back to, or a function that makes one (`make: fn() T`). Methods of `T`'s bounds with a receiver are unaffected (`value.label()` where `T implements Labelled`), and a concrete type's associated functions stay callable (`Point.fromText(text)`).

The built-in interfaces in `fortis.prelude` are:

| Interface | Methods | Notes |
|---|---|---|
| `Equatable` | `pure func equals(self, other: Self) Bool` | `==` and `!=` require it |
| `Hashable` | `pure func hash(self, into: editable Hasher) Void` | requires `Equatable` |
| `Comparable` | `pure func compare(self, other: Self) Ordering` | `<` `<=` `>` `>=` require it; requires `Equatable` |
| `Displayable` | `pure func display(self, into: editable text.Builder) Void` | interpolation requires it |
| `Iterable[T, I]`, `Iterator[T]`, `Consumable[T, I]`, `Consumer[T]` | section 5.3 | read and consuming iteration |
| `Copyable` | structural; no methods | section 6.11 |
| `Shareable` | structural; no methods | section 12.7 |
| `Abandonable` | `func abandon(owned self) Void` | lets a linear value live across a cancellation point or an `or fail`; the unwind abandons it (12.4) |
| `Default` | `pure func default() Self` | associated function |
| `Encodable` | `pure func encode(self, into: editable Encoder) Void` | describes the value as a tree of fields, lists, scalars, optionals and variants; format-agnostic |
| `Decodable` | `try func decode(source: editable Decoder) Self` | the reverse, running refinements and nominal constructors; associated |
| `Codec` | bundle: `requires Encodable, Decodable` | `derive Codec` derives both |
| `Authority` | marker | implemented by capabilities and per-activation authorities (11.6) |
| `From[S]` (retired) | `func from(source: S) Self` | accepted in v0.1; its conversion is written `source as Self`, and `derive From[S]` is rewritten by `fortis fmt` to `derive convert from S` (6.13, X-552) |

There are no user-defined operators and no operator overloading: `Money + Money` is written `Money.add(a, b)`. The operators `== != < <= > >=` dispatch to the built-in interfaces only: `==` and `!=` call the type's `equals` and `< <= > >=` its `compare`, on every backend, whether the implementation is hand-written or derived, and a `Map` or `Set` finds a key by the same `equals` and `hash` (X-516; D-84). A core type that implements `Equatable` or `Comparable` provides `equals` and `compare` as ordinary methods, callable directly (`"a".compare("b")`, `a.equals(b)`) and through a `where` bound, on every backend (X-516; D-85). `Hasher` has no members in Language 1.0, so a hand-written `hash` writes nothing and every value of the type hashes alike: the containers then decide by `equals` alone, and a hand-written `equals` is what `==`, a container lookup and a `where T implements Equatable` body call (X-516).

`derive` on a class or enum generates an implementation of `Equatable`, `Hashable`, `Comparable`, `Displayable`, `Encodable`, `Decodable` or `Codec` field-wise, and is accepted only when every field (or payload) implements the interface (`From[S]` and `Default` have their own rules; a derivation whose rule is not met is `E-TYP-090`); `derive Default` builds the value from each field's declared initialiser, falling back to the field type's `Default` only for fields without one, so a class whose fields all have initialisers derives `Default` regardless of their types. On a class it generates `Name.default()`, the interface's associated function, which takes no arguments: a class whose fields all start at a default is created without a hand-written constructor, and the header says so (X-345); a field that starts otherwise is written with its initialiser. Written without its call, `Name.default` is that function's value (`let make = HttpLimits.default`), which builds the value when it is called (X-516; D-170). A field with neither an initialiser nor a type that has `Default` (a class that does not derive or implement it, a capability, a refinement that excludes 0 or empty, an `Array`, a `FixedList`, a `FixedMap`, an `OrderedMap`, or a type parameter, which a class has no `where` clause to require a default of) makes the derivation `E-TYP-090`, and so does `derive Default` on an enum, where nothing chooses the variant: an enum that has a default implements `Default` itself (D-242). A newtype's default is its representation's, so the rule looks through newtypes, at any depth; the containers are the prelude's, known by their declaration, so a class a program names `Array` is judged by what it declares. A default that contains itself has no end: a field without an initialiser whose type unfolds, through newtypes and classes that derive `Default`, back to a class or newtype whose default is being built (`type Wrap = Inner` with `class Inner derive Default { w: Wrap }`, two classes that hold each other, a class that holds itself) makes the derivation `E-TYP-090`, naming the cycle (D-335, D-336); an optional field (`null`), a collection (empty), an initialiser or a class that implements `Default` itself ends it. `OrderedMap`, `FixedList` and `FixedMap` have no default until every backend builds an empty one, and until then a field of one without an initialiser is `E-TYP-090` (X-345). `Bool` (`false`), the unrefined integers (`0`), `Float` (`0.0`), `Duration` (`0s`), `String`, `Bytes`, `List`, `Map`, `Set` (empty) and `T?` (`null`) implement `Default`; a refined type does only when its refinement admits the representation's default; a class or enum that implements `Default` gives its own `default()`; `Encodable` and `Decodable` additionally require every field to be exported, so a class with private state writes `encode`/`decode` by hand or stays off the wire. A class that declares a method an interface would derive cannot also derive that interface (`E-MIX-001`). `Comparable` derivation orders by declaration order: a class compares its fields in order and the first difference decides, and an enum orders by variant declaration order first and, within one variant, by the payload's fields in order (X-516, after R5 of the v0.1 plan; D-54.1).

The retired generic method `func to[T](self) T where T implements From[Self]` remains accepted in v0.1, usable wherever the target type is already known (an argument, a return, a field initialiser, an annotated `let`): `Found(customer: customer.to())`. Where it is not known, write `customer as CustomerResponse`. The documented conversion is `source as Target` (6.13, X-475). `fortis fmt` rewrites the retired `to()` when its expected target is written in an argument parameter, return type, field type or annotated `let`; otherwise it keeps the call and prints a note (X-552).

Encodability is recursive: a derived `Encodable` requires every field type to be `Encodable`, so a class containing a `Secret[T]` or a resource cannot be derived, and the same holds for `Decodable`. A `type` newtype implements `Encodable`/`Decodable` exactly when its representation does, encoding as that representation and decoding through `validate`. On top of the two interfaces the core defines one canonical pair that needs no format package, with these signatures in `fortis.prelude`: `pure func toBytes(self) Bytes` and `try func fromBytes(input: BytesView) Self` (deterministic, versioned binary, identical on both backends; the byte format is published with the conformance suite), and `pure func toText(self) String` and `try func fromText(input: StringView) Self` (canonical text: JSON with keys in declaration order, no insignificant whitespace). All four allocate; decoding follows the expected type and fails with `codec.invalid` (malformed input, a value that does not fit its type, a missing field that has neither an initialiser nor an optional type, trailing data) or `codec.unknown_field` (a text key the type does not declare), and a value outside a refinement or a newtype's refinement fails with that refinement's own failure, exactly as `validate` would (both formats, version 1, are published as `conformance/codec/FORMAT.md`; a nested optional `(T?)?` has no canonical form and is not `Encodable`). Calling the pair on a type that is not `Encodable`/`Decodable` is `E-TYP-090`. Named formats (`fortis.codec.json`, `…yaml`, `…csv`, `…toml`, `…msgpack`) consume the same tree and add their own methods through mixins: `JsonSerialised`, which requires `encode`, provides `toJson()`, and `JsonDeserialised`, which requires `decode`, provides `fromJson(input)`, so an output-only type takes just the first. The canonical pair is never overridable. `Displayable` stays separate from encoding: `${value}` is a human rendering and never the encoded record. `Secret[T]`, capabilities, loans, resources, closures, futures and tasks implement none of these.

### 6.11 Trivial and copyable types

A **trivial** type is duplicated implicitly: assigning or passing a trivial value never needs an ownership marker. The set is closed:

| Trivial types |
|---|
| `Void`, `Never`, `Bool`, all integer types, `Float32`, `Float`, `Decimal`, `Byte`, `Scalar`, `Duration`, `Instant`, `Timestamp` |
| `type` newtypes over a trivial representation |
| enums whose payload fields are all trivial; `T?` where `T` is trivial |
| tuples and anonymous records all of whose fields are trivial |
| `trivial class Name { … }` — explicit opt-in; rejected if any field is non-trivial (`E-OWN-030`) |

Everything else is **ownership-bearing** and needs `copy`, `alias` or `move` when an existing place is assigned, returned or passed to an `owned` parameter. A class is ownership-bearing unless declared `trivial`, so adding a field to an ordinary class never changes the meaning of an assignment elsewhere.

A **copyable** type supports `copy`. The set is structural: trivials; `String`, `Bytes`, `BigInt`; `List`, `Array`, `Map`, `OrderedMap`, `Set`, `FixedList`, `FixedMap` with copyable elements; classes, enums, tuples and records all of whose fields are copyable. Not copyable: `Shared`, `Weak`, `Mutex`, atomics, loans and views, resources (any class holding a native handle), `Future`, `Task`, `Outcome`, `Failure`, `Must`, closures. A linear type is never Copyable (6.12, 10.7; X-696): this exclusion applies before the structural rules, including a generic instantiation or aggregate containing a linear value. Neither `derive Copyable` nor a `Copyable` bound permits copying a linear value. `copy` is a compiler-generated field-wise duplicate; it cannot fail and never shares mutable state. A type that needs a fallible or custom duplication exposes an ordinary `try func clone(self) Self`. `copy` of a `String`, `List` or other heap-owning type allocates and is rejected inside `noalloc`.

### 6.12 Linear types

A **linear** value must be consumed exactly once on every path. The linear types are `Outcome[T]`, `Failure`, `Future[T]`, `Task[T]`, the zero-sized witness `Must` (created by `Must.create()` and consumed only by `Must.discharge(move m)`, which a library uses as a private field to make a handle such as a transaction linear), `fn once` closures that capture one, and any type whose values can contain a linear value: `T?`, classes, enums, tuples, records and collections over a linear type. `pop()` and `Consumer.next()` on a linear element type therefore return a linear `T?`, which must be consumed with `match` or `??`; `Iterator.next()` only loans the element, leaving the obligation in the collection. A type parameter is non-linear unless declared `T: linear`. The bound matters only where the function *holds* a `T` by value (an `owned T` parameter, a local, a field, a return): such a function cannot be instantiated with a linear type unless it says `T: linear` and consumes the value (`E-OBL-003`, X-519). A user generic type (class or enum) may be instantiated at a linear argument only if its corresponding parameter declares `T: linear` (`E-OBL-003`). A class or enum writes the bound in a `where` clause at the end of its head, before its body, as a function does (`class Bin[T] where T: linear { … }`, X-611); this clause on a class or enum accepts linear bounds only. This applies to explicit and inferred arguments, including those inferred from a later use or passed through a generic function. Its fields, payloads and representation are substituted before judging linearity, so an instantiation holding a linear value is itself linear; bodies under `T: linear` must consume what they own (10.7). A function that only takes `T` by loan (`x: T` or `x: editable T`) never owns one, so it may be instantiated with a linear type freely; the obligation stays with the caller. For the same reason it cannot return, store or give away the `T` it was lent: a `T` is ownership-bearing in every generic body, and only `copy x` with `T implements Copyable` makes an owned one (6.10, X-513; D-352). The consuming operations are listed per type in sections 10 and 12. Dropping a linear value at block exit, overwriting it by assignment or `discard`ing it is an error (`E-OBL-001`), with the single exception for cold futures in 12.3.

### 6.13 Conversions

One operator converts a value from one type to another: `expression as Type` (X-475). It binds tighter than the arithmetic and comparison operators (5.6), so `a as Decimal * b` is `(a as Decimal) * b`, and it chains left to right: `n as Int as Decimal` widens, then converts. The target is a type name, with a module alias, type arguments or a refinement where the type has them; a `<` directly after it opens a refinement, so a converted value is compared in parentheses, `(a as Decimal) < b`.

**Resolution is static.** `x as T` names exactly one conversion for the pair (the type of `x`, `T`): a row of the table below for two built-in types, or, for a base type into a refinement of it, the conversion that `validate` spells (6.3). A pair with no conversion is rejected at the `as` (`E-TYP-180`, naming the pair), including a pair that could be reached through a third type: conversions never chain by search, so `s as Decimal` for a `Scalar` is rejected although `Scalar` converts to `Int` and `Int` to `Decimal`. An identity conversion, `x as T` where `x` already has type `T`, says nothing and is rejected too (`E-TYP-181`); so is a refinement to its base or to a wider refinement, which is a subtype already (6.3). Nothing is resolved at run time. For a newtype, `as` uses only the representation rows below; a newtype declares no conversion members. For classes and enums, it resolves to the conversion the source or target declares or explicitly derives (below); a pair nothing declares is `E-TYP-180` like any other. The retired `From[S]` and `to()` spellings remain accepted in v0.1 and use the same declared pairs (X-475, X-551, X-552).

**Total and fallible.** A conversion that cannot lose information is **total** and has type `T`. One that could lose information is **fallible**, has type `Outcome[T]` and fails exactly when it would lose information; it is handled on the line like any outcome (`or fail`, `or recover`, `catch`, `match`; 10.2), and left unhandled it is caught by the outcome rules (an `Outcome` dropped is `E-OBL-001`, one used as a `T` is `E-TYP-040`). Nothing rounds: a conversion that would need a rounding choice is not a conversion but a named function with the choice written, `Decimal.fromFloat(x, scale, mode)` (6.5), so `f as Int` and `f as Decimal` for a `Float` are rejected (`E-TYP-180`) naming it.

**Built-in types convert automatically.** The table is the whole of what converts without a declaration:

| From | To | Kind |
|---|---|---|
| an integer type | an integer type whose range contains its own (`Int8` to `Int`, `UInt8` to `Int16`) | total |
| an integer type | an integer type whose range does not contain its own (`Int` to `Int8`, `Int` to `UInt`, `UInt64` to `Int`) | fallible: `conversion.out_of_range` |
| an integer type | `Decimal` | total |
| `Decimal` | an integer type | fallible: `conversion.not_integral` for a fractional part, `conversion.out_of_range` beyond the type |
| `Int32` and the smaller integer types | `Float` | total: every value is exactly representable |
| `Int`, `Int64`, `UInt`, `UInt64` | `Float` | fallible: `conversion.not_exact` when the value is not exactly representable |
| `Float`, `Float32` | an integer type, `Decimal` | no conversion: rounding is a choice, and `Decimal.fromFloat(x, scale, mode)` writes it |
| the integer types, `Decimal`, `Bool`, `Scalar`, `Float`, `Float32` | `String` | total: the canonical text, what `Displayable` prints (allocates) |
| `String`, `StringView` | an integer type, `Decimal`, `Bool` | fallible: `conversion.parse` for any text but the canonical form, `conversion.out_of_range` for an integer beyond the type |
| `StringView` | `String` | total: an owned copy (allocates) |
| `Bytes`, `BytesView` | `String` | fallible: `conversion.not_utf8` at the first byte that is not UTF-8 (allocates) |
| `String`, `StringView` | `Bytes` | total: the UTF-8 bytes (allocates) |
| `Scalar` | `Int` | total (`Byte` to `Int` is a widening above) |
| `Int` | `Scalar` | fallible: `conversion.out_of_range` for a surrogate or a value above U+10FFFF |
| a base type | a refinement of it (`n as Int<0..100>`) | fallible: `refinement.out_of_range`, the conversion `T.validate(x)` spells (6.3) |
| a base type | a newtype of it (`raw as Pence`) | total when the representation is proved; otherwise fallible: `refinement.out_of_range` (6.9, X-552) |
| a newtype | its representation (`p as Int`) | total; preserves the representation refinement; visible only in its declaring module, otherwise `E-NAM-004` (6.9, X-552) |
| a refinement | its base or a wider refinement | not a conversion: a structural subtype, free (6.3); `E-TYP-181` |

Two names of one type (`Int` and `Int64`, `UInt` and `UInt64`, `Byte` and `UInt8`; 6.2) are the identity. Text parsing consumes the whole input, using ASCII digits only, without trimming whitespace on either side (including a final line feed), digit separators, exponents or literal suffixes (X-690). The canonical text of an integer has no leading zeros, no `+` and no sign on zero; a `Decimal`'s has exactly its scale's fractional digits and no negative zero (6.5); a `Bool`'s is `true` or `false`. The sized integer types have no other operations yet (6.4); their conversions are these rows all the same, since they are widening and narrowing only.

A canonical integer of any digit count beyond the target range fails with `conversion.out_of_range`, never `conversion.parse`. In its payload, `value` is the integer if it fits a signed 128-bit value, otherwise its exact canonical text; no digits are discarded (X-690). A Decimal text whose coefficient or scale exceeds 6.5's limits is canonical for no Decimal and fails with `conversion.parse`. The table provides no text conversion to `Float`, `Float32` or `BigInt`; a refined integer is reached by parsing to its base and then explicitly validating it.

**Failures.** A fallible built-in conversion fails with one of five codes, each with a payload the backends build alike: `conversion.out_of_range` (`value`, `low`, `high`), `conversion.not_integral` (`value`, the Decimal's text), `conversion.not_exact` (`value`), `conversion.parse` (`text`) and `conversion.not_utf8` (`position`, the first byte that is not UTF-8).

**The operand.** `x as T` passes `x` as a call passes an argument to a parameter taken by read loan (5.7, 7.5): a bare ownership-bearing place is loaned for the conversion and the loan ends when it returns, `move x as T` moves the value in, and `copy x as T` converts a copy. A declared conversion whose head writes `owned` takes the value itself, and its use site writes `move x as T` (below). A conversion into a new `String` or `Bytes` allocates and is rejected in `noalloc` code (6.6); a declared conversion contributes the effects its body has, as a call contributes its callee's (11.3).

```fortis
// accept
try func parsePercent(text: StringView) Int<0..100> {
  let n = text as Int or fail                // fallible: the canonical text of an Int, or conversion.parse
  return n as Int<0..100> or fail            // fallible: the refinement, or refinement.out_of_range
}

func label(count: Int8, price: Decimal) String {
  let total = count as Decimal               // total: an integer widens into a Decimal
  return "${count as String} of ${price} is ${total}"
}
```

```fortis
// reject: E-TYP-180, E-TYP-181 (at the marked lines)
import fortis.text { Scalar }

func wrong(f: Float, n: Int, s: Scalar) Int {
  let rounded = f as Int                     // reject: E-TYP-180 — rounding is a choice: Decimal.fromFloat(f, 0, HalfEven)
  let same = n as Int                        // reject: E-TYP-181 — n is already an Int
  let d = s as Decimal                       // reject: E-TYP-180 — no row for the pair: Scalar to Int to Decimal is not searched
  return n
}
```

**Custom types convert by declaration.** Nothing converts a class or an enum value automatically: its type declares the conversions it takes part in as members, and the head is the what, the body the how (X-475, X-551). `convert to T` converts a value of the declaring type into a `T`; `convert from S(s)` converts an `S`, named `s`, into a value of the declaring type. `try` makes the conversion fallible: its body may `fail`, and `x as T` has type `Outcome[T]`, handled on the line like any outcome. The body is a method body: effects, ownership, `try` and obligations are checked as for a method (11.3, 7.9, 10.2), and every `return` gives the target type, which is the declaring type for `convert from`. `self` (or `s`) is a read loan of the source unless the head writes `owned self` / `owned s`; then the conversion consumes the value and the use site writes `move x as T`: a bare ownership-bearing place is `E-OWN-001`, and `copy x as T` converts a copy. A member is visible as a method is: `export convert …` resolves wherever the type is visible, and a member without `export` resolves only inside its own module, so the pair has no conversion elsewhere (`E-TYP-180`).

```fortis
// accept
class Celsius {
  degrees: Float

  export convert to Fahrenheit {
    return Fahrenheit(degrees: self.degrees * 9.0 / 5.0 + 32.0)
  }
  export convert from Fahrenheit(f) {
    return Celsius(degrees: (f.degrees - 32.0) * 5.0 / 9.0)
  }
}

class Fahrenheit {
  degrees: Float
}

class Buffer {
  bytes: Bytes

  export try convert to Text(owned self) {
    return Text(body: self.bytes as String or fail)
  }
}

class Text {
  body: String
}

class Score {
  points: Int

  export try convert to Int8 {
    return self.points as Int8 or fail       // fallible: conversion.out_of_range beyond the type
  }
}

try func showAll(c: Celsius, f: Fahrenheit, points: Int) String {
  let hot = c as Fahrenheit                  // total: the member on Celsius; `c` is loaned for it
  let cold = f as Celsius                    // total: the member `convert from Fahrenheit` on Celsius
  let text = Buffer(bytes: "hi" as Bytes) as Text or fail   // consuming: the temporary is moved in
  let small = Score(points: points) as Int8 or fail         // fallible: handled on the line
  return "${hot.degrees}/${cold.degrees}/${text.body}/${small as String}"
}
```

**Declared conversions from text (X-676, D-421).** A member `convert from String(s)` also supplies the pair from `StringView`, including a string literal; `convert from StringView(s)` also supplies the pair from `String`. These are two exact source pairs of the one member, as for an enum's text derivation, never a search through a third type. A read parameter borrows the text for the call without allocating. If the parameter is `owned String`, a view or literal is copied into a new owned `String` before the call; that copy allocates, contributes `alloc` and leaves the source view usable. An existing `String` still needs `move` or `copy` when the member takes ownership. A `StringView` parameter remains a view of the source, under the ordinary loan and escape rules (7.6). The two text names are their built-in identities: a program's class named `String` gets no extra pair. Declaring members for both text sources duplicates both pairs and is `E-TYP-182`; combining a member with the enum derivation is rejected for the same reason.

**Retired spellings and explicit derivation (X-552).** `From[S]`, `T.from(x)` through that interface, `to()`, `derive From[S]`, `String.from(view)` and `T.validate(x)` remain accepted in v0.1. Their documented replacements are `x as T`, `view as String` and `derive convert from S`. A legacy `From[S]` implementation supplies the same conversion pair as a declared total borrowed `convert from S`; it participates in the same coherence checks. `fortis fmt` resolves each rewritten declaration and call by identity, leaves an unrelated associated function named `from` or `validate` alone, and preserves grouping and evaluation order. Where `to()` has no written expected target it is kept with a note; an inferred generic type argument does not supply a written target. A retired `validate` call still returns an outcome even when its argument is proved; a directly enclosing failure handler is removed only when the formatter proves that this call always succeeds, preserving operand evaluation and comments (X-677, D-509). In an outcome-valued position, when the newtype conversion is total, its rewrite is `Outcome.success(x as T)`, preserving a stored outcome, a match or an explicit handler (X-553). A refinement already proved needs no conversion, so its corresponding rewrite is `Outcome.success(x)`. `String.from(s)` where `s` is already an owned `String` copies through a view loan; its rewrite is `copy s` for a place, since `s as String` would be an identity, or the owned temporary itself. When a copied operand is an arbitrary borrowed expression, the formatter may bind it through a fresh typed inline function, evaluating it once before copying the borrowed place (X-553).

`class Target derive convert from Source` opts into a total borrowed field-wise conversion. Every target field must have a visible source field of the same name, with an equal type or a declared or built-in total borrowed conversion to the target field type, and the source field must be `Copyable`. The fields are copied and converted in target declaration order. A missing or private field, a non-`Copyable` field, a fallible or consuming field conversion, or a non-class source or target makes this field-wise derivation `E-TYP-090`. The enum text derivation below is the sole exception. A fallible field conversion needs an explicit `try convert from` body that handles its outcome. This is the narrowest total reading of the retired `derive From[S]` rule, whose result is `Self`, never an outcome. The generated conversion has the target type's visibility and participates in the one-pair rule; a second declaration of the pair is `E-TYP-182`. No field-wise conversion exists without an explicit derivation or member. Generic classes retain their retired `From[S]` implementations and `derive From[S]` declarations in v0.1; `derive convert from S` has the same allowance (X-555). The written target arguments substitute into the declared source and field types before the pair is matched. The derivation is checked for all its declared arguments, using its ordinary bounds; it does not add a generic explicit `convert` member.

**Enums from text (X-630, X-631).** A non-generic enum with no payload in any variant may declare `derive convert from String`. It generates two fallible borrowed pairs, (`String`, the enum) and (`StringView`, the enum), each returning `Outcome[Enum]`. These are exact pairs, not a search through an intermediate view; they accept read loans of their source and a string literal uses the `StringView` pair. A view returned by `trim()` works like any other view. The conversion matches the whole text against a variant's name, ignoring ASCII case (`A` to `Z` against `a` to `z`) and comparing every other scalar exactly. It does not trim. Failure is `conversion.parse`, retaining its `text` field and adding `variants`, owned text listing the declared variant names in declaration order, separated by `, `. Existing numeric and Boolean parse failures keep their existing payload. The payload owns its text and names, so the outcome keeps no source loan. Two names equal under this ASCII comparison, a payload variant, a generic enum, or any other source type makes the derivation `E-TYP-090`. A class still uses the total field-wise rule and cannot derive this text conversion. Both generated pairs have the enum's visibility and obey the one-pair rule: a hand-written member for either pair is `E-TYP-182`. No reverse conversion is generated; `Displayable` still supplies interpolation. The formatter preserves the explicit derivation.

**One conversion per pair, declared by an owner.** Two conversions declared for one pair — a `convert to T` on the source and a `convert from S` on the target, or two members for one pair — are rejected where they are declared and at an `as` that would have to choose between them (`E-TYP-182`). A member for a pair the built-in table already decides — a row of it, two names of one range, or the identity — duplicates the table and is rejected (`E-TYP-183`): the pair converts without a declaration, or, for the identity, does not convert at all. A pair is keyed by the whole type, as `implement` coherence keys its pairs (9.5): `Box[Int]` and `Box[String]` are two pairs, and two modules' types of one name are two types. Coherence is the member's placement: a conversion lives in the body of the pair's source (`convert to`) or target (`convert from`), so the module that declares it owns one side of the pair, and a pair of two foreign types has no body to hold a member and cannot be declared. A generic class or enum declares no `convert` member (`E-TYP-184`): a pair is a pair of whole types, and an instantiation's member would need the type argument at run time, as an `implement` block for one would (X-180). A newtype declares none either — it has no body to hold one, X-180's exclusion stands, and its conversions are the built-in table's; the other type a member names is a nominal type, never a refinement, since a refinement target stays the table's `validate` row (`E-TYP-184`).

```fortis
// reject: E-TYP-182, E-TYP-183, E-TYP-184 (at the marked lines)
class Celsius {
  degrees: Int

  export convert to Fahrenheit {
    return Fahrenheit(degrees: self.degrees)
  }
  export convert to Celsius {                // reject: E-TYP-183 — the pair is the identity: the table already decides it
    return Celsius(degrees: 0)
  }
}

class Fahrenheit {
  degrees: Int

  export convert from Celsius(c) {           // reject: E-TYP-182 — (Celsius, Fahrenheit) is declared twice
    return Fahrenheit(degrees: c.degrees)
  }
}

class Pair[T] {
  first: T
  second: T

  export convert to Text {                   // reject: E-TYP-184 — a generic class declares no convert member
    return Text(body: "")
  }
}

class Text {
  body: String
}
```

| Rule | Statement | Acceptance evidence |
|---|---|---|
| LANG-06-01 | Nominal identity MUST survive calls, storage, serialization and FFI. (old TYP-001) | Wrong-id compile-fail and decoder tests. |
| LANG-06-02 | Refinement subtyping MUST be structural as in 6.3; the interval domain MUST be implemented exactly as listed, with no additional inference. (old TYP-004) | Accept/reject interval corpus; definitional-interpreter agreement. |
| LANG-06-03 | Untrusted values MUST pass the checked `as` conversion (also spelled `validate` in v0.1) before holding a refined or nominal type. (old TYP-002) | Boundary corpus. |
| LANG-06-04 | Integer, `Decimal` and listed float operations MUST agree bit-exactly across VM and native; a checked operator MUST fail as its checked function fails, at the same operation, on every backend (X-345). (old TYP-003) | Differential numeric corpus; checked-operator corpus. |
| LANG-06-05 | Float operators MUST be total and strict IEEE; float refinements MUST be rejected. | Float corpus. |
| LANG-06-06 | `Map`/`Set` keys MUST implement Hashable and Equatable, and iteration MUST be deterministic per build. | Static checking and repeated-run tests. |
| LANG-06-07 | The trivial and copyable sets MUST be exactly those of 6.11. | Marker-required negative corpus per type. |
| LANG-06-08 | Optionality, `Bool` and `Outcome` MUST NOT convert into one another. (old TYP-006) | Three-outcome suite. |
| LANG-06-09 | Linear values MUST be consumed exactly once. | Drop/overwrite/discard negative corpus. |
| LANG-06-10 | Coherence: an implementation of an interface for a type MUST live in the package that defines one of them; overlaps MUST be rejected. | Orphan/overlap corpus. |
| LANG-06-11 | A call whose type arguments are fixed neither by its arguments nor by its expected type MUST be rejected (`E-TYP-160`). | Uninferred/inferred corpus; checker agreement. |
| LANG-06-12 | In Language 1.0, an associated call on a type parameter (`T.member(...)`) MUST be rejected (`E-TYP-170`), and a call of a method of `T`'s interface bound on a value of `T` MUST run on every backend (X-180). | Rejection corpus; generic-bound execution on three backends. |
| LANG-06-13 | In Language 1.0, an `implement` block's target MUST be a non-generic class or enum the program declares; a core type, a newtype, a generic class or enum or any other type MUST be rejected (`E-TYP-171`), and an accepted block MUST behave as its target listing the interface (X-180). | Rejection corpus; implement-block execution on three backends. |
| LANG-06-14 | `x as T` MUST resolve at compile time to exactly one conversion for the pair, a row of 6.13 or a declared one, else be rejected (`E-TYP-180`); an identity MUST be rejected (`E-TYP-181`); a built-in conversion MUST be total or fail exactly as 6.13 lists, alike on every backend; a newtype unwrap MUST retain its representation refinement and MUST be rejected outside its declaring module (`E-NAM-004`, X-475, X-552). | Conversion corpus; execute fixtures on every backend; checker agreement. |
| LANG-06-15 | A class or enum MUST declare or explicitly derive its conversions, one per pair, keyed by the whole type; the retired `From[S]` declaration remains accepted in v0.1 (X-552): a pair declared twice MUST be rejected (`E-TYP-182`), a member for a pair the built-in table decides MUST be rejected (`E-TYP-183`), and a generic class or enum and a newtype MUST have no `convert` member (`E-TYP-184`); the body MUST be checked as a method body whose return type is the target or the declaring type, a consuming member MUST take `move x as T`, and an accepted member MUST run alike on every backend (X-475, X-551). | Conversion-member corpus; execute fixtures on every backend; checker agreement. |
| LANG-06-16 | The functional methods of `List`, `ListView`, `Map`, `Set`, `T?` and `Outcome[T]` (6.7, 6.8, 10.2) MUST take the callback types their signatures state: a `pure fn` where the method only computes (`E-TYP-040` for a lambda that does not qualify), a `fn mut` for `each`, an outcome-returning function for the `try` methods; a method that returns elements MUST require `Copyable`; and a `pure` function MUST NOT call `each` or `tryEach` (`E-EFF-001`). Every backend MUST run them with the same results and the same order of visits. | Accept and reject fixtures per method; execute fixtures on the interpreter, the VM and native. |

## 7. Ownership

### 7.1 The judgement

Every expression is checked under an environment Γ that records, for each binding: its type, whether it is mutable, whether it currently owns a value (or has been moved out), and the set of active loans on it. The judgement is

`Γ ⊢ e : τ ▷ L, E, O`

where `L` is the set of loans the expression creates, `E` its effect set (section 11) and `O` the linear obligations it leaves open (section 10). A statement transforms Γ to Γ′. The rules below define `L`; the remaining components are defined in their own sections. The checker is a finite dataflow analysis over the function's control-flow graph with the join rules stated in each section; it does not call a solver.

### 7.2 Places and owners

A **place** is a path that denotes storage:

| Place form | Example |
|---|---|
| a binding | `invoice` |
| a field path rooted in a binding or `self` | `invoice.lines`, `self.status` |
| a `List`, `Array` or `FixedList` element with a literal index (X-711) | `grid[3]` |

Nothing else is a place: `list[i]` with a variable index, call results, `a ?? b` and literals are values. A marker (`copy`, `alias`, `move`, `editable`) is applied to a place. Partial moves are not supported: `move invoice.lines` out of a binding that still owns `invoice` is rejected (`E-OWN-040`); move the whole binding or restructure.

A binding **owns** the value it was initialised with until the value is moved out. A `let` binding that owns an ownership-bearing value drops it at block exit unless it has been moved. Parameters are bindings that cannot be reassigned; an `owned` parameter owns its argument, a plain or `editable` parameter holds a loan.

### 7.3 Loans

A **loan** is a read-only (`alias`) or exclusive (`editable`) reference to a place. Loans are lexical:

- A loan created by `let r = alias p` or `let r = editable p` is held by `r` and lasts until the end of the block that declares `r`.
- A loan activated for a synchronous call's argument or receiver ends when the call returns, unless the result carries it (5.7). A loan taken by a comparison, an interpolation or a `capture` ends with the statement; one taken by a `for` header lasts for the whole loop.
- A loan activated for an argument of an `async` call is **held by the future** the call returns, exactly like a `from` result: it lasts until that future completes or is discarded, which is what lets a launched task keep using a borrowed `net` after the launch statement has ended (12.3, 12.5).
- A loan returned from a call with a `from` clause is held by whatever binding receives the result and lasts as that binding's loan. So is a loan the result holds inside an optional, a tuple or a record (`xs.get(0)` is `(alias T from self)?`), whether the binding receives it by `let`, by assignment (then until the end of the assigned binding's block), by `move` of another binding that holds it, inside a tuple or record literal, or through a `??` whose present side lends; a loan of a trivial value is that value and holds nothing (D-348). The kept loan has the mode the result declares, so `editable cursor.next()` leaves a read loan of what the cursor lends, and the cursor may be advanced while the item lives.
- A loan captured by a closure lasts as long as the closure binding.

There is no non-lexical lifetime shortening. To end a loan early, close the block: `{ let r = alias p … }`.

**Loans through loans.** A binding that holds a loan (an `editable` or plain parameter, or a `let r = alias p` / `let r = editable p`) is a *path* to the owner, and every loan is recorded against the owner, not the path. Accessing a place through a loan-holding binding creates a **child loan** of the same owner with these rules:

- A read child of a read parent is a read loan; a read child of an exclusive parent is a read loan that **freezes** the parent (the parent may be read but not written or exclusively re-loaned) until the child ends.
- An exclusive child requires an exclusive parent and **suspends** the parent (no access through the parent at all) until the child ends. A child held by a binding ends with that binding's block; a child created for a synchronous call ends when the call returns unless the result carries it (5.7); a child taken for an `async` call is carried by its future.
- A parent cannot be moved, reassigned or dropped while any child is live; a loan-holding binding cannot be reassigned at all. That is a binding this paragraph names, one that holds a loan whole and is a path to the owner: a parameter, `let r = alias p`, `let r = editable p`, or a binding that receives a `from` result that is itself a loan. A `let mut` binding that receives a loan inside an optional, a tuple or a record (`let mut t = xs.get(0)`) is not a path to the owner, and may be reassigned: the new value's loans replace the old ones at the assignment, and the old value is not destroyed (7.9). After `t = xs.get(1)` the list is lent to `t` until the end of `t`'s block; after `t = null`, `t` lends nothing. A `let mut` binding that holds a standard cursor (`let mut c = alias xs.iterate()`, 5.3) holds a loan-carrying value that the binding owns (7.7), not a path to the owner, and is likewise reassignable: the assignment destroys the old cursor and ends the loan it stored (7.9), while a loan drawn through the old cursor is the origin's and outlives it (7.6); the binding holds the new cursor's loan until the end of the binding's own block (D-401).
- At a control-flow join, a loan is live if it is live on any incoming path. A loop's exit is such a join: it joins the header with its condition false, reached before the body has run or after any iteration (a `continue` included), and every `break`, so a body that ends a loan (`t = null`) ends it on its own path only, and the loan is still live after a `while` or `for` that may run no times or leave by `break` (D-362). A `give` out of an `if` or `match` branch, and a `recover`, carry the loans their value holds to the expression's consumer, which holds them as it would the loans of a call bound directly; a `match` whose scrutinee is a temporary that lends (`match xs.get(0)`) lends to the arm's payload bindings for the arm (D-355).

```fortis
// reject: E-OWN-011 (at the marked line)
class Cell {
  export value: Int
}
func inspect(c: Cell) Int => c.value

func use(c: editable Cell) Void {
  discard inspect(c)         // accept: read child of the exclusive parent, ends when inspect returns
  let r = editable c         // exclusive child held by r: c is suspended until the block ends
  r.value = 1
  discard c.value            // reject: E-OWN-011 — c is suspended while r is live
}
```

**Aliasing invariant.** At any program point, an owner has either exactly one active exclusive loan or any number of active read loans, never both. An exclusive loan of any field path freezes the whole root owner. Read loans of disjoint field paths of one owner may coexist with each other. While any loan is active, the owner may not be moved, dropped, reassigned or (for a read loan) mutated. Violations are `E-OWN-010` (conflicting loan) and `E-OWN-011` (owner used while loaned).

```fortis
// reject: E-OWN-010 (at the marked line)
let mut invoice = Invoice.create(customer: CustomerId(7), currency: Currency.GBP, lines: move lines) or fail
let title = alias invoice.customer        // read loan on field path
let count = alias invoice.lines           // disjoint read loan: fine
editable invoice.issue(Timestamp.epoch()) // reject: E-OWN-010 — exclusive loan while read loans are active
```

```fortis
// accept
let mut invoice = Invoice.create(customer: CustomerId(7), currency: Currency.GBP, lines: move lines) or fail
{
  let items = alias invoice.lines
  discard items.count()
}                                          // loan ends here
editable invoice.issue(Timestamp.epoch())  // accept
```

### 7.4 Moves

`move p` transfers ownership out of the place `p`. After a move the place is uninitialised on that path: reading it, loaning it, moving it again or dropping it is an error (`E-OWN-020 use after move`). At a control-flow join, a place is initialised only if it is initialised on every incoming path. A `let mut` place may be re-initialised by assignment after a move. A condition follows the shape that evaluates it: an `if` condition, an if-expression and a `when` guard are evaluated once, so a `move` in them moves the place once (D-448); a `while` condition is evaluated on every iteration (5.3), and its body returns to it at its normal end or at a `continue`, so a `move` in it is a use after the move on the next evaluation (`E-OWN-020`) unless the body re-assigns the place on every path back. The backend may implement a move as a bit copy; the source rule is unaffected.

### 7.5 Markers and the positions that require them

The markers are place operators: they do not evaluate, they select how the place is used.

| Marker | Meaning | Requires |
|---|---|---|
| `copy p` | independent duplicate | `p: Copyable`, not `noalloc` if it allocates; `copy` of a place whose type is not copyable (6.11), including `Shared` and `Weak`, is rejected (`E-OWN-002`) |
| `alias p` | read loan | any place |
| `editable p` | exclusive loan (held by a binding, or for a call argument) | `p` rooted in a `let mut`, `editable` parameter or exclusive loan |
| `move p` | transfer ownership | `p` owns its value, no active loans |

Which positions need a marker and which take an implicit read loan:

| Position | Rule for an ownership-bearing place |
|---|---|
| `let x = p`, `x = p`, field initialiser `name: p`, `return p` | marker required: `copy`, `alias` (loan) or `move`; `return x` of a local `let` binding is a move without one (below) |
| argument to a plain parameter; receiver of a `self` method | implicit read loan; no marker |
| argument or receiver that the result borrows `from` | `alias p` (or `editable p`) required: the loan outlives the statement |
| receiver of an `editable self` method | `editable p.method(…)`; the marker may be left out on a root that shows it changes (below) |
| receiver of an `owned self` method | `move p.method(…)` required (or a fresh value) |
| call of a `fn mut` / `fn once` closure binding | `editable f(…)` / `move f(…)` required; the `editable` may be left out as for a receiver (below) |
| argument to an `editable` parameter | `editable p` required |
| argument to an `owned` parameter | `move p` or `copy p` required |
| closure capture list | marker required (section 8.3) |
| operands of `==` `!=` `<` … and of interpolation | implicit read loan |
| `match p { … }` | implicit read loan; payload bindings are read loans. `match move p` consumes and binds owned payloads |
| `for x in p` | implicit read loan; `for x in move p` consumes |
| `capture field = p.path` (section 13) | implicit read loan, snapshot copied |

A fresh value (constructor call, function result, literal, arithmetic result) needs no marker anywhere. A trivial value needs no marker anywhere. The left operand of a `??` is read as the position the result goes to reads it: where the result is owned and goes to a position of the first row (`let p: Token? = some ?? move d`, an assignment, a `return`, a `give`, an `owned` argument or field), an existing ownership-bearing place there needs `move` or `copy`, since the result would otherwise be a second owner of its value (D-357); lent for a call, a comparison or an interpolation (`show(some ?? make())`, `"${name ?? "none"}"`), the bare place is the read loan the position takes. `move some ?? move d` moves `some` on every path.

```fortis
// reject: E-OWN-001, E-OWN-020 (at the marked lines)
let user = User(name: copy name, age: 30)
let mut independent = copy user
let seen = alias user
rename(editable independent, "Richard")
enqueue(move independent)
// reject: E-OWN-001 — assignment of an existing ownership-bearing place without a marker
let other = user
// reject: E-OWN-020 — independent was moved
let again = alias independent
```

**What the declaration already shows** (X-345). A marker is written where it tells the reader something the line does not. Two markers say what a declaration the reader can see already says, and may be left out; the checker then checks the unmarked form exactly as the marked one, with the same loans, moves, discarded facts and codes:

- **Receivers and `fn mut` calls.** The receiver of an `editable self` method, and a `fn mut` closure being called, need no `editable` wherever `editable` could be written (the table above): on a place rooted in a `let mut` binding, an `editable` parameter, an `editable` or `move` capture, a binding that holds an exclusive loan (`let r = editable p`, or an `editable … from` result) or, inside a method, its own `editable self`. `readings.push(40)` on `let mut readings` is `editable readings.push(40)`, and `self.order.push(w)` in an `editable self` method is `editable self.order.push(w)`. This holds whatever the receiver's type: a trivial value takes no marker, but its `editable self` call still takes the exclusive loan, so it conflicts with a live read loan (`E-OWN-010`) as the marked call does (D-303). On any other root the call is rejected, marked or not (`E-OWN-001`): a `let` binding, a read parameter, capture or loan, and the `self` of a read method.
- **Returning a local.** `return x`, where `x` is a local `let` or `let mut` binding of the function (or of the lambda whose body it is), moves `x` as `return move x` does: nothing in the function can use `x` afterwards. A local still loaned or already moved is rejected with the codes the marked form gives (`E-OWN-011`, `E-OWN-020`; `E-DEF-002` when a `defer` names it). Returning anything else keeps the table's rule: a parameter (an `owned` one included), a field, a capture, a pattern or `for` binding, any other expression.

```fortis
// accept
export class Tally {
  order: List[String]
  export func add(editable self, word: StringView) {
    self.order.push(String.from(word))       // the method's own editable self
  }
}
func fill(t: editable Tally) {
  t.add("a")                                 // an editable parameter
}
func build() Tally {
  let mut t = Tally(order: List.default())
  t.add("b")                                 // a let mut binding
  fill(editable t)                           // an argument keeps its marker
  return t                                   // a local: the return moves it
}
```

The written marker stays accepted in both positions, and `fortis fmt` removes it. Everything else in the table is unchanged, because each marker says something the line would not otherwise show: `copy` and `alias` in bindings and field initialisers, the markers of a capture list (8.3), `editable` and `move` on arguments, and `move` everywhere but the return of a local.

### 7.6 Parameters, receivers and returns

| Parameter form | Caller passes | Callee holds |
|---|---|---|
| `x: T` (ownership-bearing) | the place, implicitly | a read loan for the call |
| `x: editable T` | `editable p` | an exclusive loan for the call |
| `x: owned T` | `move p`, `copy p`, or a fresh value | ownership |
| `x: T` (trivial) | the value | a copy |

A local `let t = editable s` retains the exclusive permission of the loan, including when `s` is itself an editable parameter or local loan and its record fields are trivial (X-733). The local binding remains non-reassignable. An `alias` loan retains read permission only.

Parameters are never reassignable. `editable` grants permission to mutate the referent through the parameter, not to rebind the parameter. Methods declare their receiver as `self` (read loan), `editable self` (exclusive loan) or `owned self` (consumes). The call site mirrors the mode exactly as for arguments: `invoice.render()`, `editable invoice.issue(now)`, `move invoice.archive()`, except that the `editable` may be left out where the receiver's declaration already shows it may change (7.5, X-345): `invoice.issue(now)` on `let mut invoice`. A marker before a receiver applies to the receiver place, so `editable a.b.issue()` loans `a.b` exclusively.

A function that returns a loan names its origin with `from`:

```fortis
// accept
import acme.accounts { User }
export pure func displayName(user: User) StringView from user {
  return alias user.name
}
```

`from` names exactly one parameter, which must be a plain or `editable` parameter (never `owned`), and the caller's argument for it must be a place, not a temporary. A receiver where the result is `from self` must also be a place if the loan is kept beyond the statement (7.9, X-625) (`fresh().get(0)`, `fresh().first()`, a cursor of a temporary list: the temporary ends with the statement, so a loan drawn from it is `E-OWN-051`; a receiver that is itself a loan, a view or a cursor lends what it lends, which outlives the statement; D-392), written with a marker that follows the **mode of that parameter or receiver**: `alias p` for a read parameter, `editable p` for an exclusive one, even when an exclusive receiver returns only a read loan (`editable cursor.next()` yields `(alias T from self)?`). The marker shows at the call site that a loan is being kept: `let name = displayName(alias user)`, `let mut cursor = alias list.iterate()`, `let item = editable cursor.next()`. This applies whether the result *is* a loan (a loan type or a view) or *carries* one (an owned `[lifetime r]` value, 7.7). The kept loan has the result's declared mode, not the marker's: `editable cursor.next()` leaves `cursor` exclusively loaned only until the call returns, and `item` holds a read loan drawn from the cursor's stored loan. The caller's loan on that argument is then held by the binding that receives the result instead of ending at the statement. The mode of the returned loan is written in the signature: `StringView from user` is a read loan; `editable Cell from c` is an exclusive loan, permitted only when `c` is itself `editable`, and it suspends the caller's `c` as a child loan (7.3) for as long as the result binding lives. A function needing loans from two parameters in its result is rejected (`E-OWN-050`); return an owned value or split the function. A loan of a local or temporary cannot be returned (`E-OWN-051`). Nor can a value of the function's own where the result is declared `from` a parameter: it would be held by the caller as a loan of what ends with the call, and is `E-OWN-051` too (D-373). A `from` result is a loan, so the expression that produces it, by `return`, by an expression body or by a `give` into it, must lend: a place read through a loan (a parameter, `self`, a binding that holds a loan, a field or element path through one), `alias p`, a call whose result is declared a loan, a `??` both of whose sides lend, an `if` or `match` every one of whose results lends, an `or recover` or `catch` whose value side and every `recover` lend (`lookup(alias xs, want) or recover xs.get(0)`; `or fail`, and a handler that fails or returns, produce nothing), `present` of one, parentheses around one, and, where the result is a view, a string literal without interpolation, which is a view of static storage (`StringView from xs => "plain"`; 6.6). Anything else is a value of the function's own: `return move d`, a copy, a fresh value, an interpolated string (`"x${…}"` allocates a fresh `String`; 6.6), a literal where a `String` loan is declared (it converts by allocating), a tuple or record literal (`return (xs.get(0) ?? move d, 1)`, whatever its parts), a constructor, a lambda, a call whose result is owned, an arithmetic or string result, and a `??`, `if`, `match`, `or recover` or `catch` any of whose results is one of these (`return xs.get(0) ?? move d`, `null => move d`, `or recover move d`, `recover move d`), at any depth (D-377, D-381, D-385, D-386). A trivial value holds nothing and is exempt. A binding that owns a part of what it holds beside a loan (7.9: a `??` whose fallback is the function's own, a tuple or record built around one, an `if` or `match` one of whose branches gives one) is the function's own on that path, so returning it, bare, moved, inside `present` or given out of an arm, is `E-OWN-051` too (D-388, D-389). Whichever way the result is produced, a `return`, an expression body or a `give`, the loans it keeps are judged against the declared origin: a loan of a second parameter is `E-OWN-050` (D-395). The same holds for a closure's `from` result, whose origin is a parameter of the closure or an `alias` or `editable` capture, loans the closure holds; a `move` or `copy` capture is the closure's own value, destroyed with the closure (7.9), so a result declared `from` one is `E-OWN-051` (D-379). Bind the loan and return an owned value from a function whose result is owned, `let v = xs.get(0)` then `return copy v ?? move d`, or lend on every path.

```fortis
// accept
import acme.cells { Cell }
func project(c: editable Cell) editable Cell from c {
  return editable c
}

func bump(cell: editable Cell) Void {
  let result = project(editable cell)  // cell is suspended while result lives
  result.value = 1                     // accept
}
```

Returning an existing owned local is `return local`, which moves it (7.5, X-345; `return move local` is the same); returning an `owned` parameter uses `return move param`; returning a field of a loaned object uses `return copy obj.field` or a `from` signature; returning a fresh expression needs no marker.

**A loan is not an owned value.** A loan stays a loan wherever it goes (X-95): a call's result that lends (`xs.get(0)` is `(alias T from self)?`, `m.get(k)` likewise), a binding that holds one (`let v = xs.get(0)`, narrowed by a null test or not), and an `a ?? b` whose result can be one: `a` lends, `b` lends (a nested lent `??`, at any depth), or either is a place read through a loan (a read or `editable` parameter, a binding that holds a loan, `self` in a method that borrows it). Where an owned value is expected (the `return` or expression body of a function whose result is not a loan, a binding or assignment of an owned type, an `owned` argument, a field initialiser, a variant's payload, a `recover` value), a loan of a trivial value is that value; a loan of any other value is made owned only by `copy`, and without it the position is `E-OWN-001` for a copyable value and `E-OWN-002` for one that is not Copyable, which has no owned copy at all (6.11; D-341). `copy v` of a binding that holds a loan copies the value it lends and has the owned type: `T?`, or `T` where a null test narrowed `v`. A call's result is bound first, `let v = xs.get(0)` then `return copy v`, or matched, `present(w) => copy w`; a place is copied where it is read, `return copy s ?? fallback`; a value that cannot be copied stays lent (a read parameter, a `from` result) or is taken out of its owner (`pop`, `move`).

A call whose result carries no loan ends the loans carried by its receiver and arguments (X-714). For example, `a.trim().scalarCount()` returns an `Int` without a loan of `a`; `a.trim()` still returns a view lent from `a`. A scalar expression result, such as the `Bool` from comparing trimmed views, likewise retains no operand loan. Loans of other, enclosing expression operands remain live.

### 7.7 Loan-carrying values; no stored loans

A loan lives in a local binding, a parameter or a returned value that names its origin with `from`; it is never stored in a field. A **loan type** is written `[alias | editable] Type from origin`, where `alias` is the default and the origin is a parameter, the receiver `self` or `static`. The standard library's views (`StringView`, `BytesView`, `ListView[T]`) and cursors (`ListCursor[T]` and the cursor of every standard collection, 5.3) are **loan-carrying types**: compiler-known types whose values store a read loan. A loan-carrying value is **owned** by the binding that holds it (so it may be loaned `editable` like any owned value, and the child-loan rule does not treat its holder as a path to the origin), while the loan it stores keeps its own mode and region. A `let mut` binding of a standard cursor may be reassigned (7.3): the assignment destroys the old cursor and ends the loan it stored (7.9), and the binding holds the new value's loan for the rest of its block (D-401); a view bound by `let v = alias p` is a binding 7.3 names and keeps its rejection (X-496). It is used in exactly three ways: as the type of a local binding, whose region is that of the loan it was created from (`let label = alias user.name`); as a parameter type (`view: StringView`, a loan for the call); and as a return type with `from` (`StringView from user`). Values of a loan-carrying type may not outlive the owner they were created from and may not cross `launchParallel` or `launchBlocking` (cooperative `launch` within the owner's block is permitted, 12.5). Read loans are covariant in their region (a loan `within` a longer region may be used where a shorter region is expected); exclusive loans are invariant.

**Stored loans are not in Language 1.0** (X-137). A class or record may not declare a lifetime parameter, and no field may have a loan type or a loan-carrying type other than a loan `from static` (which borrows from no owner); both are rejected with `E-OWN-053`, naming the field or the parameter. A value that must live as long as something else is owned (`move`), copied (`copy`), held through an explicit `Shared[T]` (7.8), or passed to each call that needs it. A collection of user code that implements `Iterable` returns a standard cursor (`func iterate(self) ListCursor[T] from self => self.items.iterate()`), and a parser that would keep a view of its input keeps offsets into it instead.

A closure that captures `alias` or `editable` carries the origins of those captured places, including captures inferred under 8.3 (X-695; D-645). Substitution, an annotation, a generic call and a branch result preserve every captured origin. An optional, tuple, record or outcome holding that closure carries the same loans. Such a closure cannot be stored in a collection or class field (`E-OWN-053`), and neither it nor a value holding it may escape a captured owner's scope (`E-OWN-051`, or `E-OWN-050` for a different declared origin, as in 7.6). A local holder is permitted while all its owners live. An `owned` callback parameter names its region as 8.3 requires, including a callback inside an aggregate argument; the captured owners must match the argument supplied for that region. Capturing an owned value by `move` or `copy` introduces no loan; moving a closure that already carries a loan preserves that loan.

**Collection storage (X-605, D-580).** A loan-carrying type may not be an element, key or value type of an owned collection (`List`, `Array`, `Deque`, `Map`, `Set`, `SortedSet`, `OrderedMap`, `FixedList`, `FixedMap`, `VariantMap`), or an argument of another type that stores it (`Shared`, and user generic classes and enums included, even when no field uses the parameter). This is a stored loan even when the collection itself is local. The rule follows optional, tuple, record and outcome contents and newtype representations; wrapping a view does not make it owned. A loan explicitly `from static` is exempt, since it borrows from no owner. The source must prove that origin: a static literal or a value already typed `from static`; an annotation, assignment, argument or callable conversion cannot relabel a non-static loan as static (`E-OWN-053`, D-584). The check follows optional and aggregate contents, and every branch that supplies a static result must justify it. Returns retain the expression and region proofs of 7.6. A plain `StringView` is not that exception, even when its initial value is a literal.

The checker reports `E-OWN-053` on the enclosing declaration or annotation of a written storage type, or at the expression or generic call that instantiates an inferred storage type. Generic declarations may use `List[T]`; each instantiation must satisfy this rule, including storage in a generic body whose result does not expose `T`. A generic function used as a value must have its storage arguments fixed by the expected function type; otherwise its storage requirement would be lost. A generic function that only borrows its argument has no such requirement. Inference must not erase an owned argument's loan or a callback's loan result before this judgement. The existing conversion of a loan of a trivial value to that value (7.6), including an optional one, applies first. A generic adapter whose inferred result would carry a non-static loan without a declared origin is also `E-OWN-053`; forwarding it through another call cannot make it owned. An unfixed type argument remains `E-TYP-160` (6.10), so `List.default()` without an expected element type cannot defer inference to a later `push`.

Optionals, tuples, records and outcomes used as local bindings, parameters or `from` results retain the loan rules of 7.3; their contents become stored when placed in a field, collection or other storing type. Function parameter and result types describe calls, not stored values. A closure that carries a non-static capture loan cannot be stored in a collection, whether the capture was written or inferred (8.3). A direct newtype of `ListView[T]`, including a refined representation (6.9), is a local loan wrapper too: declaring or using that nominal view does not store its loan, but placing it in a field or collection does. The standard views and cursors remain permitted as locals, parameters and `from` results, and `xs.get(i)` may still return `(alias T from self)?`. Futures and tasks retain their existing scoped loan rules (12.3, 12.5). Collections of owned values are unaffected; copy a borrowed value or convert text to `String` before inserting it.

```fortis
// reject: E-OWN-053
export class Line {                      // reject: E-OWN-053 — a field may not store a loan
  export number: Int
  export text: StringView
}
```

*Rationale.* Stored loans are the only place in the language where a reader would have to follow a region through a value's type, and nothing in the core library or the acceptance applications needs them: parameters, locals and `from` returns already express every loan the core uses, and the standard views and cursors are compiler-known. Adding stored loans in a later version breaks no program; removing them after a release would.

### 7.8 Shared ownership

`Shared[T]` is a reference-counted owner created by `Shared.create(move value)`. Its operations, all in `fortis.prelude`, are:

| Operation | Type | Meaning |
|---|---|---|
| `Shared.create(move value)` | `Shared[T]` | takes ownership of `value`; one strong handle |
| `s.retain()` | `Shared[T]` | another strong handle (atomic increment); the only way to duplicate a `Shared`, since `copy` of a `Shared` is rejected |
| `s.downgrade()` | `Weak[T]` | a non-owning handle; does not keep the value alive |
| `w.upgrade()` | `Shared[T]?` | a strong handle while at least one strong handle exists, `null` once the last strong handle has dropped; `w.upgrade() ?? fallback` is the ordinary spelling |
| `w.retain()` | `Weak[T]` | another weak handle; `copy` of a `Weak` is rejected like `Shared` |

Reading through a `Shared[T]` is a read loan of the value (`alias s.value` is written as a plain field path: `s.field`, `s.method()`); dropping the last strong handle drops the value. Effects: `retain` and `downgrade` have none, because the reference count is not a Fortis place and no Fortis operation can observe it, so a `pure` function may create and pass handles; `upgrade` carries `nondet`, because its result depends on whether another task has dropped the last strong handle, so a `pure` function cannot call it; dropping a handle has the cleanup effects of `T` when it is the last. `Shared[T]` gives read access only: mutation requires `Mutex[T]` (section 12.7) or an atomic. Neither `Shared` nor `Weak` is linear: dropping either is permitted. Cycles through `Shared` leak by construction and the checker does not detect them; owner/back-reference pairs use `Weak`. `Shared[T]` and `Weak[T]` are `Shareable` exactly when `T` is (12.7), and lower to `Arc<T>` and `Weak<T>` (Appendix C.1).

### 7.9 Destruction

A value is destroyed at exactly one of these events:

| Event | Order |
|---|---|
| block exit | the block's loans end, then defers LIFO, then locals in reverse initialisation order |
| assignment to a `let mut` owner | the new value is evaluated first, then the old value is dropped, then the store happens |
| temporaries | at the end of the full statement, in reverse creation order |
| `break` / `continue` / `return` / `fail` | each exited block in turn, innermost first, using the block-exit order |
| failed construction (a `fail` inside an associated constructor function) | already-initialised locals in reverse order; the partially built object does not exist |
| class value destroyed | the class's own `cleanup` method if declared (section 12.2), then fields in declaration order |
| tuple or record value destroyed | its elements in index order, a record's fields in written order, except the loans it holds |
| closure value destroyed | the captures it owns, its `copy`, `move` and bare ones (8.3), in capture-list order, except those its body moved out; its `alias` and `editable` captures are loans, which end |
| a call that consumes a closure (`move f(…)`, 8.2; the work a task runs) | the closure, when the call returns, after the call's own locals and owned parameters; an `async` closure's call hands it to the future, which destroys it when the body has run or when the future itself is destroyed |
| `Mutex[T]` destroyed | its value (12.7) |

Only an owner destroys (D-338). A binding that holds a loan is a path to the owner (7.3), and its end destroys nothing: the loan ends. That holds for a binding written `alias p` or `editable p`, one that receives a result declared `from` a parameter or the receiver (`xs.get(0)`, `m.get(k)`, a cursor's `next()`, `xs.first()`), a `??` whose present side is such a loan, a pattern binding of a borrowed `match` and the binding of a `for` over a borrowed collection, and for a plain or `editable` parameter and a closure's `alias` or `editable` capture. A field or element reached through a loan is lent too: `xs.get(0)?.inner` is `(alias Inner from xs)?`, and a `let mut` binding that holds a read loan may be rebound but never written through (`g.n = 9` on `let mut g = f ?? Inner(n: 0)` is `E-NAM-002`; D-356). A tuple or a record that holds a loan beside values of its own (`(xs.get(0), make())`) owns only those: its end, and the end of a binding of the loan taken out of it by a pattern, destroys nothing it borrows (D-349). Which of its parts are loans is decided as for a binding, by what each part was given: where a part's type is a loan, a `??` whose fallback was taken, an `if` that gave a value of its own or a binding moved in that owns one gives the tuple or record that value, and it is destroyed with the other parts (D-366). A closure's `move` capture is decided the same way: a binding moved in that holds a loan gives the closure that loan (D-374), and one that owns its value gives it that value, which the closure destroys with the captures it owns, when the consuming call ends or in index order inside a tuple the body moved it into (D-378). So is `match move o`: where `o` holds a loan (`let o = xs.get(0)`, or the `Outcome` of a `try` call whose result is a loan), the arm's bindings hold loans of the owner's parts, and the arm's end destroys nothing (D-394). Assigning to a `let mut` binding that holds a loan replaces the loan and destroys nothing; the binding then holds what it is given, a loan or a value of its own (D-350). A record's field is the same: assigning to it destroys the old value only where the record owned it, and the record then holds what it is given (D-376). A binding that owns such a part beside its loans (`let t = empty.get(0) ?? move d`, `let pair = (empty.get(0) ?? move d, 1)`) is the owner of that part, so a loan drawn from it (`keep = present(t)`, in an outer block) is a loan of that binding and may not outlive it (`E-OWN-011`; D-369). An unmarked read of such a binding or part (`let a = t`, `let a = pair.0`) is a loan of it, never a second owner (D-370). So a class value's `cleanup` runs exactly once, when its last owner destroys it, and never when a loan, an alias or a view of it ends. A value moved out takes the obligation with it: its new owner runs the `cleanup`, once. Where nothing takes it, the statement is its owner: a `move p` or `copy p` lent for a call, bare or as a `??` fallback (`show(empty.get(0) ?? move d)`), and the value a `catch` or `or recover` gives where the statement discards it, are temporaries, destroyed when the statement ends (D-375, D-413). A temporary destroys only what it owns: an `or recover` or a `catch` lent for a call whose try side succeeds with a loan holds the loan, and the statement's end destroys nothing, as a binding's end does; only a replacement the failure moved in or made is destroyed there (D-451). The same rule holds when the whole expression is discarded: the branch actually taken determines whether the statement owns a value, even when the try side has a loan result type. A replacement moved in or made by `or recover` is destroyed exactly once at that statement's end (X-477; D-454). A loan-carrying value (7.7) is owned by its binding, so dropping a cursor or a guard ends that value, never the elements it lends.

Destruction of a linear value is never generated: the obligation rules make it unreachable.

A method borrowing a temporary receiver uses the same full-statement lifetime (X-594). A `??` or recovery
expression that selects a loan, including a static string literal, forwards that loan without creating an
intermediate owner. Where the selected branch creates an owned temporary, a chained call or interpolation may
borrow it until the statement ends; this does not let a stored view outlive that owner.

A view or other loan drawn from an expression that may own a temporary cannot be bound, assigned or returned
past that statement (`E-OWN-051`; X-625, D-562). This includes a fresh call, conversion, interpolation, a `??`
with an owned branch, an `if` or `match` with an owned arm, and an `or recover` or `catch` with an owned value.
The check follows every supplying branch, even when another branch lends and gives the expression a loan type.
Parentheses and further view-producing calls preserve the restriction. Reading the view within the same full
statement is permitted, as is keeping a view when every supplying branch selects static text or a named place
without moving or copying it. Bind the possible owner first: `let fallback = " temp " as String`, then
`let part = (empty.get(0) ?? alias fallback).trim()`. The owner is then `fallback`, which outlives `part`;
no temporary lifetime is extended.

### 7.10 The Rust-expressible subset

Recovery preserves the selected value's ownership through nesting (X-590). A binding or statement temporary
holds either the successful loan or the owned replacement described in 7.9; `catch` and `or recover` follow the
same rule. A `??` whose fallback cannot produce a value, because it leaves the function, preserves the present
loan itself and creates no replacement owner.

The ownership rules are chosen so that every accepted program has a documented lowering to safe Rust. Appendix C lists the lowering shape for each construct. An ownership situation that the frontend can describe but for which no shape exists (for example a loan returned `from` a receiver and stored in a `Shared`) is rejected with `E-OWN-090 unsupported shape` and a pointer to the nearest supported pattern. Widening the subset is a language change with a decision record, never an implementation shortcut. A `rustc` rejection of frontend-accepted code is a compiler defect only within the Rust-checked fragment of Appendix C; everything else is checked by the frontend alone.

| Rule | Statement | Acceptance evidence |
|---|---|---|
| LANG-07-01 | Assigning, returning or capturing an existing ownership-bearing place MUST carry `copy`, `alias` or `move`, except that returning a local `let` binding moves it; `owned` and `editable` arguments MUST carry `move`/`copy` or `editable`; an unmarked `editable self` receiver or `fn mut` call MUST be checked exactly as the marked one, taking the same exclusive loan whatever the receiver's type, and rejected where the marker could not be written (X-345, D-303). (old OWN-001) | Marker corpus per position in 7.5. |
| LANG-07-02 | Parameter bindings MUST never be reassignable. (old OWN-002) | Negative tests. |
| LANG-07-03 | The checker MUST NOT insert clones, reference counts or hidden shared ownership to repair a rejected program. (old OWN-003) | Generated-IR audit. |
| LANG-07-04 | Use after move, conflicting loans, owner use while loaned and escaping loans MUST be rejected statically, including through closures, `defer`, `await` and `scope`. (old OWN-004) | Adversarial corpus; definitional-interpreter agreement. |
| LANG-07-05 | Loans MUST be lexical as in 7.3; the aliasing invariant MUST hold at every program point. | Loan-region corpus. |
| LANG-07-06 | Returned loans MUST name one origin region; two-origin signatures MUST be rejected; a lifetime parameter, loan-carrying field or storage type argument MUST be rejected (`E-OWN-053`, X-137, X-605), except a loan `from static`. (old OWN-005) | Region corpus, stored-loan rejections and Rust lowering fixtures. |
| LANG-07-07 | Destruction MUST follow 7.9 on both backends. (old RES-001 part) | Drop-order execute tests. |
| LANG-07-08 | Shared ownership costs MUST be explicit (`retain`), and `copy` of `Shared` MUST be rejected. (old OWN-006) | Provenance and cost reports. |
| LANG-07-09 | Unsupported ownership shapes MUST be rejected with `E-OWN-090`, never lowered with `unsafe` or erasure. | Shape corpus; `forbid(unsafe_code)` on generated application crates. |

## 8. Functions and closures

### 8.1 Named functions

```fortis
// illustrative
[@aspect(args) …]
[export] [Contract …] [pure] [noalloc] [async] [try] [recovers] func name[TypeParams](params) [ReturnType [editable|from …]] [where …] { body }
[export] [Contract …] [pure] [noalloc] [async] [try] [recovers] func name(params) [ReturnType] => expression
```

Modifiers appear in exactly this order; any other order is a canonicalisation error that the formatter fixes. Named contracts (section 11.5) are modifiers and stand in the modifier position, before the single-word modifiers; aspects (section 11.8) are applied on the lines above the head. `recovers` states that the function discharges a failure without reporting it (10.4); like `try`, it must be written on an exported function whose summary has the effect, so a signature diff shows who swallowed a failure. Every parameter has a type. The success return type follows `)` immediately. A `Void` return type may be left out (X-345): a head that ends at `)`, or at its `where` clause, returns `Void`, exactly as the head with `Void` written, which stays accepted and which `fortis fmt` rewrites. This holds for every function and method head: free and associated functions, methods, expression bodies (`func log(e: Entry) => console.print(e.word)`), interface members, mixin members and requirements, `implement` blocks and `cleanup` (9.4). A function type is a type, written whole, so it keeps its result: `fn(Int) Void` (8.4). A record result is still written after `)`: in `func bounds(n: Int) { low: Int, high: Int } { … }` the first braces are the result type and the second the body. `try` states that the function may fail (its actual return type is `Outcome[ReturnType]`); `async` that it may suspend (callers receive `Future[ReturnType]` or `Future[Outcome[ReturnType]]`). `pure` and `noalloc` are checked claims (section 11). A named contract written in the modifier position expands to its modifiers and capability requirements.

Expression-bodied functions (`=>`) have exactly one expression as their body. Block-bodied functions follow the exit rules of 5.4.

A named function used as an expression denotes a non-capturing `fn` with its declared parameter types, parameter modes, modifiers and result type; an unannotated `let` may infer that type (X-725). This holds for every import form, including a qualified module name. An associated function value substitutes the owner's written type arguments into every parameter and the result: `Array[Int, 2].filled` has parameter `Int` and result `Array[Int, 2]`. Taking the value does not call the function. Existing marked declaration references retain the same checked function signature. The function type retains the declared success result and the `try` and `async` modifiers. Calling the value applies each implied wrapper once, just as a direct call does: `try fn(String) Int` returns `Outcome[Int]`, `async fn() Int` returns `Future[Int]`, and `async try fn() Int` returns `Future[Outcome[Int]]`. The modifiers retain their effects (X-734).

A function value must have a complete signature. A complete expected function type may fix a generic function's parameters or an associated function's unwritten owner arguments (X-726). Where neither written arguments nor the expected function type fixes every parameter, the value is rejected with `E-TYP-040`; call it directly, or wrap that call in a lambda with typed parameters. A generic derived `default` requires written owner type arguments when used as a value. A function or owner with `where` bounds likewise remains a direct call, so the bounds are checked at the call. An instance method, including a method forwarded by a newtype or reached through an interface or type parameter, is called on its receiver, never taken as a value, even with an ownership marker; use a lambda with the required captures instead. A class or newtype name alone is not its constructor function; wrap the constructor call. Enum variant constructors follow 9.7: a payload variant used without a call requires a complete expected function type, including the enum’s type arguments, otherwise it is `E-TYP-040` asking for that type or a lambda (X-727).

### 8.2 Lambdas and closure kinds

```fortis
// illustrative
[pure] [noalloc] [async] [try] [recovers] fn[captures](params) ReturnType { body }
fn(params) ReturnType => expression               // no captures: no brackets
fn(params) => expression                          // an expression body may leave out its return type
fn mut[captures](params) ReturnType { body }
fn once[captures](params) ReturnType { body }
```

The bracket group after `fn`, `fn mut` or `fn once` is the **capture list**; it is written only when it is non-empty. A lambda takes the same modifiers as a named function, in the same order, and must write `async`, `try` and `recovers` when its body has those effects; `pure` and `noalloc` may be written to assert them and are otherwise inferred; the formatter prints no inferred one back and keeps one the author wrote, because the function type the lambda is passed to states what it must be and the checker enforces it there (X-345). Lambdas are not generic. Parameter types may be omitted only when the lambda is passed directly to a parameter of a known function type, in which case they are taken from it. A block body may leave out its return type when it is `Void` (X-580); any other block result is written. As for a function head, parsing tries a written result type before a bare block, so `{ field: Type }` followed by a body remains a record result type (3.2, P3). An expression body (`=>`) may leave it out (X-345): the return type is then the result type of the function type the lambda is passed to or bound to (a parameter, an annotated `let`, a field), when that type names one, and the body is checked against it as against a written type; otherwise, where that result is a type parameter the call has not fixed or there is no expected function type, the return type is the body's type, widened as an unannotated `let` widens its initialiser (5.1): a computed integer interval or a literal's value gives the base type (`fn(x) => x + 1` over `Int<0..9>` returns `Int`).

The kind states how the closure may be invoked:

| Kind | Captures may be | Callable | Lowers to |
|---|---|---|---|
| `fn` | `copy`, `alias`, `move` (used read-only) | any number of times | `Fn` |
| `fn mut` | as `fn`, plus `editable` and `move` captures that are mutated | any number of times; the binding holding it must be `let mut` and a call is written `bump()` or `editable bump()` (7.5, X-345), taking an exclusive loan of it | `FnMut` |
| `fn once` | as `fn mut`, plus `move` captures that are moved out of in the body | exactly once; the call is written `move finish()` and consumes the closure | `FnOnce` |

A lambda whose body needs a stronger kind than declared is rejected (`E-FUN-010`), naming the capture and use. Declaring a stronger kind than needed is permitted but the formatter reports it at the strict lint level. A `move` capture does not by itself make a closure `fn once`; what matters is whether the body moves the captured value out.

A lambda literal written directly as an argument to a non-`owned` parameter of function type may omit both its capture list and kind (X-479, X-580). With neither a list nor `mut` or `once` written, the captures and kind are inferred as in 8.3 before the backend receives the lambda. Parentheses around the literal do not change this rule. A stored, field, returned or `owned` argument lambda, or one with an explicit list or kind, keeps the written-capture rule. The inferred kind must satisfy the parameter's function type (8.4); incompatible kinds are rejected as for a written kind. The formatter prints neither an inferred list nor an inferred kind and preserves every written list and kind.

### 8.3 Capture lists

Each entry names a visible local or parameter with a marker: `copy x`, `alias x`, `editable x`, `move x`; a trivial value may be listed bare (`[threshold]`), which copies it, consistent with trivial values never needing a marker. A name not in the written or inferred list is not accessible in the body (`E-FUN-011`). Capture expressions are evaluated left to right when the lambda expression is evaluated. Module-level `const`s, functions, types and module aliases are not captures: the body names them directly, and an entry that names one is rejected (`E-FUN-011`). Capturing a field path (`alias self.lines`) captures that path only, not the whole owner, and takes the corresponding loan on the root owner for the closure's lifetime. Two `editable` captures of field paths of one owner are rejected (`E-OWN-010`), as are an `editable` capture and any other capture of the same owner.

For the directly loaned literals of 8.2, each free place read in the body is captured `alias`, or bare (copied) when it names a trivial local or parameter. A field path takes a loan of its owner, as a written field-path capture does. A place written through, assigned or passed editable is captured `editable`, and the inferred kind is `fn mut`; otherwise it is `fn`. Repeated uses of one place make one capture, in order of first use, with editing taking precedence over reading. Distinct field paths remain distinct captures, so two editable captures of one owner, or an editable capture alongside another capture of that owner, are rejected by the same rule as written entries. Moving a captured value out is never inferred: write the capture list and `fn once` (`E-FUN-010`). Every loan, authority and region rule applies to an inferred capture exactly as to a written one (X-479, X-580).

A closure holding loans has the region of the shortest loan and cannot escape it. Passing it to a plain callback parameter (`f: fn(Line) Bool`) is an ordinary argument: the callee receives a read loan of the closure for the duration of the call, which is within the region, so `countWhere`, `map`, `filter` and every synchronous callback-taking function accept a loan-holding closure without saying so in their types. A closure is *owned* by exactly three positions: an `owned fn …` parameter, a field, and a return. A loan-holding closure may occupy such a position only when the position's type names its region (`owned f: fn(Line) Bool from threshold` as a parameter, `fn(Line) Bool from threshold` as a return type); it is never a field, because a field never stores a loan (7.7). Loaning it to a plain `fn` parameter for a call is not ownership and needs none of this. A closure holding only `copy` and `move` captures of `Shareable` values is itself `Shareable`.

A closure that captures a linear value by `move` is itself **linear** and must be declared `fn once` (a `fn` or `fn mut` could run zero or many times, so it may not own a linear value, `E-OBL-006`). A linear closure must be called exactly once on every path, or passed to an `owned fn once` parameter whose callee is bound by the same rule. Boxing as `dynamic fn once` preserves linearity.

```fortis
// reject: E-OWN-011 (at the marked lines)
let prefix = String.from("Hello")
let greet = fn[alias prefix](name: StringView) String {
  return "${prefix}, ${name}"
}
let mut counter = 0
let mut bump = fn mut[editable counter]() Void {
  counter = Int.saturatingAdd(counter, 1)
}
editable bump()             // calling a fn mut takes an exclusive loan of `bump`
// reject: E-OWN-011 — counter is loaned exclusively by `bump` until the block ends
let now = counter
```

### 8.4 Function types

A function type names the kind, parameter types (with `editable`/`owned` modes), the return type and modifiers:

```fortis
// illustrative
fn(Int) Bool
pure fn(StringView) Int
fn mut(editable Buffer) Void
try fn once(owned Job) Void
async fn(Request) Response
```

A callable value is assignable to a function type when its kind is at most as demanding (`fn` ≤ `fn mut` ≤ `fn once`), its parameter modes match exactly (`E-TYP-040` at the differing parameter), its modifiers are at least as strong (`pure` satisfies non-`pure`; non-`try` satisfies `try`), and, where the callable is owned by the receiving position (an `owned fn` parameter, a field, a return), its region is at least as long; a callback loaned for a call has no region requirement beyond the call itself (8.3). The callable and the receiving function type must agree on `async` (X-736): a non-async function value is not assignable to `async fn` or `async try fn`, including in bindings, arguments, returns, fields, generic arguments and qualified imports (`E-TYP-040`). Declare the function `async`, or wrap the call in an async lambda. An async value remains unassignable to a non-async type; the `try` rules are unchanged. A literal in that position may infer its captures and kind under 8.2; inference does not relax this assignability judgement (X-580). A lambda parameter with no written type inherits both its type and mode from the expected function type; a written parameter type has read mode unless `editable` or `owned` is written. The same mode comparison applies to named functions and function-typed values. Function types are nominal in their modifiers: an unconstrained callback cannot be passed where `pure fn` is required. A `from` written after a function type's result names the closure's region (`fn(Line) Bool from threshold`); when the result is itself a loan-carrying type, the `from` belongs to the result and the closure's region, if needed, is written on a parenthesised result (`fn() (StringView from s) from s`).

The backend represents each lambda as a concrete environment type; generic callers are monomorphised. Parameter
modes are preserved through a function value as through a named call (X-594): an `owned` parameter receives the
value, including a fresh argument, and an `editable` parameter receives an exclusive loan. A heterogeneous collection of closures needs the explicit type `dynamic fn(…) …`, which is a boxed, owned representation and allocates on creation. Universal boxing of closures is forbidden. A returned collection or generic holder of one closure type retains that concrete environment type; returning it does not introduce `dynamic fn` or boxing (X-698).

| Rule | Statement | Acceptance evidence |
|---|---|---|
| LANG-08-01 | Every function MUST state parameter types and the success return type, which a function or method head MAY leave out when it is `Void` and a lambda as 8.2 allows (inferred parameter types; an expression body's inferred return type) (X-345); modifiers MUST follow the order in 8.1. (old SYN-001) | Grammar corpus; inferred-result fixtures. |
| LANG-08-02 | Every lambda MUST list its captures (no brackets when it has none) and declare its kind, except a directly loaned literal as defined in 8.2, whose captures and kind MUST be inferred under 8.3; a body needing a stronger written kind or an inferred kind incompatible with its parameter MUST be rejected (X-479, X-580). (old FUN-001, FUN-002) | Capture/kind corpus. |
| LANG-08-03 | Names not in the written or inferred capture list MUST be inaccessible in the body (X-580). | Negative corpus. |
| LANG-08-04 | Callable assignability MUST follow 8.4; the backend MUST NOT box closures unless `dynamic fn` is written. (old CMP-004 part) | Codegen inspection. |

## 9. Classes, interfaces, mixins and enums

### 9.1 Classes

```fortis
// illustrative
[export] [trivial] class Name[TypeParams] [derive I1, I2] [implements I3, I4] [with M1, M2] {
  [export] field: Type
  [export] field: Type = constantDefault
  [export] … func method([editable | owned] self, params) Ret { … }
  [export] … func associated(params) Ret { … }
  cleanup(editable self) [Void] { … }
}
```

A class is a nominal product type with fields and methods. There is no inheritance, no virtual dispatch between classes and no runtime member lookup. Fields are private unless exported (section 4.4). Field defaults are constant expressions. Fields, methods, associated functions and constants of one class share one namespace; a name declared twice is rejected (`E-TYP-070`).

### 9.2 Construction

Every class has a **generated constructor** `Name(field: value, …)` taking every field as a named argument (fields with defaults may be omitted); a field may be given by a pun, `Item(sku, wanted)` for `Item(sku: sku, wanted: wanted)` (5.8, X-345). A class that derives `Default` also has `Name.default()`, which takes no arguments (6.10, X-345). It is callable only where every field is visible. Field arguments that are existing ownership-bearing places need a marker exactly as in 7.5; the constructor takes ownership of every argument. All fields are initialised by the constructor; there is no partially constructed object visible to source.

A constructor call may take a **spread**, `...copy p` or `...move p`, where `p` is a place of a class or record type whose fields are visible at the call. It fills every field of the target that the source has with the same name and the same type (or a type with `From` of it), and nothing else; remaining fields are given explicitly after it, and a field given both ways is an error. A target field with no source counterpart, no explicit value and no default is `E-TYP-050`; defaults are applied last, after the spread and the explicit fields. The two forms differ in what happens to the source:

- `...copy p` copies the taken fields (each must be `Copyable`) and leaves `p` untouched.
- `...move p` consumes the whole of `p`: the taken fields move, the fields the target lacks are dropped, which is an error if any of them is linear (`E-OBL-007`), and `p` may not have a `cleanup`. There is no partial move.

Evaluation order is the spread first, then the explicit fields, left to right. This is how a value is projected into a narrower type (a response from a row) without naming each field.

```fortis
// accept
let response = CustomerResponse(...copy customer)
let summary = CustomerSummary(...copy customer, invoiceCount: count)
```

A class whose construction must validate or may fail provides an **associated function**: a function declared inside the class without a `self` parameter, called as `Name.function(…)`. Associated functions are the only path to a value whose fields are private, which is how a `type` newtype's or a class's invariants are enforced against every caller, including decoders and generated code.

### 9.3 Methods

A class may also declare associated constants (`const columns = …`), referenced as `Name.columns`. A method is a function declared inside a class whose first parameter is `self`. The receiver forms are `self` (read loan), `editable self` (exclusive loan) and `owned self` (consumes the value); the mode word precedes `self` just as it precedes a parameter type. Inside a method, fields are accessed as `self.field`. A method is invoked as `value.method(args)` for a read receiver, `editable place.method(args)` for an exclusive receiver and `move place.method(args)` for a consuming receiver (7.5); the `editable` may be left out wherever it could be written: a place rooted in a `let mut` binding, an `editable` parameter, an `editable` or `move` capture, an exclusive loan or the method's own `editable self` (7.5, X-345). A reviewer therefore sees at the call, or in the declaration of the place it names, whether the receiver can change. A method declared `pure` may write through `editable self` (that write is its footprint, section 11.3) but may not perform I/O, read nondeterministic sources, suspend, block or write to anything else.

### 9.4 Cleanup

A class may declare `cleanup(editable self)` (with `Void` written or left out, 8.1), run when a value is destroyed (7.9), before its fields are dropped. `cleanup` cannot be `try` or `async`, cannot be called directly (`E-TYP-040`), and its effects are part of the effect summary of every function that may drop the type. A failing finalisation (`flush`, `commit`) must be an explicit method the caller invokes; `cleanup` can only release.

### 9.5 Interfaces

```fortis
// accept
export interface Renderable requires Displayable {
  pure func render(self) String
  func reset(editable self) Void
  func create(name: String) Self    // associated: no self
  const limit: Int                 // associated constant
}
```

An interface declares method signatures (including modifiers and receiver modes), associated functions and associated constants. There are no associated types in v1; an interface that needs a type parameter takes it as a generic parameter (`Iterable[T, I]`).

An interface may carry a `where` clause constraining its type parameters before its body (`interface Iterable[T, I] where I implements Iterator[T] { … }`), checked at every implementation and use: `class C implements Source[Int]` with `interface Source[T] where T implements Tag` is rejected when `Int` does not implement `Tag` (`E-TYP-080`), and so is the type `dynamic Source[Int]` wherever it is written (`E-TYP-090`, the code an unmet bound has at a call, 6.10). An interface may **require** other interfaces: `interface Hashable requires Equatable { … }`. A type implementing `Hashable` must also implement `Equatable` (declared or derived), or the implementation is rejected naming the missing requirement (`E-TYP-080 incomplete implementation`, also the code for a missing member, an incompatible signature, an associated constant of a type other than the interface declares, and an unmet mixin requirement). A requirement is matched by identity and type arguments: the interface the requiring declaration's own module names, with the arguments the requirement states for the implementation's (`Svc requires Tagged[String]` is not met by an implementation of `Tagged[Int]`, nor by another module's `Tagged`, 4.4). An upcast is available only between dynamic-eligible interfaces (LANG-09-07): if `NamedShape` requires `Shape` and both are eligible, `dynamic NamedShape` is usable where `dynamic Shape` is expected. `Equatable` mentions `Self` beyond the receiver and is not dynamic-eligible, so `dynamic Hashable` to `dynamic Equatable` is not such an example (D-620). Interfaces carry no method bodies: default behaviour on top of an interface is a mixin that `requires` the interface's methods it uses (9.6). An interface with requirements and no members of its own is a **bundle** (`interface Codec requires Encodable, Decodable {}`) and is satisfied by every type that implements everything it requires, without being named, because it adds nothing that coherence would have to disambiguate. An interface with neither members nor requirements is a **marker** (`interface Authority {}`, `interface ServerContext {}`) and is implemented only by explicit declaration; automatic satisfaction never applies to it.

Interfaces are nominal: a type implements one only by `implements`, by `derive`, or by an `implement I for T` block. Having methods with the right names is not an implementation. The exceptions are exactly the memberless ones with requirements or structural rules: `Copyable` and `Shareable` (structural, 6.11 and 12.7) and bundles; markers are nominal. A requirement cycle (`A requires B`, `B requires A`) is rejected (`E-TYP-060`); deriving an interface a type also implements by hand is rejected (`E-MIX-001`); a `dynamic A` where `A requires B` converts to `dynamic B` without allocation. A class or enum implements an interface by listing it in `implements` and providing every member. An implementation may not strengthen a precondition (narrower parameter refinement) or weaken a guarantee (drop `pure` or `noalloc`, add `try`, `async` or `recovers`, widen a return refinement, or drop a contract's clause; 11.2). Callers through the interface see the interface's contract only.

**Coherence.** A package may implement an interface for a type only if the package defines the interface or the type. Two implementations of one interface for one type are rejected. Both coherence violations, an implementation outside the defining packages and an overlap, are `E-TYP-100`. **Language 1.0 limit (X-180):** an `implement I for T` block's target must be a non-generic class or enum the program declares (however the block names it: an import alias names the declaration it resolves to), and is then exactly `T` listing `I` in `implements` with the block's methods, which are reached through the interface (a bound or a `dynamic I`); a block method `T` already has (its own or a mixin's), or that another block for `T` adds in any module of the package, is a member declared twice (`E-TYP-070`); a block for an interface `T` already implements or derives is the overlap `E-TYP-100` alone. A block for a core type (`implement Labelled for Int`), a newtype, an optional, a generic class or enum (`implement Labelled for Pair[Int]`) or any other type is `E-TYP-171`: core types and newtypes have no run-time identity of their own in Language 1.0's backends, so `==`, interpolation, the codec and dispatch through a type parameter could not honour the block, and a block for one instantiation of a generic type would need the backends to tell its instantiations apart; the full type-passing implementation lifts the limit in a later release. A generic implementation (`implement Displayable for Pair[T] where T implements Displayable`, for a `Pair` the package declares) is therefore not in Language 1.0; when a later release admits it, it is declared at module level with `implement Interface for Type { … }` and follows the same rule. Two blocks for one interface and one target type are an overlap whatever their modules; the target type is the declaration with its type arguments, so `Box[Int]` and `Box[String]` are two targets, and a block in one module of a package is seen by every module that imports its interface or its type (D-132).

**Dynamic dispatch.** An interface is not itself a type a value has: a field, variant payload, parameter or result declared with a bare interface name is rejected at its declaration (`E-TYP-040`), naming `dynamic I`, `Shared[dynamic I]` and a type parameter bounded by `I`. `dynamic Interface` is a type whose value is any implementation, dispatched at run time. It is written with an ownership form: `dynamic Interface` (owned, boxed; allocates on creation), `alias dynamic Interface` (loan), `Shared[dynamic Interface]`. An interface is dynamic-eligible only if it has no associated types, no generic methods and no method taking or returning `Self` beyond its receiver (a `Self` parameter, lent or owned, names a second value whose type a `dynamic` cannot know); `dynamic I` of any other interface is rejected wherever the type is written, a declared type, a lambda's or a `let` annotation (`E-TYP-172`), and an associated function or constant does not count, since a `dynamic` value does not reach it. A value converts to `dynamic I` only from a type that implements `I` with the written type arguments, from a type parameter whose bound grants `I` (directly or through an interface that requires it), or from a `dynamic J` where `J` is `I` or requires it; any other conversion is `E-TYP-040`. An `async` method called through `dynamic` allocates its future; such a call is rejected in `noalloc` code.

### 9.6 Mixins

A mixin contributes method implementations to classes that apply it with `with`. A mixin declares what it **requires** as method signatures, never as fields, and may not add fields, constructors or `cleanup`:

```fortis
// accept
export mixin Summarised {
  requires pure func title(self) String
  requires pure func lineCount(self) Int
  export pure func summary(self) String => "${self.title()} (${self.lineCount()} lines)"
}
```

A mixin requires methods only, never an interface (the owner's decision of 29 October 2026, Q6 of v0.1; X-526): `requires Displayable` is not a sentence of the grammar (`E-SYN-000`); a mixin that needs an interface's methods requires those methods. A requirement is a signature: the class's method of that name is compared with it as an implementation is with an interface member (receiver, modifiers and claims, parameters, result; 9.5), or the application is `E-TYP-080`, and a method the class has by derivation or through an interface it lists meets a requirement of that name. The method that meets a requirement is visible to the mixin's bodies whether or not the class exports it, since the requirement names it. This exception grants access only to the mixin declaring the requirement, not to another applied mixin or to an ordinary caller outside the class's module (X-478). A mixin's method may in turn satisfy a member of an interface the class lists. A method declared in the class takes precedence over a mixin method of the same name. Two applied mixins providing the same method name, with no class method of that name, is an error (`E-MIX-001`); the class resolves it by declaring the method itself. Mixin bodies are checked at each application site, as methods of the applying class, and see only the class's exported members (and the methods meeting its requirements), even when the mixin is declared in the same module; a mixin therefore never reads a private field (`E-NAM-004`), and a type error in a mixin body is reported at the body for each class that applies it. This includes an application from another module of the package (X-476): names written in the body resolve in the mixin's declaring module, including that module's private functions, constants and types; `self` denotes the applying class with the member visibility just described. A diagnostic names the mixin's source file and the line containing the offending body text, and identifies the applying class and its module. The interpreter and compiled backends use the same declaring-module scope when running the body (D-485, D-486).

### 9.7 Enums

```fortis
// accept
export enum InvoiceStatus derive Equatable {
  Draft
  Issued(at: Timestamp)
  Paid(at: Timestamp, reference: String)
}
```

Variants are UpperCamel, optionally with named payload fields. A variant is constructed as `InvoiceStatus.Draft` or `InvoiceStatus.Issued(at: now)`. The prefix is omitted inside a `match` over the enum type, and may be omitted in any position whose expected type is already that enum: a `return` in a function returning it, an argument to a parameter of that type, a field or payload initialiser, an annotated `let`, and the right operand of a comparison (`==`, `!=`, `<`, …) whose left operand has that enum type or is that enum optional (X-345). `return Found(customer: move customer)` in a function returning `GetCustomerResponse` is therefore complete, and so is `if severity == Error`. A bare name that also reaches a declaration in scope (a class, an error kind, another type) is ambiguous in such a position, and is `E-TYP-040` asking for the qualified form, so a name never silently changes meaning (X-345); a pattern has no qualified form, and there the scrutinee's variants are read first, as before. Where nothing expects the enum (an unannotated `let`, a generic argument no expected type fixes, a binding of another type, the left operand of a comparison) a bare variant would have no type, and is `E-TYP-040` asking for the qualified form (D-304). The formatter drops the prefix where the bare form means the same. Enums are closed: every `match` must cover every variant. Enums may have methods and implement interfaces. Enum values are trivial when every payload is trivial. A variant name used where a function type is expected denotes its constructor, with the payload fields as positional parameters (`fallback: Unavailable` passes `fn(PublicProblem) GetCustomerResponse`).

Code generated from an external schema (the HTTP kit) produces a *transport* enum with an explicit `Unknown` variant and a fallible conversion to the closed domain enum; closed source enums are never silently extended.

### 9.8 Patterns and `match`

```fortis
// illustrative
match scrutinee {
  Pattern [when guard] => expression
  Pattern [when guard] => { … give value }
  Pattern | Pattern … => expression            // alternatives: no bindings, no guard
}
```

`match` evaluates the scrutinee once, tests arms top to bottom, takes the first matching arm and has no fallthrough. In expression position every arm yields a value of one unified type, using `give` in block arms. As a statement, arms are blocks or `Void` expressions. The patterns are:

| Pattern | Matches |
|---|---|
| `name` | anything; binds `name` |
| `_` | anything; binds nothing |
| literal | equal value (`Equatable`); integers, strings, booleans |
| `a..b`, `a..<b`, `a..`, `..<b` | an integer scrutinee whose value lies in the interval: `..` includes the far bound, `..<` excludes it, a one-sided form runs to the scrutinee type's own limit; bounds fold as a refinement's (6.3); binds nothing; usable in payloads and alternatives |
| `Variant`, `Variant(field: name, …)`, `Variant(name, …)` | enum variant, binding payload fields by name or position |
| `present(name)`, `null` | optional |
| `success(name)`, `failure(name)` | outcome (section 10.4) |
| `is ErrorName`, `is ErrorName(field: name, …)`, either followed by `as name` | a `Failure` of a declared kind, optionally binding the whole failure (section 10.5) |
| `Name(field: p, …)` | a class whose fields are all visible; every field must be named (or `_` for non-linear fields); only with `match move` / `let … = move` (consuming destructure); rejected for classes with `cleanup` |
| `(p1, p2)` | tuple |
| `{ field: p, … }` | anonymous record |
| `otherwise` | catch-all; permitted only when the scrutinee type is open (integers, strings, `Failure`), not for closed enums or optionals. A top-level `_` or bare binding pattern is likewise rejected as the last arm of a match over a closed enum (`E-MAT-002`), so adding a variant always surfaces every match that must handle it |

Patterns nest. An arm may list **alternatives**, `A | B | C => value`, which matches when any alternative does, tried left to right (X-345). `|` separates whole patterns of an arm, never parts of a payload or of another pattern. The form is narrow, so the arm body sees the same bindings, none, whichever alternative matched: an alternative binds no names (no bare name, no named or positional payload binding, no `as`), an alternative is not `otherwise`, and an arm with alternatives takes no `when` guard; each is `E-MAT-003`. Exhaustiveness counts each alternative as covering its case, as an arm of its own would: a variant of a closed enum that no alternative names is still reported (`E-MAT-001`), and a `_` alternative makes the arm a catch-all (`E-MAT-002` over a closed enum). There is no unreachable-arm check: as an arm repeating an earlier unguarded arm's case is accepted and never runs, so is an alternative that repeats a case. A **range pattern** (X-517) matches an integer scrutinee — `Int` or a sized integer type, refined or not — whose value lies in its interval: `a..b` includes `b`, `a..<b` excludes it, and the one-sided forms `a..` and `..<b` run to the limits of the scrutinee's own type. Each bound is a constant expression folding as a refinement's bound folds (6.3, X-512): an empty or inverted interval (`5..1`, `5..<5`), a bound that does not fold, or one outside the scrutinee type's own range MUST be rejected (`E-MAT-005`). A range pattern over a scrutinee of any other type is `E-TYP-040`: a refinement alias or a newtype (`type Score = Int<0..100>`) is not an integer scrutinee — match on the refined integer type itself or on a binding `validate` produced. A range pattern binds no names, nests in payloads and alternatives like any pattern, and is refutable in a consuming destructure `let` as any pattern is (`E-MAT-004`, X-522). Over a refined integer scrutinee, exhaustiveness counts intervals (X-518): the unguarded arms whose intervals together cover the scrutinee's whole interval make the match exhaustive without `otherwise` (which remains permitted as the catch-all), the intervals left uncovered are named in the record (`E-MAT-001`), and guarded arms and literal arms do not count toward the coverage; an unrefined integer scrutinee keeps its catch-all requirement. The pattern establishes the scrutinee's interval before the `when` guard runs (X-713). An editable loan or write in the guard invalidates facts about the place and its owner by the ordinary rules (6.3); the body sees only the facts that survive the guard. A separate integer copy keeps its own facts. Inside an unguarded arm the scrutinee is narrowed to the arm's interval, as a `when` guard narrows (6.3), and a later unguarded arm sees the intervals the earlier unguarded arms covered removed from its scrutinee's known interval. Binding-with-subpattern (`x @ p`) is not in v1. A `when` guard is a `Bool` expression over the bindings; a guarded arm does not count toward exhaustiveness. Missing variants of a closed enum are reported individually (`E-MAT-001`).

A consuming destructure of a class is the way to take a linear field out of a linear class: `let Pending(result: r) = move p` or `match move p { Pending(result: r) => … }`; the whole value is consumed, every field is bound, and non-linear fields may be discarded with `_`. There is no partial move.

Ownership: `match p` takes a read loan of the place for the match, and payload bindings are read loans. `match move p` consumes the place, and payload bindings own their values. `match` of a fresh value (a call result) owns it. Binding a linear payload (an `Outcome` inside an enum) requires the owning form and the arm must consume it.

| Rule | Statement | Acceptance evidence |
|---|---|---|
| LANG-09-01 | Inheritance, runtime member lookup and reflection-based construction MUST be absent. (old OO-001) | Grammar and negative corpus. |
| LANG-09-02 | The generated constructor MUST be callable only where every field is visible; private fields MUST be settable only inside the module. | Visibility corpus. |
| LANG-09-03 | Implementations MUST NOT strengthen preconditions or weaken guarantees; coherence MUST hold. | Substitution corpus. |
| LANG-09-04 | Mixins MUST require methods only and MUST NOT add state; a requirement MUST be met by a method of a compatible signature, and a mixin body MUST be checked at its application against the class's exported members; conflicts MUST be rejected. | Mixin corpus. |
| LANG-09-05 | `match` on a closed type MUST be exhaustive without `otherwise`, with each missing variant named; the alternatives of an or-pattern MUST each count as covering their case, MUST bind no names and MUST NOT be `otherwise`, and an arm with alternatives MUST NOT take a guard (X-345). (old ERR-006) | Pattern corpus; or-pattern corpus. |
| LANG-09-06 | Pattern binding ownership MUST follow 9.8. | Owning/borrowing match corpus. |
| LANG-09-07 | `dynamic I` MUST name a dynamic-eligible interface (`E-TYP-172`), and a value MUST convert to `dynamic I` only from a type that implements `I`, a type parameter whose bound grants `I`, or a `dynamic J` where `J` requires `I` (`E-TYP-040`). | Eligibility and conversion corpus; checker agreement. |

## 10. Outcomes and failures

### 10.1 Fallible functions

`try func f(…) T` returns `Outcome[T]`: either `success(value)` or `failure(failure)`. These two words are pattern forms; a test or adapter that needs to build an outcome by hand uses `Outcome.success(v)` and `Outcome.failure(move f)`. Inside its body `return v` produces success and `fail f` produces failure. A call to a `try` function needs no prefix; the result is an `Outcome[T]` value that is **linear** and must be consumed by one of the operations below before the end of the enclosing block. The success value is never accessible without consuming the outcome.

```fortis
// reject: E-OBL-001, E-TYP-040 (at the marked lines)
import acme.accounts { CustomerId, loadAccount, save }
func a(id: CustomerId) Void {
  loadAccount(id)                        // reject: E-OBL-001 — outcome dropped at end of block
}

func b(id: CustomerId) Void {
  save(loadAccount(id))                  // reject: E-TYP-040 — save expects Account, got Outcome[Account]
}

try func c(id: CustomerId) Void {
  let account = loadAccount(id) or fail  // accept
  save(account)
}
```

### 10.2 Consuming operations

| Operation | Effect on the obligation | Available in |
|---|---|---|
| `e or fail` | propagates the failure from the enclosing `try` function | `try` functions |
| `e or recover v` | discharges the failure, yields `v` (lazily evaluated); adds the `recovers` effect | anywhere |
| `e catch name { … }` | binds the failure as `name` inside the block, which must end in `recover v`, `return`, `fail`, or a `Never` call; see 10.4 | anywhere |
| `match e { success(x) => …, failure(f) => … }` | the `failure` arm must consume `f` | anywhere |
| `report e` (where `e: Failure`) | consumes; `report e as receipt` also binds `Report`, an inert receipt | functions with the `io` effect |
| `move`/pass to an `owned Outcome[T]` or `owned Failure` parameter | transfers | anywhere |
| store into a linear container or field | transfers | anywhere |
| `fail e` (where `e: Failure`) | propagates | `try` functions |

Bare `or value` does not exist: every discard of a failure is spelled `recover`. There is no `?`, no `try` at call sites and no `unwrap`.

**`map` on an outcome (X-479).** An `Outcome[T]` has one method, `map[U](owned self, f: pure fn(T) U) Outcome[U] where U: linear`, which consumes it: a success becomes the success of `f` applied to its value (read loan), and a failure passes through unchanged with `f` not called. The result is an outcome like any other and is handled on the line: `parse(text).map(fn(n) => n * 2) or recover -1`. The receiver of `map` is a `try` call's result or a place written `move result.map(f)`, since the method takes `owned self`. It is the only helper: `or fail`, `or recover`, `catch` and `match` are the ways to handle a failure and `annotate` is the way to add context (10.6), and a `map` over the failure, an `andThen` or an `unwrapOr` would each be a second spelling of one of them. The collection methods `tryEach`, `tryMap` and `tryFold` (6.7) are `try` functions of this kind: each returns an outcome that is handled on the line.

### 10.3 Failure values

`Failure` is the single failure type. It carries a stable `code` (`StringView from static`, dotted, declared by an `error` declaration), a typed payload, an origin (module, function, line) and an optional cause chain. `Failure` is linear and not copyable. A failure created by `fail ErrorName(…)` in a `pure` function is deterministic: it reads no clock and allocates no identifier. The bounded representation (payload up to 64 bytes inline) satisfies `noalloc`; a payload that cannot be bounded makes the failure allocate and the function non-`noalloc`.

```fortis
// accept
import fortis.money { Currency }
export error CurrencyMismatch {
  code: "billing.invoice.currency_mismatch"
  payload { expected: Currency, found: Currency }
  message: "Line currency ${found} does not match invoice currency ${expected}"
}
```

An `error` declaration defines a failure kind: a constructor `CurrencyMismatch(expected: …, found: …)` producing a `Failure`, a pattern `is CurrencyMismatch(…)`, and a display template. The `message` string is a template, not an interpolated expression: `${name}` in it may name only payload fields (any other name is `E-NAM-001`), and is rendered when the failure is displayed. The template is kept as written between its quotes, and rendering replaces each `${name}` with that payload field's text, as a record's `payload` shows it, on every backend (D-340). Codes are unique per package and stable across versions. The payload fields are trivial or `String`/`Bytes` with a declared length bound, so that failures are bounded and serialisable without secrets.

### 10.4 `catch` and `recover`

```fortis
// accept
let account = loadAccount(id) catch e {
  report e
  recover Account.guest()
}
```

`catch` binds the failure to an explicit name. The block runs only on failure. Inside it, `return` and `fail` leave the enclosing function, `recover v` ends the catch with `v` as the expression's value, and `give` is not permitted. If the block ends without having consumed `e` (by `report`, `fail`, `move` or `match`), `recover` consumes it and adds the `recovers` effect; if `e` was consumed earlier, `recover` adds nothing. Every path through the block must end in `recover`, `return`, `fail` or a `Never` call (`E-OBL-002`). A `catch` never covers an operation other than its left operand.

### 10.5 Matching on failures

```fortis
// accept
let maybe: Invoice? = match Invoice.create(customer: id, currency: gbp, lines: move lines) {
  success(invoice) => move invoice
  failure(f) => match move f {
    is EmptyInvoice => null                                  // discharged here: adds `recovers`
    is CurrencyMismatch(expected: want, found: got) as mismatch => {
      report mismatch                                             // consumed by the arm
      give null
    }
    otherwise(other) => {
      report other
      give null
    }
  }
}
```

A `match move f` over a `Failure` uses `is ErrorName(…)` patterns. Payload fields are bound by copy (they are trivial or bounded strings). An `is` arm may bind the whole failure with `as name`; the arm must then consume `name`. An `is` arm without `as` discharges the failure when it matches, which adds the `recovers` effect to the function. Because `Failure` is an open type, the match must end with `otherwise(name)`, which receives ownership of any other failure and must consume it. Matching a failure through a read loan (`match f` without `move`) is permitted for inspection only: payloads are copied, no arm may consume, and `f` must still be consumed after the statement.

### 10.6 Reports, annotation and aggregation

`report f` is a statement: it consumes the failure and emits it to the task's failure reporter. `report f as receipt` additionally binds a `Report`, a trivial receipt that cannot be converted back to a `Failure`, for use in a public response. Reporting and then propagating the same failure is impossible because the value is gone. The diagnostic operations are ordinary functions in `fortis.prelude`: `annotate(f: owned Failure, note: StringView) Failure`, `Failures.aggregate(list: owned List[Failure]) Failure`, and `report` (a statement). `annotate(move f, note)` returns a new `Failure` with the same code and payload, the original as its cause and the given note; it is the only way to add context, and it keeps exactly one obligation alive. `Failures.aggregate(move list)` consumes a `List[Failure]` and yields one `Failure` with code `failure.aggregate` (a dotted code, as 10.3 requires) and the inputs as causes; it is the rule for parallel work and retry loops. Metrics, span status and debug logging take a read loan of a failure and consume nothing.

`report` has the `io` effect but is the one `io` operation that needs no capability value: it writes only to the diagnostic plane of the current task, which cannot feed anything back into the program (section 11.7). A `pure` function therefore cannot report; it propagates or recovers deterministically.

The canonical boundary pattern, in a server handler of the HTTP kit, is:

```fortis
// accept
import acme.customers { CustomerId, CustomerRepo, GetCustomerResponse }
import fortis.http.server { PublicProblem }
export func show(id: CustomerId, repo: CustomerRepo) GetCustomerResponse {
  return match repo.find(id) {
    success(present(customer)) => GetCustomerResponse.Found(customer: customer.to())
    success(null) => GetCustomerResponse.NotFound
    failure(f) => {
      report annotate(move f, "GetCustomer") as receipt
      give GetCustomerResponse.Unavailable(problem: PublicProblem.from(receipt))
    }
  }
}
```

The failure is reported once, the public response carries only the receipt's correlation identifier, and the internal failure cannot leak into the response because `PublicProblem.from` accepts a `Report`, not a `Failure`.

### 10.7 Linear containers and generic code

A collection of outcomes (`List[Outcome[T]]`) is linear. The standard library declares, in each operation's signature, what happens to linear elements: `push(move item)` transfers in; `pop()` returns `T?` transferring out; `consumeEach(owned self, fn mut(owned T) Void)` consumes the collection and calls the repeatable callback once per element, in order, running to completion (there is no early exit, so no element is left unvisited; an empty collection is consumed with no calls); `clear()` and dropping are rejected for linear element types; `count()` and `isEmpty()` take a read loan. `Outcome.collect(move List[Outcome[T]])` is `Outcome[List[T]]` and consumes all inputs. A generic function may only hold values of a type parameter `T` in linear positions if it declares `T: linear`, and must then consume them. A user generic type follows the same declaration rule for a linear type argument (6.12): without `T: linear` its instantiation is `E-OBL-003`; with that bound its owned contents must be consumed, and dropping the linear instance is `E-OBL-001`. Inference cannot bypass either rule.

The consuming-loop rule of 5.3 and `consumeEach` also apply after unpacking a bounded generic class. An owned linear callback parameter carries the same obligation as a loop item; returning from the callback does not leave the traversal, but must consume that parameter. The repeatable, synchronous, non-failing `consumeEach` callback cannot propagate a failure or suspend the traversal.

The `List` methods `each`, `tryEach`, `map`, `tryMap`, `fold`, `tryFold`, `filter`, `find`, `any` and `all` take read `self`, including when the caller owns the list. Their callbacks borrow each item and leave its obligation in that list. A `move` marker does not change these signatures; a fresh linear receiver, or a fresh linear iterable used by reading iteration, would be dropped after the loan and is `E-OBL-001`. Bind it, then consume it after the borrowing operation. `filter` requires `Copyable` elements, so it cannot filter linear elements. The unbounded accumulator parameter of `fold` and `tryFold` rejects a linear accumulator (`E-OBL-003`). The by-value result of a generic function and the owned result it obtains from a callback require the same `T: linear` bound as an owned parameter (6.12); wrapping a call cannot remove this requirement. The result parameter `U` of `List.tryMap` and `ListView.tryMap` must be non-linear, since a later failure drops earlier results. Similarly, `Outcome.map` and `Outcome.collect` require non-linear payload `T`: they can drop a success payload without consuming it. These signature restrictions are `E-OBL-003`; handle every linear payload in a consuming loop instead. A non-failing `List.map` may produce linear results: the returned list owns them all (X-621).

### 10.8 Defects

A **defect** is a condition that the checker cannot represent as an `Outcome` because the program's assumptions are already broken. The taxonomy is closed:

| Defect | Source | Native behaviour | VM behaviour |
|---|---|---|---|
| contract violation in trusted code | a `trusted` adapter breaks its declared contract | process abort (`panic = abort`) | terminate the task with a `TaskDefect`, keep the process |
| stack exhaustion | unbounded recursion | abort | task terminated |
| out of memory | allocator failure | abort | task terminated |
| runtime invariant | VM or runtime bug | abort | task terminated |

Ordinary source cannot cause a defect: there is no `unwrap`, no unchecked index, no assertion that aborts; the conformance corpus injects each class through the reference toolchain's `injectDefect(kind)` hook, which is not part of the language. A defect is never a `Failure`, is never caught by `catch`, and produces no completion value; `TaskCompletion.Defect` (section 12.6) is observed only by the supervisor of the terminated task. Expected conditions (bounds, parsing, I/O, arithmetic) are always outcomes.

| Rule | Statement | Acceptance evidence |
|---|---|---|
| LANG-10-01 | Outcomes MUST remain wrapped until consumed by an operation in 10.2. (old ERR-001) | Nested-call and access corpus. |
| LANG-10-02 | Linear obligations MUST survive storage in collections, fields and closures; dropping one MUST be rejected. (old ERR-002) | Container/closure/drop corpus. |
| LANG-10-03 | A reported failure MUST be unreachable afterwards; `annotate` MUST keep exactly one obligation. (old ERR-003, ERR-004) | Lineage corpus. |
| LANG-10-04 | `noalloc try` functions MUST use the bounded failure representation on success and failure paths. (old ERR-005) | Allocator probes. |
| LANG-10-05 | `recover` without a prior `report` MUST add the `recovers` effect. | Effect summary tests. |
| LANG-10-06 | Defects MUST follow the taxonomy of 10.8 and MUST NOT be catchable. | Fault injection per backend. |

## 11. Effects and contracts

### 11.1 The effect set

Every function has an **effect summary**: a subset of the closed set below, computed from its body and its callees. The set cannot be extended by packages or configuration. A body whose summary exceeds what the function's modifiers admit (`recovers` without the modifier, an allocation in `noalloc`, an effect in `pure`, a suspension without `async`) is rejected with `E-EFF-001`, naming the effect and the statement that introduces it.

| Effect | Present when the function may… |
|---|---|
| `alloc` | allocate heap memory (including inside callees, failures, cleanup and closures it creates) |
| `io` | perform input/output through a capability, including `report` |
| `mutate` | write to storage that is neither fresh in this activation nor reachable from one of the function's own `editable` parameters (see the write-footprint rule in 11.3) |
| `nondet` | read a non-deterministic source (clock, random, environment, address identity) |
| `suspend` | reach an `await` |
| `block` | block the calling thread (synchronous lock wait, blocking I/O); forbidden in `async` functions and in anything they call (`E-EFF-020`), because the cooperative executor has one thread. Blocking work goes to `launchBlocking`, whose callback is invoked by the pool, not by the async function, so its `block` is not imported (11.3) |
| `fail` | return a failure |
| `recovers` | discharge a failure that was not reported (10.4) |

Mutation of **fresh** storage is not `mutate`: storage is fresh in an activation if that activation (or a callee during it) created it and it has not yet been stored anywhere reachable from a parameter, capture or `self`. Building a list locally and returning it is therefore pure. Writes through a function's own `editable` parameters are recorded in its write footprint rather than as `mutate`; callers instantiate the footprint (11.3).

### 11.2 Modifiers as claims

| Modifier | Claim (checked) |
|---|---|
| `pure` | `io`, `mutate`, `nondet`, `suspend`, `block` are all absent |
| `noalloc` | `alloc` is absent |
| `async` | `suspend` may be present (and is permitted) |
| `try` | `fail` may be present (and is permitted) |
| `recovers` | `recovers` may be present (and is permitted) |

A function without `async` is rejected if its summary contains `suspend`; without `try`, if it contains `fail`; without `recovers`, if it contains `recovers`. `pure` and `noalloc` are independent: a `pure` function may allocate, a `noalloc` function may do I/O. `recovers` is written as a modifier on any function whose summary has the effect (8.1); a contract may forbid it. Modifiers on interface methods bind every implementation, whether written or carried by a contract (11.5): the implementation claims each modifier the member's claims expand to, rules out each effect its contracts forbid and excludes each capability they exclude, written or through a contract of its own, or the implementing type is rejected (`E-TYP-080`). Its body is then checked against those claims, so a caller through an interface or `dynamic` relies on the interface's claims, and sees only those.

There is no `nostate`. The toolchain offers an opt-in lint `no-local-mutation` for teams that want to forbid `let mut` in selected modules; it is not part of the language and has no effect on typing.

### 11.3 Transitive analysis

A function's summary is computed by expression-level transfer rules, then closed under recursion by a least fixed point over the finite set:

- **Calls.** A call imports the callee's summary at the callee's actual type arguments and declared callback types, with two adjustments. `fail` is imported only if the call's outcome leaves the function unhandled: `or fail` and `fail f` keep it; `or recover`, `catch` (ending in `recover`) and a `match` that consumes the failure remove it, adding `recovers` where 10.4 says so. `mutate` is imported per the callee's **write footprint** (below).
- **Async calls.** Calling an `async` function constructs a future and imports only the effects of evaluating the arguments. The future's type carries the callee's summary; `await` imports it, as does `launch` into the awaiting scope. `launchParallel` and `launchBlocking` import the work's summary minus `block`, since the pool, not the async function, runs it.
- **Write footprints.** A function's summary records which of its `editable` parameters (and `editable self`) it writes through, directly or via callees: its write footprint. A write through an `editable` parameter is not `mutate` inside the function itself. At a call site, each footprint parameter is instantiated with the argument place: if that place is fresh in the caller, nothing is imported; if it is reachable from one of the caller's own `editable` parameters, it joins the caller's footprint; otherwise (a mutable capture, a `Mutex` guard, an atomic) the caller gets `mutate`.
- **Fresh storage.** A `let mut` local initialised from a constructor, a literal or an *owned* call result (a value, not a loan type) is fresh for the activation; writes to it, including through methods with `editable self` called on it, are footprint-free. A binding holding a loan result (`let mut r = project(editable c)`) is a path to the loan's owner, and writes through it are attributed to that owner. Returning fresh storage gives the result "fresh" provenance for the caller.

```fortis
// accept
pure try func increment(x: Int) Int => Int.add(x, 1) or fail

pure recovers func fallback(x: Int) Int => increment(x) or recover 0   // accept: fail removed, recovers added

class Cell {
  export value: Int
  pure func set(editable self, value: Int) Void { self.value = value }   // footprint: {self}
}

pure func build() Cell {
  let mut cell = Cell(value: 0)
  editable cell.set(1)          // accept: cell is fresh, so the footprint imports nothing
  return move cell
}
```

`Hashable.hash(self, into: editable Hasher)` and `Displayable.display(self, into: editable text.Builder)` are `pure` with footprint `{into}` under the same rule. Callback parameters contribute the effects permitted by their function type, not those of any particular argument, and that holds for the standard library's operations as for a function of the program: a parameter that is not `pure` makes the call carry `io`, `mutate` and `nondet` as possible effects. Every callback type without `noalloc`, including `pure fn`, also contributes possible `alloc` (X-610). Thus `each` (6.7) and `consumeEach` (10.7) prevent a caller from claiming `pure`; `map`, `fold`, `any` and other operations with `pure fn` callbacks prevent a caller from claiming `noalloc`. Only a callback parameter declared `pure noalloc fn` excludes both sets. v0.1 has no effect polymorphism over callbacks: a particular argument with fewer effects does not narrow the signature. A `noalloc` caller rejected with `E-EFF-001` at the call writes a `for` loop, whose body has its own inferred effects. A lambda literal's own type carries `pure` when its inferred summary has none of the effects `pure` excludes, so it may be passed where a `pure fn` is required without writing the modifier. `cleanup` methods contribute to every function that may drop their type: one that owns a value of it as an `owned` parameter or `owned self`, binds one it owns, destroys a temporary of it (an argument lent for a call, a temporary receiver, a moved aggregate) or discards one; a binding that holds a loan destroys nothing (7.9) and is charged nothing (D-393, D-400). The authority a `cleanup` acts under is one its class holds in a field (11.6), which an `owned` parameter of the class carries for the function as a receiver does (X-495). Trusted intrinsics in the `fortis` package are declared with the `trusted` modifier before the other modifiers (`trusted pure noalloc func …`), have no Fortis body (the backend supplies one), and carry their review record in the documentation comment above them. Only the packages of the published compatibility set (Platform 1.5: the standard library and its audited first-party adapters) may declare `trusted`; a program's own `trusted` function or method is rejected (`E-EFF-001`), so no declaration can bypass the effect analysis. A trusted operation's summary is its modifiers' effects (`fail`, `suspend`, `recovers`), `io` when it is a method of an `Authority` type or takes an `Authority` parameter (11.6), and the effects its signature cannot show (`alloc`, `block`, `io`, `mutate`, `nondet`), which its standard package declares as data shipped with it (`reference/library/effects/<package>.json`), reviewed with its trusted operations and read by every checker. A trusted claim is reported as `trusted` evidence, never as `proved`.

A function may declare a claim that its summary satisfies; it may not declare a weaker set of effects than it has. Every function must write `async`, `try` and `recovers` when its summary has the effect, because each changes the function's type. `pure` and `noalloc` are claims a function *may* make: a caller, generic bound or contract can rely on them only when they are written, so leaving one off never breaks a caller, it only withholds a guarantee. Private callers inside the module see the inferred summary; across a module boundary only the written claims count. Removing a written `pure`/`noalloc`, or adding `async`/`try`/`recovers`, on an exported function is a public API change, which `fortis check --api-baseline OLD` reports as the warning `E-EFF-040` at the declaration (a contract's modifiers count as written; X-130); the strict lint level reports an exported function whose inferred summary would support a claim it does not make.

### 11.4 `noalloc` and plans

`noalloc` is a property of a function **under a target plan and a telemetry plan**. A library function declared `noalloc` claims: no allocation given the bounded instrumentation plan it was checked with. The build checks the claim again against the application's actual plan; if the application's telemetry policy attaches an allocating record to the function, the build is rejected at the policy application site (`E-EFF-030`), not by silently removing the instrumentation or by failing the library. For an `async` function, `noalloc` covers four phases: future construction, each poll, completion and destruction. The checker decides allocation by matching lowering patterns against the whitelist in Appendix C, not by inspecting optimiser output; a pattern not on the list is an allocation.

The VM may allocate its own objects to execute logically `noalloc` code; that overhead is labelled separately and does not invalidate the native claim.

### 11.5 Contracts

```fortis
// accept
export contract FastCalculation {
  pure
  noalloc
}

export FastCalculation func square(value: Int<-1000000..1000000>) Int => value * value
```

A contract is a named conjunction of modifiers, forbidden effects (`forbids recovers`, over the eight effects only), required authorities (`requires capability Clock`, `requires capability TaskContext`: the function must take a parameter of that `Authority` type, capability or per-activation) and excluded authorities (`excludes capability Network`: the function may not receive that authority type and may not call, directly or through callees, a function that requires it; summaries carry the set of authority types a function requires, so the check is transitive and closes over wrappers and callbacks). The word `capability` in both clauses names any `Authority` type. Nothing else may appear in a contract. A contract *subtracts*: applying it never changes what a function does, it only makes the checker reject a body that does something the claims rule out. Because a contract is a set of modifiers, it is applied where modifiers are written: `export FastCalculation func …`, `FastCalculation fn(…) …`; several names may be listed and compose by conjunction. The toolchain shows the expansion of a contract name on request; the formatter never rewrites the name, because an aspect may `needs` it by name, and prints the single-word modifiers it expands to after it (`export FastCalculation pure noalloc func square`, X-130). A contract never takes arguments and is never written with `@`; behaviour around a function is an aspect (11.8). Contracts compose by conjunction; contradictory contracts are rejected (`E-EFF-050`, X-526): a `pure` function may not take a capability parameter at all, so `pure` with `requires capability` is a contradiction, and so is a modifier that permits an effect beside `forbids` of that effect (`async` with `forbids suspend`, `try` with `forbids fail`, `recovers` with `forbids recovers`), and one authority both required and excluded. A contradiction within one contract is reported at the contract; one between a function's written modifiers and a contract it carries, or between two contracts it carries, at the function, with the side the function then breaks (`E-EFF-001`). The type a `capability` clause names must be an Authority type (11.6), or the clause is `E-TYP-040`: a type in scope by its declaration, a name not in scope by being one of the capabilities 11.6 lists, as the type of an authority parameter is judged, so `excludes capability Network` needs no import and `excludes capability Netwrok` is refused. The origin of every obligation (modifier, contract, interface, mixin, aspect) is recorded for diagnostics. Project configuration never attaches or selects contracts: a contract applies to a function only by appearing in its head. Configuration chooses destinations, sampling, retention and provider implementations within what the source declares.

### 11.6 Capabilities

An `io` effect requires an **authority value** in the signature: a nominal, non-copyable, non-serialisable value that implements the marker interface `Authority` (9.5), passed as a parameter. Authorities come in two closed sets. **Capabilities** (`FileSystem`, `Network`, `Clock`, `Random`, `Environment`, `Process`, `Supervisor`, `Console`, `SecretAccess`, `RawSql`) are process-wide and are created only for the entry `main`: by the runtime, for its capability parameters (4.6), or by its one call of `Host.capabilities()`. **Per-activation authorities** are minted by an adapter from a capability for one unit of work and handed to it as a parameter: a server adapter mints `RequestContext`, `Identity`, `Db` and `Tx` per request from `Network` and the database connection, a job adapter mints `TaskContext` per run, a mail adapter mints `Mail` from `Network` and configuration. Both sets are listed in section 15; a kit adds a per-activation authority by declaring a type that implements `Authority` and naming the capability it is minted from. The runtime creates the capabilities once, before the entry `main` runs, and passes the ones `main` declares as parameters (4.6). A `main` that declares no capability parameter may create them itself with `Host.capabilities()` in `fortis.host`, which is called once, directly in the entry `main`'s own body: not inside a loop (its condition included), a lambda or an aspect's hook, which could run it again, and never taken as a function value. A `main` that declares a capability parameter may not call it, since its capabilities already exist. A function named `main` in any other module or package is an ordinary function, and Fortis code never refers to the entry `main`, which only the runtime calls. A backend that is nevertheless asked for a second creation stops the run with the runtime-invariant defect (10.8): the checker's guarantee is broken. Capabilities are attenuated by methods (`fs.subtree(path)`) and passed explicitly. A `Shared[A]` of an authority `A` carries `A`'s authority: a parameter of that type satisfies the rule below exactly as an `A` parameter does, which is how several owners hold one capability (`let console = Shared.create(move c)`, then `console.retain()` per owner), since no field stores a loan (7.7, X-137). A per-activation authority is created only by its adapter's minting function, which itself requires the capability it derives from, so every authority traces back to `main`. The capability types are defined by the core (section 15) and re-exported by `fortis.host`; the Runtime host kit adds attenuation helpers and adapters. `report` (section 10.6) and the `log` statements (section 13) are the `io` operations exempt from this rule: they write only to the diagnostic plane (11.7). A function with the `io` effect therefore names, in its signature, the authority it uses. A method's receiver is one of its parameters: a method may also use an authority that its receiver's class holds in a field whose type is that authority or a `Shared` of it, which is how an owner holding a `Shared[Console]` prints from its methods and its `cleanup` (X-137). A field of any other type names none (a `Capabilities` record names none, as a `Capabilities` parameter names none), and an associated function names its authority by a parameter, like any function. The static rule is only that the capability type must be in scope by type; which directory or host it reaches is determined by the value and enforced by the runtime adapter. Reading a clock is `nondet` and requires `Clock`; a `pure` function receives time as an ordinary parameter.

A function other than the entry `main` whose summary reaches `io` through a capability without taking an `Authority` value, any use of `Host.capabilities` other than that one direct call (a call elsewhere, a second call, a call inside a loop, lambda or hook, a call in a `main` that takes a capability parameter, the function taken as a value), and any reference to the entry `main` are rejected (`E-EFF-010`). Capabilities, effects and **target feature sets** are three different things: a target feature set (`core`, `alloc`, `host`, `async`, `web`, `browser`) decides which standard modules exist for a compilation target; an effect is a fact about a function; a capability is a run-time authority value.

### 11.7 Observation plane

Instrumentation that the compiler generates for `capture`, `record` and completion events observes a `pure` function without changing its effect summary: capture expressions are themselves pure and infallible, and the emitted records go to a diagnostic plane that cannot feed values back into the program. The diagnostic plane is also where `report` (10.6) and the `log` statements (13) write: its records go to the task reporter (15), never to the program's standard output, and nothing written there can be read back by the program. Two equivalences are distinguished: **application equivalence** (same results and same application effects) and **trace equivalence** (also the same required observation records). Optimisation must preserve application equivalence always and trace equivalence for records declared required in an observation schema; non-required instrumentation may be elided or merged.

### 11.8 Aspects

An **aspect** is declared behaviour applied around a function. Where a contract subtracts, an aspect adds: code that runs before the body, code that runs after a successful body, a mapping for failures the body leaves unhandled, and metadata that a library reads. Aspects are how kits attach routes, authorisation, limits and schemas to ordinary functions, and how a team writes its own reusable conventions (`@transactional`, `@audited`). They are declared in ordinary modules, user packages included, and are typed, checked and lowered like any other code; there is no reflection and no runtime registry.

```fortis
// accept
import fortis.http.server { HttpMethod, HttpStatus, HttpLimits, PublicProblem, Forbidden }
import fortis.security { Authorisation, Identity }
export aspect http[R](
  method: HttpMethod,
  path: StringView,
  responses: VariantMap[R, HttpStatus],
  auth: dynamic Authorisation? = null,
  fallback: (fn(PublicProblem) R)? = null,
  limits: HttpLimits = HttpLimits.default(),
) {
  needs who: Identity                               // minted per request by the server adapter; never a request field
  before {
    if auth != null && !auth.grantedTo(who) {
      fail Forbidden()
    }
  }
  on failure(f: Failure) R {
    if fallback == null {
      fail move f                                     // no mapping: propagate
    }
    report annotate(move f, path) as receipt          // mapping: report once, then respond
    return fallback(PublicProblem.from(receipt))
  }
  metadata
}

export aspect get[R](path: StringView, ...rest) = http[R](method: GET, path: path, ...rest)
```

**Declaration.** `aspect name[R](params) { parts }`. The name is lowercase (it does something). `R` is an optional type parameter bound at each application to the target function's success type. Parameters are ordinary named parameters with defaults; their types may be anonymous record types, so an application can carry nested properties (`limits: { bodyBytes: 65536, timeout: 5s }`). A parameter name cannot be a keyword, like any parameter. The parts are a closed set:

| Part | Meaning |
|---|---|
| `needs name: Type` | the target must have exactly one parameter of this type (two is an error naming both); the hooks use it by this name with the mode the target declared (`needs db: editable Db` requires an `editable` parameter) |
| `needs Contract` | the target must carry this contract in its modifier position |
| `before { … }` | runs before the body, with the aspect's arguments and `needs` parameters in scope; its locals end when it ends |
| `after(result: R) { … }` | runs after a successful body with a read loan of the result; may `fail` under the same rule as `before`, but only when `R` is not linear, since a failing `after` would leave the result with no consumer (`E-OBL-008`) |
| `on failure(f: Failure) R { … }` | receives any failure that `before`, the body or `after` leaves unhandled, owning its obligation; must `return` an `R` or `fail` (re-failing to map a low-level failure to a domain one is permitted) |
| `metadata` | marks the arguments as data readable by a library through the toolchain (a router, a schema generator) |

A specialisation `aspect name[R](params, ...rest) = other[R](fixed: value, ...rest)` declares one aspect as another with some arguments fixed; `...rest` forwards every remaining named argument unchanged. There is no `around` hook and no access to the body: an aspect can never change what a line inside the function means, and cannot re-run the body (a retry is a library function taking a `fn once`, not an aspect).

**Application.** `@name(args)`, or `@name` alone for an aspect whose parameters all have defaults or that has none, on the lines directly above a function head (after any documentation comment, before `export`). Arguments are compile-time constant expressions: literals, record and variant-map literals of them, enum values, `fn` literals without captures, and names of declarations such as observation schemas, functions or enum variants (a variant name used where a function type is expected denotes its constructor, 9.7). The first declared parameter may be passed positionally; all others are named. An application whose arguments do not match the declaration, an unknown aspect, an unmet or ambiguous `needs`, or `...rest` outside an aspect's parameter list is rejected (`E-TYP-150`).

**Elaboration.** `@a(args) @b(args) func f(params) R { body }` elaborates, before ownership and effect checking, to:

1. an inner function `f.body(params) R` holding the original body, marked `try` if the original is or if any applied hook can fail;
2. an outer `f(params) R` with the same signature that runs the `before` hooks in application order (first line first), then calls `f.body`, then the `after` hooks in reverse application order;
3. around all of that, the `on failure` hooks, **innermost first**: a failure from any step goes to the last-applied aspect's handler; if it re-fails, to the next outer one; a handler that returns a value completes the function with it. The first line is therefore the outermost wrapper both for `before` (runs first) and for failure handling (sees a failure last).

The elaborated outer function is checked as a whole: a `before` that fails with a linear `owned` parameter still unconsumed is `E-OBL-001` like any other path (a non-linear owned parameter is simply dropped on that path); a hook that moves a `needs` parameter the target holds by loan, allocates under `noalloc`, reports under `pure`, or calls `Host.capabilities()` is rejected by the ordinary rules. A `before` may `fail` on a non-`try` target only when an `on failure` of the same or an outer aspect maps every failure to a value; otherwise the target must be `try`. Cancellation unwinds through hooks like any blocks. For observation purposes, "success" and "failure" describe the completion of the outer function, so a mapped failure is a success exit, and the frame is shared by hooks and body. The hooks' effects join the function's summary. There is no aspect at run time.

**Core aspects.** The core declares two aspects. `observe(schema: Schema)` attaches an observation schema (section 13) to a function. `ignorable()`, applied as `@ignorable`, lets callers drop the function's non-linear result without `discard` (5.5; X-345); it has no hooks, only metadata the checker reads, and an application of it to a function whose result is linear is rejected (`E-TYP-150`). Everything else is a library.

| Rule | Statement | Acceptance evidence |
|---|---|---|
| Rule | Statement | Acceptance evidence |
|---|---|---|
| LANG-11-01 | The effect set MUST be exactly the eight effects of 11.1; summaries MUST be computed transitively with the fixed-point rule of 11.3. (old EFF-001) | Transitive positive/negative graph fixtures. |
| LANG-11-02 | Fresh-region mutation and writes through the function's own `editable` parameters MUST NOT count as `mutate` (the latter form the write footprint); mutation through captures, guards or atomics MUST. (old EFF-002) | Builder/caller-buffer test pair. |
| LANG-11-03 | `noalloc` MUST be checked against the pattern whitelist for the actual target and telemetry plan, covering failures, cleanup, closures and all async phases; a conflicting policy MUST be rejected at the policy site. (old EFF-004) | Allocation audit corpus. |
| LANG-11-04 | Trusted summaries MUST be labelled `trusted`, never `proved`, and only the packages of the published compatibility set MAY declare `trusted`. (old EFF-005) | Foreign-boundary reports; a program's `trusted` declarations rejected. |
| LANG-11-05 | `async`, `try` and `recovers` MUST be written wherever the summary has them; across a module boundary only written `pure`/`noalloc` claims MAY be relied on; weakening an exported function's written modifiers MUST be a reported API change. | Semantic-diff corpus. |
| LANG-11-06 | Contracts MUST be conjunctions of 11.5 elements that contradict neither one another nor the modifiers written beside them (`E-EFF-050`), and a `capability` clause MUST name an Authority type (`E-TYP-040`); configuration MUST NOT introduce predicates absent from source. (old EFF-006, CFG-003 part) | Contract-origin fixtures; contradiction and capability-clause rejection fixtures. |
| LANG-11-07 | `io` effects other than `report` and `log` MUST require an `Authority` value in the signature, except in the entry `main` (4.6), which receives the capabilities as parameters or creates them with one direct call of `Host.capabilities()` outside every loop, lambda and hook, and in an adapter's minting function, which requires the capability it derives from; every other use of `Host.capabilities` (including one in a `main` that takes a capability parameter), and every reference to the entry `main`, MUST be rejected. | Missing-authority corpus; capability-creation corpus (loops, conditions, lambdas, hooks, function values, other modules and packages, references to the entry `main`, a `main` with capability parameters); a second creation is the runtime-invariant defect on every backend. |
| LANG-11-08 | Contracts MUST be applied in the modifier position and MUST NOT take arguments; `@` MUST apply aspects only. | Grammar corpus. |
| LANG-11-09 | Aspects MUST have only the parts of 11.8, MUST elaborate as in 11.8 before checking (hooks in application order, failure handlers innermost first), and the elaborated function MUST pass every ordinary check. | Aspect corpus incl. unconsumed-owned-parameter, moved-needs, noalloc, pure and capability negatives; elaborated-source golden tests. |

## 12. Cleanup, async and concurrency

### 12.1 `defer`

```fortis
// illustrative
defer {
  statements
}
```

`defer` registers a block to run when the enclosing block exits by any route. Only the block form exists; there is no call form. Deferred blocks run in reverse order of registration, before the block's locals are dropped. A deferred block:

- is checked as if its body were written at every exit point of the block, after the loans held by the block's own bindings have ended and before the block's locals are dropped. The places it names, inside an interpolation hole as anywhere else (3.6), must therefore be initialised at every exit: moving one of them out between the `defer` and the exit is an error (`E-DEF-002`). Between registration and exit the rest of the block may use those places freely. Loans held by bindings of *enclosing* blocks still apply at the exit and conflict as usual. Because the block's own loans have ended by then, a deferred block may not use a binding of its block that holds a loan (a view, a loan returned `from` a parameter or a receiver, whole or inside an optional, a tuple or a record, as `let t = xs.get(0)` holds one; D-364): `E-OWN-052`. A binding of an enclosing block, and a loan of a trivial value, which holds nothing, may be used.
- may not contain `return`, `fail`, `give`, `break`, `continue` or `await` (`E-DEF-001`); it cannot alter the enclosing function's outcome.
- consumes every failure it produces itself, with `or recover`, `catch … recover` or `report`; `or fail` is unavailable because there is no outcome to propagate to. A failure reported from a deferred block reaches the task reporter like any other report, labelled as raised during cleanup. Recovering a cleanup failure silently is permitted and adds `recovers` to the function's summary.
- contributes its effects to the enclosing function's summary.

This is possible because `defer` lowers to explicit cleanup code at each exit (Appendix C), not to a closure or a destructor, so no loan has to be held across the block.

```fortis
// accept
import fortis.host { FileSystem, Path }
try func copyFile(fs: FileSystem, source: Path, target: Path) Void {
  let mut input = fs.open(source) or fail
  defer { editable input.close() }            // checked at each exit; input must still be owned there
  let mut output = fs.create(target) or fail
  defer { editable output.close() }
  let data = editable input.readAll() or fail  // the defer holds no loan, so this is fine
  editable output.writeAll(data) or fail
}
```

`defer await` is not in v1 (`E-DEF-001`, like any `await` inside a deferred block); asynchronous cleanup is performed by the owning `scope` (12.5) or by explicit `await` calls before the block ends. The Delivery Programme's spike SP-06 gates any future extension.

### 12.2 Destruction order

Section 7.9 defines when values are destroyed. In summary, at a block exit: deferred blocks in reverse registration order; then owned locals in reverse initialisation order; for each class value, its `cleanup` method, then its fields in declaration order. The order is the same on both backends and under every exit kind, including cancellation observed at an `await`. A local that holds a loan is not an owned local: its end runs no `cleanup`, which runs once per value, when the value's owner destroys it (7.9, D-338).

### 12.3 Async functions and futures

An `async func f(…) T` call produces a `Future[T]` (or `Future[Outcome[T]]` when also `try`) without running the body. Arguments are evaluated at the call; the body starts at the first `await` of the future or when a `scope` launches it. `Future[T]` is linear: it must be `await`ed, launched on a scope, or `discard`ed. `discard` of a never-polled future is the one exception to the rule that linear values cannot be discarded (5.5, 6.12): it is permitted only when the future's captured environment contains no linear value and no exclusive loan, and it emits a `not_started` lifecycle record through the task reporter of the current task. A future holds the loans of its arguments (7.3); it may not outlive them, and may not be stored in a type without a lifetime parameter unless it holds none. A temporary argument or receiver (a call's result, a constructed value, a `copy`, an interpolated string) ends with its statement (5.7), unless its future is launched directly (`s.launch(f(…))`, `s.launchParallel(f(…))`), in which case the task owns it until the task has run. Otherwise a future that borrows a temporary is awaited within that statement: a statement that binds, assigns, returns or gives such a future, or an expression body that returns it, is rejected (`E-OWN-051`, X-251). Bind the argument first, or take the parameter `owned`.

`await e` suspends the current task until `e` completes and yields its value. It consumes `e`: a future (or a task) is awaited through a place that owns it, and once. One reached through a loan or a `Shared` belongs to another owner and cannot be awaited (`E-OWN-001`, as no field can be moved out of a loan), a field of an owned value would be a partial move (`E-OWN-040`), and a second `await` is a use after move (`E-OWN-020`). `await` is permitted only directly in an `async` function body (not inside a lambda that is not itself `async`, not inside `defer`). Loans held across an `await` remain active; an exclusive loan of a `Mutex` guard may not be held across an `await` (`E-ASY-002`). A `Task` that leaves its scope block, by `return`, `give` or assignment to a binding declared outside it, is rejected (`E-ASY-001`).

### 12.4 Cancellation

Cancellation is a signal delivered to a task. It is observed only at a **cancellation point**: `await` on the cooperative executor, and `await` or an explicit `checkpoint()` call on the parallel and blocking pools. Cancellation is not a failure: it cannot be caught, recovered or matched. When a task observes it, the current function unwinds through every block to the task boundary, running deferred blocks on the way (12.2), and the task completes as cancelled. A future is never destroyed while suspended: a cancelled task resumes at its cancellation point, unwinds and finishes. Synchronous code runs to completion. A `Duration` deadline attached to a scope delivers the same signal when it expires.

Because unwinding runs no user code other than `defer` blocks and `Abandonable.abandon`, a linear value live at a cancellation point would have no consumer. The rule is therefore: **no linear value may be live across a cancellation point** (`E-OBL-005`) unless its type implements `Abandonable`: `interface Abandonable { func abandon(owned self) Void }`, whose `abandon` must be synchronous, non-failing and non-suspending. The order at each block during an unwind is the block-exit order of 5.1 with one addition: first the block's loans end and the tasks launched on a `scope` declared in the block are cancelled and joined (12.5), which also ends the loans those tasks carried; then the deferred blocks run; then `abandon` is invoked on every other live `Abandonable` local that owns its value, in reverse initialisation order (a local that holds a loan of one is not abandoned: its owner is, once; D-338); then the locals are dropped. Unwinding means leaving the function by cancellation **or by `fail`**, so an `Abandonable` local is also the one linear value that may be live at an `or fail`: the failure path abandons it, while a normal exit still has to consume it explicitly (`E-OBL-001` on falling off the end). Unwinding a statement first ends every loan that statement activated (the loan the suspended call holds on `tx`, for example), so `abandon` always runs on a local with no active loan, exactly as a block exit ends loans before dropping locals (5.1). `Task` implements it (its `abandon` is what the owning scope invokes when it cancels and joins, so a task handle is never abandoned after the block's `defer`s), and a library handle such as a database transaction implements it by marking itself abandoned for its pool to roll back, which is exactly the "failure rolls back, success must commit" rule a transaction wants. So `let b = tasks.launch(…)` or `let tx = db.transaction()` may be live while an `await` suspends, but an `Outcome`, a `Failure` or a `Future` may not. Consume such a value before the `await`, pass it into the awaited call, or restructure. The futures and tasks the function is awaiting are not "live across" their own `await`: an awaited child future is cancelled with its parent and the parent waits for its unwind to complete.

### 12.5 Structured scopes and `launch`

```fortis
// accept
import fortis.host { Network }
import acme.net { Url, Body, fetch }
async try func fetchBoth(net: Network, a: Url, b: Url) (Body, Body) {
  scope tasks {
    let first = tasks.launch(fetch(net, a))
    let second = tasks.launch(fetch(net, b))
    return (await first or fail, await second or fail)
  }
}
```

`scope name { … }` is a block that owns tasks. `name.launch(future)` consumes a future and returns a `Task[T]`, which is linear and must be `await`ed inside the scope or passed to `name.detach(move task, supervisor)`, which transfers the join obligation to a `Supervisor` capability (section 11.6) and removes the task from the scope. `await task` yields `T` (the body's result, which may be an `Outcome`): a child task cannot complete as cancelled or defective without the same event reaching the parent, because cancellation of a scope cancels every task in it and a defect in any task terminates the whole scope tree up to its supervisor (10.8). `TaskCompletion[T]` (12.6) is therefore observed only by supervisors of detached tasks.

When the block exits by `fail`, `return`, `break` or cancellation while tasks are still running, the scope cancels them and waits for every one to finish as part of ending the block's loans, before the block's deferred blocks run (12.4), so a `defer` in the scope block never sees a loan still carried by a task; the results of tasks that were never awaited are consumed by the scope (a failure inside such a result is reported, labelled as abandoned by the scope). `await first or fail` leaving the block therefore cancels `second`. A scope never leaks a task and never leaves a loan live after its owner's block, which is what allows cooperative `launch` of futures holding loans without a separately proved scoped scheduler: the tasks run on the parent's thread and are joined before the block ends.

Variants of `launch`:

| Form | Executor | Requirements |
|---|---|---|
| `s.launch(f)` | same cooperative executor, same thread | the future may hold loans of any place that outlives the task: every binding declared in or before the scope block qualifies, because the scope joins its tasks before those bindings are dropped |
| `s.launchParallel(f)` | CPU worker pool | the future's environment is `Shareable` and holds no loans; cancellation observed at `await` and at explicit `checkpoint()` calls |
| `s.launchBlocking(work)` | bounded blocking pool | `work` is a synchronous `fn once() T` (not a future) whose environment is `Shareable`; it is the only place the `block` effect is permitted inside async code; it observes cancellation only at `checkpoint()` |

The v1 executor runs every cooperative task of one `scope` tree on one thread; `launch` never requires `Shareable`. Borrowed sibling concurrency without a scope uses `join(a, b)`, which consumes both futures, polls them inside the parent and returns `(A, B)` once both have completed; it accepts futures holding loans. `join` never returns early: if the parent is cancelled, both futures are cancelled and run to completion first, and a failure value produced by one future is simply part of the returned tuple, so the caller consumes both outcomes.

### 12.6 Task completion

A supervisor observing a detached task receives `TaskCompletion[T]`:

```fortis
// accept
enum TaskCompletion[T] {
  Completed(value: T)
  Cancelled(reason: Cancellation)
  Defect(info: TaskDefect)
}
```

`Completed` carries the body's result, which may itself be an `Outcome`. `Cancelled` and `Defect` are distinct from domain failures and are never seen as a `Failure`. `TaskCompletion[T]` is linear when `T` is linear; a supervisor consumes it with `match`.

### 12.7 Sharing data between tasks

A value crosses `launchParallel`/`launchBlocking` only if its type is **`Shareable`**, a structural property:

| Shareable |
|---|
| trivial types; `String`, `Bytes`, `BigInt` |
| `List`, `Array`, `Map`, `OrderedMap`, `Set`, `FixedList`, `FixedMap` of shareables |
| classes, enums, tuples and records whose fields are all shareable |
| `Shared[T]` and `Weak[T]` where `T: Shareable`; `Mutex[T]` where `T: Shareable`; `AtomicInt`, `AtomicBool` |
| `fn`/`fn once` closures whose captures are all `copy` or `move` of shareables |
| capability values |

Not shareable: loans and views, `MutexGuard`, `fn mut` closures, `Future`, `Task`, resources that declare themselves thread-bound. A pool launch whose future's arguments or closure's captures include a loan or a value that is not `Shareable` is rejected (`E-ASY-003`).

Mutation shared between tasks goes through `Mutex[T]`. In async code `await m.lock()` suspends until the lock is free; in synchronous code (including `launchBlocking` bodies) `m.lockBlocking()` has the `block` effect. Both return `Outcome[MutexGuard[T]] from mutex`; the guard is an exclusive loan of `T`, is not `Shareable`, cannot cross an `await`, and unlocks when destroyed. The value is read and written through it: `guard.get()` copies it (for a `Copyable` `T`), `guard.set(value)` replaces it, dropping the old value, and `guard.replace(value)` replaces it and returns the old one, so any `T` can be taken out, changed and put back; a write through a guard is `mutate` (11.1). `AtomicInt` and `AtomicBool` offer `load`, `store`, `fetchAdd`, `compareExchange` with orderings `seqcst` (default) and `acquire`/`release`. There are no other orderings. Because an exclusive loan can never be shared and shared data is immutable unless behind a `Mutex` or atomic, Fortis programs are data-race free by construction; this is a property of the source rules and the VM and native backends both preserve it. No agreement between backends is claimed for the timing of interleavings.

| Rule | Statement | Acceptance evidence |
|---|---|---|
| LANG-12-01 | `defer` bodies MUST be checked at every exit as in 12.1, MUST NOT alter the function's outcome, and the places they name MUST be initialised at every exit. | Defer corpus with every exit kind. |
| LANG-12-02 | Failures inside `defer` MUST be consumed in the body and MUST NOT replace the primary outcome; reports from cleanup MUST be labelled as such. (old RES-002) | Multiple-failure execute tests. |
| LANG-12-03 | `Future` and `Task` MUST be linear; a pending future MUST never be destroyed; cancellation MUST be observed only at the cancellation points of 12.4. (old ASY-001, ASY-003) | Cancellation corpus. |
| LANG-12-04 | A scope MUST join or cancel every task it launched before its block exits. (old ASY-004) | Scope corpus and leak tests. |
| LANG-12-05 | Only `Shareable` environments MAY cross `launchParallel`/`launchBlocking`; loans MUST NOT cross any launch except cooperative `launch` within the owner's block. (old ASY-002) | Borrowed-spawn rejection and native compile tests. |
| LANG-12-06 | A `MutexGuard` MUST NOT be live across `await`. | Guard corpus. |

## 13. Observation schemas and statements

The core defines the `observation` schema declaration and two statements that record facts about an activation; everything downstream (destinations, projections, export) is the Telemetry kit.

```fortis
// accept
import acme.orders { CustomerTier, Region, TransactionId }
observation OrderProcessing {
  version: 1                         // optional; 1 when omitted
  customerTier: CustomerTier         required: all
  region: Region                     required: all
  transactionId: TransactionId?      required: success
  encoding: bounded(512)
}
```

A schema names typed fields, each with `required: all | success | failure | none`, an optional `version` and an encoding bound. Every field type must be `Encodable`, checked recursively like any derivation (6.10); since `Secret[T]`, authorities, loans, resources, closures, futures and tasks are not `Encodable`, none of them can occur anywhere inside a captured value. A schema is attached to a function with the core aspect `@observe(OrderProcessing)` (11.8), or by a library aspect that takes a schema as a property.

```fortis
// accept
import acme.orders { Order, Payments, Receipt, OrderProcessing }
@observe(OrderProcessing)
async try func processOrder(order: Order, payments: Payments) Receipt {
  capture customerTier = order.customer.tier
  capture region = order.customer.address.region
  let receipt = await payments.charge(order) or fail
  capture transactionId = receipt.transactionId
  return move receipt
}
```

`capture field = expression` assigns a snapshot of `expression` to the named field of the **observation frame** of the current activation. The frame's fields and their types come from the observation schema attached to the function (`@observe(OrderProcessing)`). The expression must be `pure`, infallible, non-suspending and of the field's declared type; it takes a read loan for the statement and copies the value into the frame, so later mutation cannot affect it. A `String` field must declare a bound in the schema; values longer than the bound are truncated or omitted as the schema declares, and the frame records which happened.

The checker treats the frame as a set of definitely-assigned variables over the function's control-flow graph, including `fail`, `return`, `await` and cancellation edges:

- a field declared `required: all` must be assigned on every path before every exit;
- a field declared `required: success` (or `failure`) must be assigned before every exit of that class;
- a required field left unassigned on some applicable exit is rejected (`E-OBS-001`, naming the field and the exit);
- a second `capture` of the same field on one path is rejected (`E-OBS-010`);
- a `capture` inside a loop body is rejected (`E-OBS-011`); repeated measurements use `record`;
- a `capture` or `record` whose expression has an effect, or that names no field of the attached schema, is rejected (`E-OBS-012`).

`record Name(field: value, …)` emits an event of the named schema immediately, as many times as it executes; it has the same purity requirement for its arguments. A frame carries its schema and version, the observed function, the captured fields, the fields cut to their bound, and the activation's outcome class (`success`, `failure`, `cancelled`, `defect`), which the compiler generates and source cannot forge. An activation that is cancelled before it starts produces a frame marked `not_started` without any captured fields. The core frame carries no timing: an activation's start, end and duration are the Telemetry kit's to define (X-252).

**Logs.** `log.info(message)`, `log.warn(message)` and `log.error(message)` are observation statements: each execution writes one `log` record, carrying its level, its message and the function that wrote it, to the diagnostic plane (11.7). Logging is a platform-native concern (X-147), so it needs no capability and no configuration in the program (X-345): the task reporter (15) writes the record to standard error, rendered as text on a terminal (`warn: could not take 9 nuts`) and as one JSON line otherwise, where the platform's log collection picks it up. A log record never mixes into the program's standard output, which is its own product (`console.print`), and every backend writes the same records in the same order. `message` is a `StringView`, so a literal, an interpolated literal or a `String` may be passed; it is evaluated like any argument before the record is written. A log statement has the `io` effect, like `report`, so a `pure` function cannot log; with `report`, it is the `io` that needs no authority (11.6).

```fortis
// accept
import fortis.host { Console }
func main(console: Console) Void {
  log.info("starting")
  console.print("the result")
  log.warn("finished with a warning")
}
```

`log` is not a keyword. The statement is `log.` followed by `info`, `warn` or `error` and one argument, written as a statement where `log` names no binding or declaration in scope, so a program that already names something `log` keeps it. `log` used any other way is an ordinary name, unresolved unless the program declares it (`E-NAM-001`, which also names the three levels). A failure is recorded with `report e` alone; `log` is for what is not a failure (a fallback taken, progress, a summary), so a handled failure is reported, not logged as well.

| Rule | Statement | Acceptance evidence |
|---|---|---|
| LANG-13-01 | `capture` MUST snapshot at the statement, copying the value; expressions MUST be pure and infallible. (old OBS-001) | Mutation-after-capture tests. |
| LANG-13-02 | Required fields MUST be definitely assigned on every applicable exit exactly once; loops and double assignment MUST be rejected. (old OBS-002) | Branch/failure/cancellation corpus. |
| LANG-13-03 | Frame contents MUST be bounded by the schema; truncation or omission MUST be recorded. (old OBS-006) | Oversize corpus. |
| LANG-13-04 | Each execution of `log.info`, `log.warn` or `log.error` MUST write exactly one `log` record to the diagnostic plane, never to standard output, the same on every backend; it MUST need no `Authority` and MUST have the `io` effect. | Log execute fixtures on every backend; a `pure` function that logs is rejected. |

## 14. Test declarations

Tests are declarations and part of the core grammar, so every package can be tested without a kit. The runner, property generation, fuzzing, mutation and benchmarks are in the Delivery Programme: `fortis test` runs example tests, table rows, properties and (with `--fuzz`) every property as a coverage-guided fuzz target, and reports a failing property's minimal input with the seed that replays it (X-131).

```fortis
// accept
import acme.accounts { isAdult, loadAccount, CustomerId, Account }
test "adult boundary" cases {
  "below": { age: 17, expected: false }
  "equal": { age: 18, expected: true }
} as row {
  expect isAdult(row.age) == row.expected
}

test "guest account on failure" {
  let account = loadAccount(CustomerId(999)) catch e {
    expect e.code == "accounts.not_found"
    recover Account.guest()
  }
  expect account.isGuest()
}

property "reversing twice is identity" forall (items: List[Int<0..100>]) {
  expect items.reversed().reversed() == items
}
```

A `test` body is a block whose implicit function is `try` (`or fail` is available and a failure fails the test). A test can only react to a failure it did not expect by failing, so in `test` and `property` bodies `or fail` is implied (X-345): a call of a fallible function there has its success type, and its failure fails the test with the failure's record, exactly as `or fail` would. `expect (f() or fail) == v` is written `expect f() == v`, and `let x = f()` binds the success value. The rule covers every call in the body whose callee is fallible (`try`), except a call that is the left operand of `or` or `catch` or the scrutinee of a `match`, which consume the outcome themselves, so expecting a failure stays as explicit as before; it does not reach into a lambda written in the body, whose body is its own function. A `let` that binds such a call keeps the outcome, as before, when the body later consumes that binding as an outcome (`let o = f()`, then `o catch`, `o or …` or `match o`) or when the binding is annotated `Outcome[…]`. Integer arithmetic in the body is a checked region in the same way (6.4): an `Int` operator the checker cannot prove safe is the checked operation, and its failure fails the test, so `expect (a * b or fail) == c` is written `expect a * b == c`; arithmetic that is the left operand of `or` or `catch` keeps its own region. Writing `or fail` stays accepted. Outside tests nothing changes: an unconsumed outcome is still `E-OBL-001` or `E-TYP-040`.

```fortis
// accept
export error Negative {
  code: "demo.negative"
}

try func half(n: Int) Int {
  if n < 0 {
    fail Negative()
  }
  return n / 2
}

test "half of an even number" {
  expect half(10) == 5
  let quarter = half(half(8))
  expect quarter == 2
}

test "a negative number fails" {
  let r = half(-2) catch e {
    expect e.code == "demo.negative"
    recover 0
  }
  expect r == 0
}
```

`cases { … } as name { … }` binds each row explicitly to `name`. The row record type is inferred from how the body uses the fields (bidirectionally, as for any literal), and every row literal is then checked against it at compile time, so `quantity: 1` in a row passed to an `Int<1..10000>` parameter is checked exactly like the literal `1` would be. `expect condition` is a statement that records a typed actual/expected pair and fails the test when `condition` is false; `expect` of a non-`Bool` is an error. `expect` is test code: it may appear only in a `test` or `property` body, the `cases` bodies inside them and a lambda written there, and anywhere else (a function body, a method, a deferred block, a lambda written outside a test) it is `E-TYP-120`, as `give` outside a match arm is (the owner's decision of 29 October 2026, Q9 of v0.1; X-526, D-414). Test-only modules would widen the rule; v0.1 has none. Expecting a failure consumes its obligation inside the test. Test declarations are compiled only in test builds and may import test-only modules.

| Rule | Statement | Acceptance evidence |
|---|---|---|
| LANG-14-01 | Table rows MUST be bound explicitly with `as`; no implicit names enter scope. | Grammar corpus. |
| LANG-14-02 | `expect` MUST take a `Bool` and MUST record typed evidence. | Runner golden tests. |
| LANG-14-03 | In `test` and `property` bodies, a call of a fallible function that is not the left operand of `or` or `catch` or a `match` scrutinee, nor bound by a `let` that the body consumes as an outcome or annotates `Outcome[…]`, MUST behave as that call followed by `or fail`, and integer arithmetic there outside such an operand MUST be a checked region followed by `or fail` (6.4); outside them nothing changes. | Test fixtures with implied `or fail` on calls and on arithmetic, passing and failing, on every backend; the same call outside a test, and arithmetic in a test's lambda, rejected. |
| LANG-14-04 | `expect` MUST appear only in a `test` or `property` body, the `cases` bodies inside them and a lambda written there (`E-TYP-120`). | Rejection corpus (a function body, a method, a deferred block, a lambda outside a test); acceptance corpus. |

## 15. Core runtime inventory

The core is not only a compiler: a conforming implementation ships the runtime pieces below, so that a program that imports no kit can still run. Kits add adapters and policy on top; they never replace these.

| Component | Provided by the core | Kits add |
|---|---|---|
| Executor | one cooperative executor per process running the root task and every `scope`; the parallel and blocking pools; cancellation signalling and deadlines | the Tokio-backed adapter, I/O readiness, timers, channels (Runtime host) |
| Task reporter | the diagnostic-plane sink for `report`, `log` records (13), cleanup reports and `not_started` records, and the holder of completed `capture` frames; by default it writes reports and each frame's schema identity, outcome class and duration to standard error, and **never a captured field value**: field values leave the process only through the Telemetry kit's pipeline, after redaction (Platform 6.1, 6.3), and without that kit they are retained in memory for the debugger and REPL and then discarded; the toolchain's development plan `permit-all` (`FORTIS_TELEMETRY=permit-all`, which the conformance runners set) permits every field unchanged on the record stream, an explicit choice like any plan | exporters, archive, projections (Telemetry) |
| Authorities | the marker interface `Authority`; the capabilities `FileSystem`, `Network`, `Clock`, `Random`, `Environment`, `Process`, `Supervisor`, `Console`, `SecretAccess`, `RawSql`, created once per run for the entry `main`'s parameters or by `Host.capabilities()` in `fortis.host`; the rule for per-activation authorities (`RequestContext`, `Identity`, `Db`, `Tx`, `TaskContext`, `Mail`, minted by the owning kit's adapter) | the adapters that mint per-activation authorities, attenuation helpers, sandbox integration |
| Observation schemas | the `observation` head (section 13) | schema registry and pipelines (Telemetry) |
| Entry | `main` in its four forms (4.6), its parameters supplied by type, one report and exit status 1 on failure | the `application` head and lifecycle (Runtime host) |
| Standard modules | `fortis.prelude`, `fortis.text`, `fortis.collections`, `fortis.numeric`, `fortis.time`, `fortis.money` (`Decimal` plus a run-time `Currency`; `Money.of`, `Money.zero`, `Money.add`, `Money.subtract`, `Money.multiply(money, factor: Int)`, `Money.divide(money, divisor: Int<1..>)`, `Money.greaterThan`, the arithmetic fallible on currency mismatch), `fortis.time` (`Duration.days/hours/minutes/seconds/milliseconds`, `Duration.add(a, b)`, `Duration.sub(a, b)` and `Duration.multiply(d, factor)` fallible on overflow of the signed 64-bit nanosecond count, `d.nanoseconds()`, comparison operators on `Duration`, a `Duration` displaying as its count in the largest unit that divides it exactly (`150s`, `1h`, `250ms`, `0s`), `Timestamp.epoch()`, `Timestamp.add(t, d)` fallible on overflow, `Timestamp.after(a, b)`, `Timestamp.before(a, b)`), `fortis.host` (which also defines `Path`, built from text by `try func parse(text: String) Path`, and gives each capability its core operations: `FileSystem` `readText`, `writeAll`, `readAll`, `open`, `create`, `subtree` and the `File` it opens; `Network.resolve`; `Clock.now`, `frozen`, `sleep`, `elapsedSince`; `Random.below(bound: Int<1..>)` and `seeded`; `Environment.get`; `Process.run`; `Console.print`; `RawSql.execute`; kits add the rest. The operations that wait on the outside block the calling thread and have the `block` effect (11.1): `FileSystem`'s `readText`, `writeAll`, `readAll`, `open` and `create`, `File`'s `readAll` and `writeAll`, `Network.resolve`, `Process.run` and `RawSql.execute`, so `async` code reaches them through `launchBlocking`) | everything else under `fortis.*` |
| Secrets | `Secret[T]` (no codec, no capture, fingerprint display: `${secret}` shows a per-process keyed hash, never the value) and `reveal(secret, cap: SecretAccess)` in `fortis.host`, because the core `Environment.get` returns `Secret[String]?` | `declassify`, `Sanitiser`, authentication adapters (Security) |

## 16. Reserved declaration heads for kits

Kits extend the language with **aspects** (11.8) and, where a declaration is not function-shaped, with **declaration heads**: fixed keywords that introduce declarations whose bodies are validated by a versioned schema and elaborated into ordinary core declarations before type checking. The core grammar reserves them so that no identifier collides with a head of any kit, and defines the elaboration contract: a head produces classes, functions, enums, contracts and observation schemas that obey every rule in this document, and nothing else. Packages cannot add heads, tokens or grammar rules.

| Head | Kit | Elaborates to |
|---|---|---|
| `application` | Runtime host | a generated `main`, capability acquisition and lifecycle ordering |
| `explore` | Developer tooling | an exploration declaration (Delivery Programme) |
| `context` (with `provide` inside its body) and the `inject` class member | Dependency injection | constructor parameters and a generated scope struct |
| `model` | Data | a nominal data class, schema, codecs, query metadata |
| `view` (standalone, from `fortis.web.ssr`) and `component` (with `state`, `action`, `view`, `resource` inside its body, from `fortis.web.components`) | Web UI | a render function; a class with an explicit state struct, action functions, render function |
| `observation` | core (section 13); consumed by Telemetry | a frame type and schema |

A head's elaboration happens before ownership and effect checking, so kit declarations have no ownership or effect rules of their own: the generated code is checked like any other.

A head body is a balanced token sequence of the core lexer, with one exception: a **raw template region**. The region is opened lexically, with no knowledge of the enclosing declaration: a statement (in the sense of 3.2, so a continued line is not a new statement) whose tokens are `view`, optionally preceded by `export` and by one of `server`, `client`, `hybrid`, followed by nothing, by a name, or by a name and a balanced parenthesised group, and then `{`. Where the view is a declaration the grammar requires that group to be a parameter list; inside a head body the owning kit validates it. The lexer takes the text from that `{` to the `}` that balances it, counting every `{` and `}` in between (including those of `${…}` interpolations), and hands that text to the Web UI kit unlexed as one token. Where such a region may appear (inside a `component` body, or as a standalone `view` head) is validated by the kit like the rest of a head body. Template syntax is therefore the kit's to define and never adds tokens to the core.

The core grammar provides exactly four **extension slots** through which a kit's syntax enters a module, and nothing else:

1. a head block, optionally prefixed by `server`, `client` or `hybrid`;
2. an `[export] inject [lazy] name: Type` member inside a class body (an exported slot becomes an exported field, so a consumer declared in one module can be constructed by a context in another, X-138);
3. aspects (11.8) declared by the kit and applied with `@name(args)`, which need no slot of their own because they are ordinary declarations;
4. the `cases` table syntax of test declarations.

Head words are reserved words and are never imported; importing a kit package makes its heads available, and its aspects are imported by name like any declaration. The Platform Specification lists the heads, prefixes and aspects each kit defines. A head is checked only through its kit's validator and elaboration, so a head whose kit the toolchain does not install is rejected at `check` with `E-NAM-034`, which names the kit: the core never accepts a declaration it can neither validate nor elaborate, and a test runner never reports a module with one as passing.

## Appendix A. Keyword inventory

Reserved words of the core, with the defining section. A keyword cannot name a declaration, binding or parameter, except that it may name a method or associated function; after `.` it is an ordinary member name (3.4).

| Group | Words |
|---|---|
| Declarations | `func` `fn` `let` `mut` `const` `type` `alias` `class` `trivial` `enum` `error` `interface` `mixin` `implement` `implements` `with` `derive` `requires` `forbids` `excludes` `contract` `aspect` `needs` `before` `after` `on` `metadata` `lifetime` `from` `where` `export` `import` `as` |
| Modifiers | `pure` `noalloc` `async` `try` `recovers` `editable` `owned` `dynamic` `linear` `trusted` `once` |
| Ownership | `copy` `move` |
| Control | `if` `else` `while` `for` `in` `loop` `break` `continue` `return` `fail` `give` `recover` `match` `when` `otherwise` `is` `or` `catch` `discard` `defer` `await` `scope` |
| Values | `true` `false` `null` `present` `success` `failure` `self` `Self` |
| Observation | `capture` `record` `report` `observation` `required` `version` `encoding` |
| Testing | `test` `cases` `property` `forall` `fuzz` `bench` `expect` |
| Kit heads (reserved by the core, defined by kits) | outer heads `application` `context` `model` `component` `view` `explore`; prefixes `server` `client` `hybrid`; words used inside head bodies `inject` `provide` `state` `action` `view` `resource` |

Contextual words that are not reserved: `launch`, `launchParallel`, `launchBlocking`, `detach` (methods on a scope value), `retain`, `validate`, `checkpoint`, `join`, `annotate`, `cleanup`, `capability`, `lazy`, `convert` (a conversion member of a class or enum, 6.13; `to` and `from` after it are ordinary words in that position only). Keywords are reserved in declaration and binding positions, and are ordinary names after `.` and as method or associated-function names (3.4).

## Appendix B. Closed decisions

| Topic | Decision |
|---|---|
| Declaration vs assignment | `let` / `let mut` declare; `=` assigns only. |
| Semicolons | Parse error. |
| `Void` functions | May fall off the end; non-`Void` functions need `return`/`fail` on every path. A head may leave out `Void` (X-345). |
| Markers the declaration shows | An `editable self` receiver or `fn mut` call needs no `editable` wherever the marker could be written, and is checked as the marked call; `return` of a local `let` binding needs no `move` (X-345). |
| Type of `fail` | `Never`. |
| Match-arm value | `give`. |
| Catch binding | Explicit: `catch e { … }`. |
| Unsuffixed integer literal | Type from context, default `Int`; must fit the target refinement. |
| Float literal | Needs `.` or exponent; `f32` suffix for `Float32`. |
| Decimal literal | `d` suffix. |
| String literal | `StringView from static`; converts implicitly to `String` where one is expected (allocates). |
| Named arguments | After positionals, any order, each once; names are public API. |
| Default arguments | Pure constant expressions, re-evaluated per call. |
| Variadics | `ListView[T]` parameter. |
| Negative example marker | `// reject: E-XXX-NNN`. |
| Call-site markers | `move p`, `copy p`, `editable p` required for existing places; fresh values unmarked. |
| Closure kinds | `fn`, `fn mut`, `fn once`, always written. |
| Generic lambdas | Not in v1. |
| Const generics | `N` literal only; no generic functions over `N`. |
| Partial moves | Not in v1. |
| Non-lexical lifetimes | Not in v1; loans are lexical. |
| Stored loans (lifetime parameters on user types) | Not in v1 (X-137); `E-OWN-053`. Loans live in locals, parameters and `from` returns; the standard views and cursors are compiler-known. |
| Two-phase borrows | Yes: reserve at argument evaluation, activate at call entry. |
| Two-origin loans | Rejected. |
| Operator overloading | None; `Type.add` etc. |
| Or-patterns | Yes, narrow (X-345): `A \| B => v`, whole patterns of an arm, no bindings, no guard (9.8). |
| Range patterns | Yes, v0.1 (X-517, X-518, X-522): `a..b`, `a..<b`, one-sided `a..` and `..<b`, over the integer types only; bounds fold as a refinement's; an empty, inverted or out-of-range interval is `E-MAT-005`; usable in payloads and alternatives; over a refined scrutinee the unguarded arms' intervals count for exhaustiveness and narrow the arm (9.8). |
| `nostate` | Removed; lint only. |
| `or die` | Removed; `try main`. |
| `yield` | Replaced by `give`. |
| `defer` call form | Removed; block form only. |
| `defer await` | Not in v1; `E-DEF-001`. |
| Executor | Cooperative single thread per scope tree; parallel/blocking pools need `Shareable`. |
| Atomics | `AtomicInt`, `AtomicBool`; `seqcst`, `acquire`/`release`. |
| Hash iteration | Deterministic per build; allowed in `pure`. |
| Float equality | IEEE `==` allowed; no `Equatable`/`Hashable`/`Comparable`. |
| Entry point | `main` in the manifest's entry module. |
| File extension | `.ft`. |
| Comments | `//` only; documentation is the `//` run directly above a declaration. No `/* */`, no `///`. |
| Transparent type aliases | Not in v1; `type` is always nominal. |
| Stored and returned loans | Both say `from`; `within` is removed. |
| `recovers` | A modifier, written like `try`. |
| Lambda modifiers | Same as named functions; capture brackets only when non-empty. |
| Receiver markers | `editable p.m()` and `move p.m()` required for `editable self` / `owned self` methods. |
| Kept loans at call sites | `alias p` / `editable p` required for an argument the result borrows `from`. |
| Constructor naming | `create`, `of`, `from`; there is no `new`. |
| Test rows | `cases { … } as row { … }`. |
| `report` | A statement; `report e as receipt` binds the receipt. |
| Contracts | Applied in the modifier position by name (`export FastCalculation func …`); never with `@`; no arguments. |
| Aspects | `@name(args)`, lowercase, the only use of `@`; parts `needs`, `before`, `after`, `on failure`, `metadata`; specialisation with `...rest`; `on failure` may re-fail; user packages may declare them. |
| Observation attachment | `@observe(Schema)`, a core aspect. |
| Interfaces | Nominal only; `requires` for sub-interfaces; bundles satisfied automatically; no method bodies. |
| Serialisation | `derive Codec` (= `Encodable` + `Decodable`); canonical `toBytes`/`toText`; formats as packages with `…Serialised` mixins; hand-written `encode` and `derive` cannot coexist. |
| Projection | constructor spread `...copy p` (copies taken fields) / `...move p` (consumes the whole source); `derive convert from S` opts into a borrowed field-wise copy; `as` names the target; the retired `From[S]` and `to()` remain accepted in v0.1 (X-552). |
| Aspect hooks | `before`, `after`, `on failure`, `metadata`; failure handlers innermost first; no `around`, no body re-execution. |
| Constants | Constant expressions include record/variant-map literals, declaration names and the `Type.of(…)` intrinsics. |
| Variant maps | `VariantMap[E, V]` written `{ Variant: value }`; optional callables need parentheses `(fn(…) R)?`. |
| Explicit type arguments at calls | Not in v1; inferred, or annotate the binding. |
| Associated types | Not in v1; interfaces take generic parameters. |
| Variadic parameters | Not in v1; pass a `ListView[T]`. |
| Blocking in async code | Forbidden; `launchBlocking` takes a synchronous `fn once`. |
| Length-refined collections | Immutable; mutate an unrefined collection and `validate`. |
| Effect clauses | `pure`/`noalloc`/`async`/`try`/`recovers` stay as the signature vocabulary; a full `effects(…)` clause was considered and rejected as noise on every function. Inferred `alloc`, `block`, `nondet` are shown by tooling. |
| Standard package | `fortis` (`fortis.prelude`, `fortis.text`, …); there is no `std`. |

## Appendix C. Lowering reference and the Rust-expressible subset

### C.1 Lowering shapes

Program type names, type parameters and enum variants are encoded into a separate namespace in generated Rust
(X-592). Rust prelude and runtime names reserve no Fortis names. A newtype is lowered through its representation,
including when its name also spells a built-in type.

An integer assignment already accepted by the checker is represented at the destination's machine width
without changing its value (X-593). This representation adjustment requires the source's proved interval to
fit the destination; it does not widen the language's assignability rules.

Each row names which checker is authoritative. **Rust-checked** means `rustc`'s own type and borrow checking independently verifies the lowered shape, and a `rustc` rejection of frontend-accepted code is a compiler defect. **Frontend-checked** means the property has no Rust type; `rustc` acceptance is not evidence, and the IR validator plus the whitelisted Rust subset are the controls.

| Source construct | Native representation | Rust-checked | Frontend-checked |
|---|---|---|---|
| `type` newtype | `struct Name(Repr)` with private field | nominal identity | refinement invariant |
| class | `struct` + `impl` | field privacy | generated-constructor visibility |
| trivial type | `Copy` struct/enum | `Copy` | the trivial table |
| `copy p` | `Clone::clone` of a generated `Clone` impl | | copyable set; allocation |
| read loan | `&T` | loan validity | lexical loan end (frontend is stricter) |
| exclusive loan | `&mut T` | loan validity | field-freezing rule |
| `move p` | move | use after move | |
| `from` return | `fn f<'a>(x: &'a T) -> &'a U` | lifetime | single-origin rule |
| standard view or cursor | `&'a str`, `&'a [T]`, `struct Cursor<'a, T>` owned by its holder (`fn iterate<'a>(&'a self) -> Cursor<'a, T>`, `fn next(&mut self) -> Option<&'a T>`) | lifetime | standard types only: no user type stores a loan (X-137) |
| `Shared[T]` / `Weak[T]` | `Arc<T>` / `Weak<T>` | `Send`/`Sync` once emitted | `retain` visibility |
| `Mutex[T]` | `std::sync::Mutex` wrapper | `Send`/`Sync` | guard-across-await rule |
| interface | trait + generics | trait bounds | effect/refinement substitutability, coherence |
| `dynamic I` | `Box<dyn I>` / `&dyn I` / `Arc<dyn I>` | object safety | dynamic-eligibility, allocation |
| mixin | generated inherent methods | | conflicts, required methods |
| lambda | closure with concrete environment | `Fn`/`FnMut`/`FnOnce` | declared kind is at least the kind the body needs; capture list |
| lambda whose result is a function type | closure returning `Box<dyn Fn…>` (a Rust closure cannot name an `impl Fn` result), so the function it returns is boxed and the lambda allocates (C.3) | `Fn`/`FnMut`/`FnOnce` | as for a lambda |
| `try func` | `Result<T, Failure>` | | linearity (no `Drop` of a live `Failure` is ever emitted; a `#[must_use]` plus an aborting `Drop` is the trusted backstop) |
| `catch` | `match` | | recover/return semantics |
| `T?` | `Option<T>` | | no implicit absence/failure equivalence |
| `match` | `match` | exhaustiveness (both) | ownership of bindings |
| `async func` | `async fn` state machine | | linear futures, cancellation at `await` |
| `scope` | generated join set polled by the parent future | | cancellation and join on exit |
| `launchParallel` | `spawn` on a worker pool with `'static + Send` bounds | `Send` | `Shareable` |
| `defer` | body inlined at every exit path (not `Drop`) | borrow validity of each inlined copy | places initialised at every exit |
| `cleanup` | `impl Drop` calling the method | drop order | effects contributed |
| `noalloc` | no representation | | pattern whitelist (C.3) |
| `pure`, effects | no representation | | summaries |
| `capture`/`record` | explicit frame struct writes and record calls | | definite assignment |
| capability | zero-sized or handle-carrying struct with private constructor | | `io` requires capability |

Generated application crates carry `#![forbid(unsafe_code)]`. Runtime and adapter crates are audited separately.

### C.2 Unsupported shapes

Rejected before lowering, with the listed alternative. A shape that only the lowering can identify is rejected with `E-OWN-090`. A shape that an earlier rule already excludes keeps that rule's more specific code (X-126). No shape is ever lowered with `unsafe` or lifetime erasure.

| Shape | Rejected with | Alternative |
|---|---|---|
| a loan returned `from` two parameters | `E-SYN-000`: a loan type names one origin (7.6) | return an owned value or split |
| a class that stores a loan (a lifetime parameter, or a field of a loan or loan-carrying type) | `E-OWN-053`: stored loans are not in v1 (7.7, X-137) | own, copy, hold a `Shared`, or pass per call |
| a loan stored in a collection, `Shared` or `Mutex` | `E-OWN-053` (7.7, X-605) | own the value |
| a `fn mut` closure escaping into a `Shared` | `E-OWN-090` | `Mutex` around the state and a `fn` closure |
| a future holding a loan launched on a parallel pool | `E-ASY-003`: only `Shareable` environments without loans cross a pool launch (12.7) | pass owned data |
| a self-referential class | `E-OWN-053`: no field stores a loan, of its own storage or of anything else (7.7, X-137) | indices or `Shared` + `Weak` |
| a partial move | `E-OWN-040`: no partial moves (7.2) | move the whole or restructure |

### C.3 Allocation pattern whitelist

A lowering is allocation-free if every operation in it is one of: arithmetic and comparison; field and array access; construction of trivial or inline types (`Array`, `FixedList`, `FixedMap`, bounded `Failure`), and of any class or record from values that are trivial, moved, freshly constructed, returned by a call, or retained (`retain()`, `downgrade()`), since a class's own storage is inline and a heap value's allocation is counted where that value is created (X-137); read and exclusive loans; calls to functions whose summary lacks `alloc`; `match`; `defer` lowering; cooperative `launch` of a future whose state fits the scope's inline storage declared by the runtime host; `capture` into a frame of statically bounded size. Everything else is `alloc`: `List`/`String`/`Map` growth, `Shared.create`, `dynamic` creation, `copy` of heap-owning types, boxed futures from `dynamic` async calls, extended failure payloads, `launchParallel`/`launchBlocking`, a function value returned by a lambda (boxed, C.1), and a cooperative `launch` beyond the host's inline storage. The v1 runtime host declares no inline task storage, so in v1 every cooperative `launch` is `alloc` (X-125); `join(a, b)`, which polls both futures inside the parent, is not.

## Appendix D. Rule index and old requirement IDs

| Old ID (FSPEC-0.1) | New rule |
|---|---|
| SYN-001 | LANG-05-05, LANG-08-01 |
| SYN-002 | LANG-03-03 (case rule replaces symbol-category disambiguation) |
| SYN-003 | PROG-CMP (formatter), see Delivery Programme |
| SYN-004 | LANG-05-07 |
| SYN-005, SYN-006 | PROG-DIA / PROG-CMP, see Delivery Programme |
| TYP-001 | LANG-06-01 |
| TYP-002 | LANG-06-03 |
| TYP-003 | LANG-06-04 |
| TYP-004 | LANG-06-02 |
| TYP-005 | PROG-TST (library metadata), see Delivery Programme |
| TYP-006 | LANG-06-08 |
| OWN-001 | LANG-07-01 |
| OWN-002 | LANG-07-02 |
| OWN-003 | LANG-07-03 |
| OWN-004 | LANG-07-04 |
| OWN-005 | LANG-07-06 |
| OWN-006 | LANG-07-08 |
| FUN-001, FUN-002 | LANG-08-02 |
| OO-001 | LANG-09-01, LANG-09-04 |
| GEN-001 | LANG-06-10 and PROG-CMP (bounded solving) |
| MOD-001 | LANG-04-02 |
| DOC-001 | PROG-TST, see Delivery Programme |
| ERR-001 | LANG-10-01 |
| ERR-002 | LANG-10-02 |
| ERR-003, ERR-004 | LANG-10-03 |
| ERR-005 | LANG-10-04 |
| ERR-006 | LANG-09-05 |
| ERR-007 | PLAT Telemetry / Runtime host (disposition policy) |
| EFF-001 | LANG-11-01 |
| EFF-002 | LANG-11-02 |
| EFF-003 | removed with `nostate` |
| EFF-004 | LANG-11-03 |
| EFF-005 | LANG-11-04 |
| EFF-006 | LANG-11-06 |
| RES-001 | LANG-05-02, LANG-07-07 |
| RES-002 | LANG-12-02 |
| ASY-001, ASY-003 | LANG-12-03 |
| ASY-002 | LANG-12-05 |
| ASY-004 | LANG-12-04 |
| ASY-005 | PLAT Runtime host |
| OBS-001 | LANG-13-01 |
| OBS-002 | LANG-13-02 |
| OBS-003, OBS-004, OBS-005 | PLAT Telemetry |
| OBS-006 | LANG-13-03 |

## Appendix E. Glossary

The terms that name the parts of Fortis and its distribution (PROG-PRD-38, X-150). A term defined by a section of this document is defined there; this table says which, and adds the terms that the rest of the specification uses for what surrounds the language.

| Term | Meaning |
|---|---|
| binding | Two senses, always distinguished by context. A **name binding** is a name introduced by `let`, `let mut`, a parameter, a pattern or a capture (5.1, 7.2). An **embedding binding** is the versioned package through which another language (SQL, HTML, a pattern) gains fenced blocks in Fortis: a run-time library and a verifier (Programme PROG-PRD-30). |
| build mode | How a program is built and run: `dev` (the development VM), `native-dev` and `release` (Programme 5.4). What other toolchains call build profiles. Integer behaviour never depends on the build mode (6.4). |
| declaration head (head) | A reserved keyword that introduces a declaration whose body is validated by a schema and elaborated into ordinary core declarations before type checking, such as `application` or `component` (16). Every head is reserved by the core; `observation` is the core's own (13), `explore` belongs to developer tooling (Programme 9.1), and the others are defined by kits (16). |
| kit | A named, versioned bundle of packages that ships with the distribution and extends Fortis only through libraries, aspects (11.8), declaration heads (16), embedding bindings and verifiers: the HTTP kit, the Data kit, the Web UI kit. A kit never changes the meaning of core code (Programme PROG-FND-07, PROG-PRD-38). The kits and their packages are defined by the Platform Specification. |
| lint level | How strictly the toolchain reports lints: the baseline level and the strict level (4.3, 8.2, 11.3). A lint never changes typing. |
| module | One source file under `src/`; its path is its identity (4.2). |
| package | The unit of distribution, versioning and visibility: a directory with `project.yaml`, a lockfile and `src/` (4.1). Packages are nominal: two versions of one package are two packages (4.1). |
| toolchain component | A tool distributed with the toolchain that is neither the language nor a kit and adds nothing to a program: the daemon, the language server and the SDK generators (Programme PROG-PRD-38). |
| verifier | A package that can only make the check stricter: it reads the checked program and returns records, never code, is pure, deterministic and bounded, and holds no capability (Programme PROG-PRD-36). |
