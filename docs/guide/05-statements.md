# Statements and expressions

<!-- covers: LR 5 -->
*Human mode. Agent mode: [Statements and expressions, by rule](../agent/05-statements.md). Source of truth: LR 5.*

A Fortis function body is a sequence of statements in blocks. This chapter covers bindings and assignment,
conditionals, loops, the four ways to leave a block with a value or a failure, operators and the order in which
everything is evaluated. Most of it will look familiar; the differences are deliberate, and each one exists so
that a reader can tell from the line itself what it does.

## Bindings and assignment

`let` introduces a name; `let mut` introduces a name that may be reassigned later; `=` on its own only ever
reassigns (LR 5.1). The type comes from the initialiser, or from an annotation when you want a different one.

```fortis
// accept
import fortis.host { Host }

export func main(args: List[String]) Void {
  let caps = Host.capabilities()
  let greeting = "hello"
  let mut count: Int<0..10> = 0
  count = 3
  caps.console.print("${greeting}, count is ${count}")
}
```

```output
hello, count is 3
```

There is no `let` without an initialiser, and assignment is a statement, not an expression, so it never chains
(LR 5.1). Assigning to a name that was never declared is `E-NAM-001`, and assigning to an immutable binding is
`E-NAM-002`; both are reported at the assignment (LR 5.1):

```fortis
// reject: E-NAM-001, E-NAM-002
export func main(args: List[String]) Void {
  let total = 1
  missing = 3        // reject: E-NAM-001
  total = 2          // reject: E-NAM-002
}
```

A name may not shadow a binding that is still visible, even in an inner block: `E-NAM-003` (LR 4.5). Pick a new
name instead; a reader then never has to work out which of two `total`s a line means.

```fortis
// reject: E-NAM-003
export func main(args: List[String]) Void {
  let total = 1
  {
    let total = 2    // reject: E-NAM-003
  }
}
```

An inner `{ … }` block on its own is a statement. Its purpose is to end loans and drop values early (LR 5.1,
LR 7.3): everything declared inside it is gone at its closing brace.

### Constants

A module may declare constants with `const`. A constant's initialiser is a constant expression: literals, other
constants, arithmetic over them, enum values, record literals and a few constructors such as `List.of` with
constant arguments (LR 5.1). Constants are built once, before any of your code runs, and are used like any
other value.

```fortis
// accept
import fortis.host { Host }

const limit = 3
const names = List.of("ada", "brian", "carla")

export func main(args: List[String]) Void {
  let caps = Host.capabilities()
  caps.console.print("limit ${limit}, ${names.count()} names")
}
```

```output
limit 3, 3 names
```

## Conditionals

The condition of an `if` (and of a `while`) must be a `Bool`. Nothing else is accepted: there is no
truthiness, so an `Int`, an optional or a string in a condition is `E-TYP-010` (LR 5.2).

```fortis
// reject: E-TYP-010
export func main(args: List[String]) Void {
  let count = 3
  if count {          // reject: E-TYP-010
  }
}
```

`if` is a statement. It is also an expression when it has an `else` and every branch ends with `give value`; the
branches must agree on one type (LR 5.2).

```fortis
// accept
import fortis.host { Host }

export func main(args: List[String]) Void {
  let caps = Host.capabilities()
  let count = 0
  let label = if count == 0 { give "none" } else { give "some" }
  caps.console.print(label)
}
```

```output
none
```

A branch can also tell the checker something. Inside `if count < 10 { … }` the checker knows the bound, and in
the `else` branch it knows the opposite (LR 5.2). The chapter on [proofs](06-proofs.md) explains what the checker
does with such facts.

## Loops

There are three loops: `while`, `for … in …` and `loop` (LR 5.3).

```fortis
// accept
import fortis.host { Host }

export func main(args: List[String]) Void {
  let caps = Host.capabilities()
  for i in 1..3 {
    caps.console.print("inclusive ${i}")
  }
  for i in 0..<2 {
    caps.console.print("exclusive ${i}")
  }
  let words = List.of("red", "green")
  for word in words {
    caps.console.print(word)
  }
}
```

```output
inclusive 1
inclusive 2
inclusive 3
exclusive 0
exclusive 1
red
green
```

`a..b` includes `b` and `a..<b` stops before it. Ranges exist only in `for` headers and in refinement types, so
`1..3` is not a value you can store (LR 5.6).

`for word in words` borrows `words` for the whole loop and gives each element as a read loan; the list is
untouched afterwards. `for word in move words` would consume the list instead and give each element by value
(LR 5.3). The loop variable is immutable: it names the current element, so assigning to it means nothing.

`loop` runs until a `break`. `break` and `continue` apply to the innermost loop, or to a labelled one
(`outer: for …` and then `break outer`). A label is a lowercase name and a colon in front of the loop, and
`break` carries no value (LR 5.3).

```fortis
// accept
import fortis.host { Host }

export func main(args: List[String]) Void {
  let caps = Host.capabilities()
  let mut n: Int<0..100> = 0
  loop {
    if n >= 3 {
      break
    }
    n = n + 1
  }
  caps.console.print("stopped at ${n}")
  outer: for row in 1..3 {
    for column in 1..3 {
      if column == 2 {
        continue outer
      }
      caps.console.print("${row},${column}")
    }
  }
}
```

```output
stopped at 3
1,1
2,1
3,1
```

`n = n + 1` is accepted here because the checker can prove it cannot overflow: inside the loop `n` is below 3,
so `n + 1` fits `Int<0..100>`. Integer arithmetic always needs such a proof, or one of the checked operations
(LR 6.4); [Types](06-types.md) shows both.

## Leaving a block: `return`, `fail`, `give` and `recover`

Four statements end something with a value (LR 5.4):

| Statement | Ends | With |
|---|---|---|
| `return v` | the enclosing function (or lambda) | a success value |
| `fail f` | the enclosing `try` function | a failure |
| `give v` | the enclosing `match` arm block or `if` expression branch | the branch's value |
| `recover v` | the enclosing `catch` block | a replacement success value |

`return` and `fail` always mean the function you are reading, never a block or a loop, and inside a lambda they
mean the lambda (LR 5.4). A function whose return type is not `Void` must end every path in `return`, `fail` or
a call that never returns; there is no implicit last value. Falling off the end is `E-TYP-110`, reported at the
function (LR 5.4):

```fortis
// reject: E-TYP-110
pure func sign(n: Int) Int {        // reject: E-TYP-110
  if n < 0 {
    return -1
  }
}

export func main(args: List[String]) Void {
}
```

`give` outside an `if` expression or a `match` arm, `recover` outside a `catch` block, and `break` or `continue`
outside a loop are `E-TYP-120` (LR 5.4). `fail` outside a `try` function is `E-ERR-001` (LR 5.4):

```fortis
// reject: E-ERR-001
export error TooLarge {
  code: "demo.too_large"
}

func check(n: Int) Void {
  if n > 100 {
    fail TooLarge()   // reject: E-ERR-001
  }
}

export func main(args: List[String]) Void {
  check(5)
}
```

Declaring the function `try func check(n: Int) Void` makes the `fail` legal; callers must then handle the
outcome, which is the subject of [Outcomes and failures](10-outcomes.md).

## Expression statements and `discard`

An expression used as a statement must have type `Void` or `Never`. A value you compute and then ignore is
`E-TYP-140`; if ignoring it is intended, say so with `discard` (LR 5.5):

```fortis
// reject: E-TYP-140
pure func double(n: Int<0..1000>) Int => n * 2

export func main(args: List[String]) Void {
  double(4)           // reject: E-TYP-140
}
```

```fortis
// accept
pure func double(n: Int<0..1000>) Int => n * 2

export func main(args: List[String]) Void {
  discard double(4)
}
```

`discard` is not a way out of an obligation: an `Outcome`, a `Task` or any other linear value cannot be
discarded (`E-OBL-001`, LR 5.5). Those must be consumed, as later chapters show. The single exception is a
future that was never started and holds nothing linear and no exclusive loan: discarding it is allowed, and the
runtime records that it never ran (LR 5.5, LR 12.3).

## Operators

All binary operators are left-associative unless the table says otherwise; higher rows bind tighter (LR 5.6).

| Level | Operators | Notes |
|---|---|---|
| 1 | `a.b` `a.b(…)` `a[i]` `a(…)` `a?.b` | member access, calls, indexing, optional chaining |
| 2 | `-a` `!a` `await a` and the markers `copy` `alias` `editable` `move` | unary, right-associative |
| 4 | `*` `/` `%` | integer operands need proof |
| 5 | `+` `-` | |
| 6 | `<<` `>>` | the shift amount must be proved in range |
| 7 | `<` `<=` `>` `>=` | comparisons do not chain |
| 8 | `==` `!=` | |
| 9, 10, 11 | `&`, `^`, `\|` | bitwise and, xor, or |
| 12, 13 | `&&`, `\|\|` | short-circuit |
| 14 | `a ?? b` | absent-value fallback, right-associative |
| 15 | `a or fail`, `a or recover b`, `a catch e { … }` | outcome handling, right-associative |
| 16 | `a..b` `a..<b` | ranges, in `for` headers and refinements only |

Three families never mix without parentheses: the outcome operators, `??`, and the Boolean short-circuit
operators (LR 5.6). `a || b ?? c` could mean two different things, so it is rejected (`E-SYN-004`), and you write
the parentheses that say which one you mean:

```fortis
// reject: E-SYN-004
export func main(args: List[String]) Void {
  let a = false
  let b: Bool? = null
  let c = true
  let d = a || b ?? c          // reject: E-SYN-004
}
```

```fortis
// accept
import fortis.host { Host }

export func main(args: List[String]) Void {
  let caps = Host.capabilities()
  let a = false
  let b: Bool? = null
  let c = true
  let d = a || (b ?? c)
  caps.console.print("${d}")
}
```

```output
true
```

`or fail` ends an expression, so `f() or fail ?? 0` is not a sentence of the grammar at all (`E-SYN-000`), and
`await f() or fail` means `(await f()) or fail` (LR 5.6). Because `or` binds loosest, the idiom is to put the
failure decision on its own line: `let v = f() or fail`, then use `v` (LR 1.1).

There are no user-defined operators and no operator overloading; `==` and the comparisons work through the
built-in interfaces `Equatable` and `Comparable` (LR 6.10).

## Evaluation order

Operands and arguments are evaluated left to right, exactly once, and the receiver of a method call before its
arguments (LR 5.7). `&&` and `||` evaluate their right operand only when they need it.

```fortis
// accept
import fortis.host { Console, Host }

func note(console: Console, label: StringView, value: Int) Int {
  console.print("evaluating ${label}")
  return value
}

func pair(a: Int, b: Int) Int => a

export func main(args: List[String]) Void {
  let caps = Host.capabilities()
  let first = pair(note(caps.console, "left", 1), note(caps.console, "right", 2))
  caps.console.print("got ${first}")
}
```

```output
evaluating left
evaluating right
got 1
```

A call runs in four steps (LR 5.7). First every argument is evaluated, and an argument that names a place
*reserves* the loan or move it needs without taking it yet. Then all reservations become active at once. Then
the callee runs. Finally the loans taken for the call end when it returns, unless the result carries them.

The second step is why some calls that look reasonable are rejected. Here the first argument reserves an
exclusive loan of `buffer`, and the second argument's evaluation reads `buffer` while that reservation is
pending; the read finishes first, but then the move in the second call breaks the reservation (`E-OWN-012`,
LR 5.7):

```fortis
// reject: E-OWN-012
class Buffer {
  export bytes: Int
}

func update(target: editable Buffer, size: Int) Void {
  target.bytes = size
}

func consume(b: owned Buffer) Int => b.bytes

export func main(args: List[String]) Void {
  let mut buffer = Buffer(bytes: 0)
  update(editable buffer, consume(move buffer))   // reject: E-OWN-012
}
```

Evaluate the argument first, on its own line, and pass the result:

```fortis
// accept
import fortis.host { Host }

class Buffer {
  export bytes: Int
}

func update(target: editable Buffer, size: Int) Void {
  target.bytes = size
}

func size(b: Buffer) Int => 7

export func main(args: List[String]) Void {
  let caps = Host.capabilities()
  let mut buffer = Buffer(bytes: 0)
  let n = size(buffer)
  update(editable buffer, n)
  caps.console.print("${buffer.bytes}")
}
```

```output
7
```

Temporaries created while evaluating a statement are dropped at the end of that statement, in reverse order of
creation (LR 5.7).

## Calls, named arguments and defaults

Arguments are positional or named (`name: value`). Positional arguments come first, in declaration order; named
ones follow in any order, and each parameter is bound once (LR 5.8). A parameter may have a default: a pure,
non-allocating constant expression, evaluated afresh at every call that leaves the argument out (LR 5.8). That is
stricter than a module constant, whose initialiser may build a list with `List.of`, because a default runs at
every such call (LR 5.1, LR 5.8).

```fortis
// accept
import fortis.host { Host }

pure func page(items: Int<0..1000>, size: Int<1..100> = 20) Int<0..1000> {
  if items < size {
    return items
  }
  return size
}

export func main(args: List[String]) Void {
  let caps = Host.capabilities()
  caps.console.print("${page(50)} ${page(50, size: 10)} ${page(size: 5, items: 3)}")
}
```

```output
20 10 3
```

A positional argument after a named one, a name that is not a parameter, a parameter bound twice and too many
arguments are all `E-TYP-130` (LR 5.8):

```fortis
// reject: E-TYP-130
pure func area(width: Int<0..100>, height: Int<0..100>) Int => width * height

export func main(args: List[String]) Void {
  discard area(width: 2, depth: 3)      // reject: E-TYP-130
}
```

A parameter with no default that no argument binds is `E-TYP-050`, the code that also reports a constructor call
missing a field (LR 5.8):

```fortis
// reject: E-TYP-050
pure func area(width: Int<0..100>, height: Int<0..100>) Int => width * height

export func main(args: List[String]) Void {
  discard area(width: 2)                // reject: E-TYP-050
}
```

Parameter names are part of a function's public signature: renaming an exported parameter breaks its callers
(LR 5.8). User functions have no variadic parameters; a function that takes many values takes a `ListView[T]`,
and the caller passes `List.of(…)` or an existing list (LR 5.8).
