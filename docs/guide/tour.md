# A tour of Fortis

<!-- covers: LR 1, LR 2 -->
*Human mode. Agent mode: [Overview, by rule](../agent/01-overview.md). Source of truth: LR 1 and LR 2.*

This chapter says what Fortis is for, gets the tools running, and then walks through one whole program, a small
to-do list, pointing to the chapter that explains each piece. You do not need to understand every line on first
reading; the point is to see how the pieces fit before the later chapters take them one at a time.

## What Fortis is for

Fortis is a general-purpose, statically checked, compiled language designed so that software written mostly by
AI agents can be read, reviewed and trusted by people (LR 1). Its reader is someone looking at one file, or one
diff, who needs to answer a handful of questions without running anything:

- who owns this value, and can this call change it?
- can this call fail, and who handles the failure?
- what effects does this function have: does it allocate, do I/O, read the clock?
- when is this observation recorded?

Each of those questions has a visible answer in the source (LR 1). The answer for a *local* decision is always on
the page: whether a value is copied, lent or moved, whether a failure is propagated or recovered, which effects a
function claims. Facts that live elsewhere, such as what an imported function ultimately does, come from the
imported function's signature, and the language guarantees that a signature can never claim less than its body
does (LR 1).

Fortis is library-first. A program is a set of packages, a package is a set of modules, and a module is one file
(LR 1). HTTP servers, databases, web interfaces and telemetry are ordinary libraries, grouped into **kits**, that
an application imports; nothing in the core language depends on them (LR 1).

## Six design choices

Everything else in the language follows from a few decisions (LR 1).

- **Explicit ownership.** When you assign or pass an existing value that owns resources, you write `copy`,
  `alias` or `move`; a mutable borrow is a visible `editable`. Nothing is cloned or reference-counted behind your
  back. See [Ownership](07-ownership.md).
- **Three distinct answers.** An optional value (`T?`), a `Bool` and a fallible outcome (`Outcome[T]`) are three
  different types with three different operators (`??`, `||`, `or`). Nothing is "truthy". See
  [Types](06-types.md).
- **Linear failures.** The result of a call that can fail must be dealt with exactly once: propagated with
  `or fail`, replaced with `or recover`, handled with `catch` or `match`, or reported with `report`. Ignoring it
  does not compile. See [Outcomes and failures](10-outcomes.md).
- **A closed set of effects.** `pure`, `noalloc`, `async` and `try` are checked facts about a function, computed
  from a fixed set of eight effects; a named contract is a combination of them and nothing else. See
  [Effects and contracts](11-effects.md).
- **Lexical loans and synchronous cleanup.** A borrow lasts until the end of the block that holds it; `defer`
  runs when a block exits; asynchronous code has one place where it can be suspended or cancelled (`await`) and
  structured `scope`s that never leak a task. See [Cleanup, async and concurrency](12-async.md).
- **Two backends, one meaning.** A development VM and a production backend that emits safe Rust run the same
  checked intermediate representation (LR 1). The reference toolchain adds a third implementation, the
  definitional interpreter, which every example in this guide also runs on.

What Fortis leaves out is as deliberate: there is no runtime reflection, no inheritance, no macros, no
exceptions and no implicit conversions (LR 1). A reader can work out every binding's type, ownership and effects
from the file and the signatures it imports.

## Notation that earns its place

Fortis puts more words on the page than most languages, so every symbol has to justify itself. The rule is that a
symbol or keyword exists only if it changes what a reader may conclude about ownership, mutation, failure,
effects or timing (LR 1.1). `@` only ever applies an aspect; `?` only ever means "optional", never "propagate the
error"; `=` only assigns, and `==` compares; `=>` is an expression body and nothing else (LR 1.1).

Keywords that might look like ceremony each carry a decision you would otherwise have to infer from somewhere you
cannot see (LR 1.1):

| You read | It tells you |
|---|---|
| `let` / `let mut` | a new name / a name that will change |
| `copy`, `alias`, `move`, `editable` | how this place is used: duplicated, lent, given away, lent for writing |
| `owned`, `editable` on a parameter | what the callee may do with the argument |
| `try`, `pure`, `noalloc`, `async`, `recovers` | what the function may do |
| `or fail`, `or recover`, `catch` | what happens to a failure |
| `give` | this block produces a value |
| `export` | visible outside the module |

Silence has exactly one meaning in each position (LR 1.1). An argument with no marker is a read-only loan that
ends when the call returns, or, for an `async` call, when the future the call produces completes (LR 7.3); a
`for` loop over a place borrows it read-only for the loop; an assignment or `return` with no marker is a fresh
value or a trivially copyable one. So a line with no markers neither changes nor transfers anything, and keeps
no loan beyond the call or the future it is in.

Two costs are accepted on purpose (LR 1.1). Because `or` binds loosest, `(f() or fail).amount` needs its
parentheses, and the idiom is to put the failure decision on its own line: `let v = f() or fail`. And an `async`
call looks like any call: it builds a future, and the `await` that runs it is the visible point.

Finally, the formatter prints every inferred fact back in canonical form, so a formatted file shows the closure
kinds, loan modes and modifiers the checker used (LR 1.1). You will see that in the program below.

## Getting started

<!-- no-citation: how to build and run the tools; the tools are described in Programme 5.4 and 9.2, cited below -->

The reference toolchain lives in this repository. Build the production compiler once (it needs a Rust
toolchain, and no network once the sources are present):

```sh
cd compiler && cargo build --release
```

Then use the `fortis` command from the repository root (`bin/fortis`, or `fortis` once it is on your `PATH`):

| Command | What it does |
|---|---|
| `fortis check FILE.ft` or `fortis check DIR` | every static check: syntax, names, types, ownership, obligations, effects, contracts |
| `fortis run FILE.ft [ARGS…]` | check, then run `main` on the definitional interpreter, passing `ARGS` to it |
| `fortis test FILE.ft` | check, then run the module's tests |
| `fortis build FILE.ft --out APP` | check, then compile to a native executable through safe Rust |
| `fortis view PATH` | read Fortis source as the compiler sees it (`--decls`, `--symbol NAME`, `--query Q`) |
| `fortis fmt --check PATH` / `--write PATH` | the canonical layout |

A directory with a `project.yaml` is a package, and every command accepts it in place of a file. The production
compiler `fortisc`, which `fortis build`, `view` and `fmt` run for you, also runs programs on the development VM:
`fortisc vm FILE.ft` runs `main` there (it prints the program's output lines joined by `|`, in the form the
conformance tools compare), and `fortisc test FILE.ft` runs the tests there.

Every finding and every runtime report is a record: text when the output is a terminal, one JSON object per
line when it is a pipe, and `--format json` or `--format text` chooses explicitly (Programme 7.4).

### Hello, world

```fortis
// accept
import fortis.host { Host }

export func main(args: List[String]) Void {
  let caps = Host.capabilities()
  caps.console.print("hello, world")
  caps.console.print("${args.count()} arguments")
}
```

```output
hello, world
0 arguments
```

Save it as `hello.ft` and run `fortis run hello.ft`, or `fortis build hello.ft --out hello` and then `./hello`.
Words after the file are `main`'s arguments: `fortis run hello.ft one two` prints `2 arguments`. A program's
files are read through the `FileSystem` capability, from a fresh empty directory for each run unless
`FORTIS_DEV_ROOT` names one (relative to where you run the command); keep a program's input files in a
directory of their own and run it as `FORTIS_DEV_ROOT=data fortis run app` ([The core runtime](15-runtime.md)).
While you iterate on tests, `fortisc test` on the development VM is much faster than the interpreter's
`fortis test` ([Tests](14-tests.md)).
Three things already show the language's character:

- `main` is an ordinary exported function in the entry module, taking the command-line arguments (LR 4.6).
- Printing is input/output, and input/output needs authority: `Host.capabilities()` hands `main` the process's
  capabilities, once, and `caps.console` is the one that writes to standard output (LR 11.6). A function that
  wants to print must be given a `Console`; it cannot reach for one.
- `"${args.count()} arguments"` is string interpolation; the interpolated value must be displayable (LR 3.6).

## A whole program

Here is a complete module: a tiny to-do list with an identifier type, a state enum, a declared failure, an
interface, a mixin, a class, a closure, a deferred block, a fallible constructor and a test. It is the guide's
counterpart of the invoicing module in LR 2, written to run on its own.

```fortis
// accept
import fortis.host { Console, Host }

// A nominal identifier: an Int that is never confused with any other Int.
export type TodoId = Int<1..>

// A closed set of states, two of them with payloads.
export enum Status derive Equatable {
  Open
  Done(by: String)
  Dropped(reason: String)
}

// A failure kind with a stable code and a typed payload.
export error EmptyTitle {
  code: "todo.empty_title"
  payload { id: TodoId }
  message: "to-do ${id} has an empty title"
}

// Anything that can describe itself in one line.
export interface Describable {
  pure func describe(self) String
}

// A mixin adds methods on top of methods the class provides.
export mixin Labelled {
  requires pure func title(self) String
  export pure func label(self) String => "[${self.title()}]"
}

export class Todo implements Describable with Labelled {
  export id: TodoId
  name: String
  status: Status

  export pure try func create(id: TodoId, name: owned String) Todo {
    if name.byteCount() == 0 {
      fail EmptyTitle(id: id)
    }
    return Todo(id: id, name: move name, status: Status.Open)
  }

  export pure func title(self) String => copy self.name

  export pure func finish(editable self, by: StringView) Void {
    self.status = Status.Done(by: String.from(by))
  }

  export pure func isOpen(self) Bool => self.status == Status.Open

  export pure func describe(self) String {
    let phase = match self.status {
      Open => "open"
      Done(by: who) => "done by ${who}"
      Dropped(reason: why) => "dropped: ${why}"
    }
    return "${self.id} ${self.label()} ${phase}"
  }
}

export pure func countOpen(todos: List[Todo]) Int {
  let open = pure noalloc fn(t: Todo) Bool => t.isOpen()
  return todos.countWhere(open)
}

func show(console: Console, todos: List[Todo]) Void {
  defer {
    console.print("${countOpen(todos)} open")
  }
  for t in todos {
    console.print(t.describe())
  }
}

export try func main(args: List[String]) Void {
  let caps = Host.capabilities()
  let mut first = Todo.create(id: TodoId(1), name: "write the guide") or fail
  let second = Todo.create(id: TodoId(2), name: "check the examples") or fail
  editable first.finish("ada")
  let todos = List.of(move first, move second)
  show(caps.console, todos)
  let fallback = Todo.create(id: TodoId(3), name: "") catch e {
    report e
    recover Todo.create(id: TodoId(3), name: "untitled") or fail
  }
  caps.console.print(fallback.describe())
}

test "a new to-do is open" {
  let t = Todo.create(id: TodoId(9), name: "x") or fail
  expect t.isOpen()
}
```

```output
1 [write the guide] done by ada
2 [check the examples] open
1 open
3 [untitled] open
```

The `report e` line also writes one failure record, with the code `todo.empty_title` and the payload `id: 3`, to
standard error; it is not part of the program's output (LR 10.6). Now the pieces, top to bottom.

**Imports.** `import fortis.host { Console, Host }` binds two names from the standard module `fortis.host`. The
prelude (`Int`, `String`, `List`, `Bool`, …) needs no import (LR 4.3). See [Modules and packages](04-modules.md).

**A newtype.** `type TodoId = Int<1..>` declares a new nominal type whose values are integers of at least 1. A
`TodoId` never mixes with any other `Int`-based type, and the only ways to make one are a literal the checker can
prove in range, written `TodoId(1)`, or the fallible `TodoId.validate(n)` (LR 6.9). See [Types](06-types.md).

**An enum with payloads.** `Status` has three variants, two of which carry data. `derive Equatable` generates `==`
for it (LR 6.10). A `match` over a `Status` must handle every variant, so adding a fourth variant later makes the
checker point at every `match` that needs updating (LR 9.7, LR 9.8). See
[Classes, interfaces, mixins and enums](09-classes.md).

**A declared failure.** `error EmptyTitle` defines a failure kind with a stable code, a typed payload and a message
template; `fail EmptyTitle(id: id)` creates one (LR 10.3). See [Outcomes and failures](10-outcomes.md).

**An interface and a mixin.** `Describable` declares a method a class promises to provide. `Labelled` provides a
method, `label`, on top of a method it requires the class to have, `title`; a mixin adds no fields and sees only
the class's exported members, so it never reads a private field (LR 9.5, LR 9.6).

**A class.** `Todo` has an exported `id` and two private fields. Its methods show the three receiver forms you will
meet everywhere: `self` borrows read-only, `editable self` borrows for writing, and a call to an `editable`
method is written `editable first.finish("ada")`, so the call site shows that `first` changes (LR 9.3). The
generated constructor `Todo(id: …, name: …, status: …)` is usable only where every field is visible, which is
why other modules must go through `Todo.create` (LR 9.2).

**A fallible constructor.** `pure try func create(…) Todo` may fail; its callers receive an outcome that they must
consume. `name: owned String` says the function takes ownership of the string, so it can store it with
`move name` (LR 7.6). `copy self.name` in `title` duplicates a string that `self` only lends (LR 7.6).

**A closure.** `pure noalloc fn(t: Todo) Bool => t.isOpen()` is a lambda with no captures. You may write it as
`fn(t: Todo) Bool => t.isOpen()`; the formatter prints back the `pure noalloc` it infers, so the file shows what
the checker knows (LR 8.2, LR 1.1). `countWhere` accepts it because it is pure (LR 6.7). See
[Functions and closures](08-functions.md).

**Effects and authority.** `countOpen` is `pure`: it does no I/O and changes nothing outside itself. `show` prints,
so it takes a `Console` parameter; that is how a reader knows, from the signature alone, that it does I/O
(LR 11.6). See [Effects and contracts](11-effects.md).

**A deferred block.** The `defer` in `show` runs when the function's body exits, after the loop, by whatever route
it leaves (LR 12.1). See [Cleanup, async and concurrency](12-async.md).

**Failures, handled three ways.** In `main`, `Todo.create(…) or fail` propagates a failure to `main`'s caller,
which for a `try main` means one report and exit status 1 (LR 4.6). The `catch e { … }` block handles the empty
title: it reports the failure and recovers with a replacement value (LR 10.4). Nothing can be ignored: deleting
the `or fail` would leave an outcome unconsumed, which does not compile (LR 10.1).

**Ownership at the call site.** `List.of(move first, move second)` moves both values into the list; after that,
`first` and `second` cannot be used (LR 7.4). `show(caps.console, todos)` passes both arguments with no marker,
which means read-only loans that end when `show` returns (LR 7.5).

**A test.** `test "…" { … }` is a declaration like any other. Its body may use `or fail`, and `expect` records a
check (LR 14). `fortis test` runs it. See [Tests](14-tests.md).

## Where to go next

<!-- no-citation: a reading list -->

Read the chapters in order, starting with [Lexical structure](03-lexical.md); each follows one section of the LR.
Some everyday words are keywords and cannot name a variable, a parameter or a field: `before`, `after`, `from`,
`once`, `owned`, `error`, `record`, `test`, `cases`, `view`, `state`, `model` and `action` among them, all
listed in [Keywords](appendix-a-keywords.md) (LR 3.4). The [core library](../agent/library.md) lists every
operation of the standard modules, generated from their declarations.
If you already know Rust, TypeScript or Python, the notes for readers coming from [Rust](from-rust.md),
[TypeScript](from-typescript.md) and [Python](from-python.md) map what you know onto Fortis. The
[agent mode](../agent/README.md) states the same rules densely, by rule ID.
