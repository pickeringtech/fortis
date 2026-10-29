# Modules and packages

<!-- covers: LR 4 -->
*Human mode. Agent mode: [Modules and packages, by rule](../agent/04-modules.md). Source of truth: LR 4.*

> Preview: dependencies and lock files (LR 4.1) arrive in v0.2. An async entry point (LR 4.6, LR 12.3) arrives in v0.3. One package, its modules, imports, visibility and the ordinary entry point (LR 4.1, 4.2, 4.3, 4.4, 4.5 and 4.6) are in v0.0.1.

A Fortis program is a set of packages; a package is a set of modules; a module is one file (LR 1). This chapter
covers how those three fit together: the manifest that names a package, how a file's path becomes its module's
name, how imports find things, who can see what, and where a program starts.

## Packages and their manifest

A **package** is the unit of distribution, versioning and visibility: a directory holding a manifest,
`project.yaml`, a lockfile and a `src/` directory (LR 4.1). The manifest names the package, its version, its
dependencies, which of its modules other packages may import (`exports:`) and, for an application, the module
where it starts (`entry:`):

```yaml
# illustrative
schema: fortis.project/v1
name: acme.billing
version: 1.4.0
exports: [invoices, money]
dependencies:
  fortis: "1.0"
```

Packages are nominal: two versions of one package in the same build are two different packages, and their types
never interchange (LR 4.1). A dependency on a standard package (`fortis` or `fortis.…`) must name a package of
the published compatibility set at the set's version, because the standard packages and kits are tested
together as a set (LR 4.1).

The lockfile, `fortis.lock`, written by `fortis lock`, pins everything a build depends on: the root package, the
digests of the compiler, standard library and Rust toolchain, and the identity and content digest of every
package the manifests reach. When a package has a lockfile, resolution uses only it and fetches nothing. A
dependency the lockfile does not pin, a pin nothing uses, or an unreadable lockfile is `E-NAM-020`; a digest that
differs from the pinned one is `E-NAM-021` (LR 4.1).

A few more checks guard the edges of a build (LR 4.1):

| Code | When | LR |
|---|---|---|
| `E-NAM-030` | `fortis check --target T` or `fortis build --target T` finds an import of a standard module the target's feature set lacks | LR 4.1 |
| `E-NAM-031` | a dependency names a standard package outside the compatibility set, or at another version | LR 4.1 |
| `E-NAM-032` | a native build meets a package that ships a Cargo build script | LR 4.1 |
| `E-LIM-003`, `E-LIM-004`, `E-LIM-005` | a manifest over 1 MiB or 10,000 lines, a lockfile over 8 MiB or 100,000 lines, or JSON the toolchain reads over 256 MiB or nested more than 128 deep | LR 4.1 |
| `E-INT-001`, `E-INT-002`, `E-INT-003` | the toolchain itself failed: a checker defect (its record names a minimised reproduction), generated Rust that does not compile, or a native tool that is missing or fails | LR 4.1 |

None of these is a crash: a failure inside the toolchain is always a record (LR 4.1).

## Modules

A **module** is one source file under `src/`. Its name is its path relative to `src/`, with `/` replaced by `.`
and the extension dropped: `src/orders.ft` is `orders` and `src/pricing/tax.ft` is `pricing.tax`. A directory is
not itself a module, and a module has no header line declaring its name (LR 4.2). Other packages import it as the
package name followed by the module path: `guide.shop.pricing.tax`.

A module contains only declarations and `const` bindings whose initialisers are compile-time constants (LR 4.2).
There is no module initialisation code, so importing a module never runs anything, and the order in which modules
load can never change what a program does. A statement at the top level is therefore a syntax error:

```fortis
// reject: E-SYN-000
func main() {
}

let counter = 0                 // reject: E-SYN-000
```

## A package, file by file

Here is a small application package, `guide.shop`, with three modules, `app`, `orders` and `pricing.tax`,
named by their paths under `src/` (LR 4.2). Each block is one file:

```yaml
# file: guide.shop/project.yaml
schema: fortis.project/v1
name: guide.shop
version: 1.0.0
entry: app
exports: [orders]
dependencies:
  fortis: "1.0"
```

```fortis
// accept
// file: guide.shop/src/pricing/tax.ft
// Tax at 20%, in whole pennies, rounded down.
export pure func withTax(pennies: Int<0..100000>) Int => pennies + pennies / 5
```

```fortis
// accept
// file: guide.shop/src/orders.ft
import guide.shop.pricing.tax { withTax }

// An order line. Every field is exported, so other modules may build one.
export class Line {
  export item: String
  export pennies: Int<0..100000>
}

export pure func total(lines: List[Line]) Int {
  let mut sum = 0
  for line in lines {
    sum = Int.saturatingAdd(sum, withTax(line.pennies))
  }
  return sum
}
```

```fortis
// accept
// file: guide.shop/src/app.ft
import fortis.host { Console }
import guide.shop.orders { Line, total }
import guide.shop.pricing.tax as tax

func main(console: Console) {
  let lines = List.of(Line(item: "tea", pennies: 250), Line(item: "cake", pennies: 300))
  console.print("total ${total(lines)} pennies")
  console.print("one cake ${tax.withTax(300)}")
}
```

```output
total 660 pennies
one cake 360
```

Run it with `fortis run guide.shop` (the package's directory), or build it with `fortis build guide.shop`.

## Imports

An `import` names a module by its full path, and imports appear only at the top of a module, before any
declaration (LR 4.3). There are three forms:

| Form | Binds |
|---|---|
| `import guide.shop.orders` | the alias `orders`; members are used as `orders.total(…)` |
| `import guide.shop.pricing.tax as tax` | the alias `tax` |
| `import guide.shop.orders { Line, total }` | the listed members, directly, and nothing else |

The path is split into a package and a module by the manifest: the longest prefix that names the current package
or one of its declared dependencies is the package, and the rest is the module path (LR 4.3). That is why the
modules of `guide.shop` import each other as `guide.shop.orders` and not as `orders`: a bare `orders` names no
package. Inside a package, any module may import any of its siblings (LR 4.3).

The standard package is `fortis`. Its module `fortis.prelude` is imported into every module automatically and
provides the core types (`Int`, `String`, `List`, `Map`, `Outcome`, …) and the built-in interfaces; every other
standard module (`fortis.text`, `fortis.collections`, `fortis.numeric`, `fortis.time`, `fortis.money`,
`fortis.host`) needs an import (LR 4.3).

An import after a declaration is a syntax error (LR 4.3):

```fortis
// reject: E-SYN-000
import fortis.host { Host }

func main() {
}

import fortis.text                // reject: E-SYN-000
```

A name imported into a module must not collide with a declaration of that module (LR 4.3). The LR names no
code for this yet, so read the record's message (X-152).

The LR also says that importing the same name twice is an error, and that an unused import is a warning under the
baseline lint level and an error under the strict one (LR 4.3).

### A package's root module

A package may name a **root module** in its manifest (`root: core`); an import of the bare package name then
denotes that module (LR 4.3). Here `guide.units` is a library whose root module is `core`, and `guide.convert`
depends on it:

```yaml
# file: guide.units/project.yaml
schema: fortis.project/v1
name: guide.units
version: 2.1.0
root: core
exports: [core]
dependencies:
  fortis: "1.0"
```

```fortis
// accept
// file: guide.units/src/core.ft
export pure func metres(feet: Int<0..10000>) Int => feet * 3048 / 10000

test "ten feet is three metres, rounded down" {
  expect metres(10) == 3
}
```

```yaml
# file: guide.convert/project.yaml
schema: fortis.project/v1
name: guide.convert
version: 1.0.0
entry: main
exports: []
dependencies:
  fortis: "1.0"
  guide.units: "2.1.0"
```

```fortis
// accept
// file: guide.convert/src/main.ft
import fortis.host { Console }
import guide.units { metres }

func main(console: Console) {
  console.print("100 feet is ${metres(100)} metres")
}
```

```output
100 feet is 30 metres
```

Without a root module, an import of the bare package name is an error, and a set of dependencies whose names
could split an import path two ways is rejected as a whole (`E-NAM-010`, LR 4.3).

## Visibility

There is one visibility modifier, `export` (LR 4.4). A declaration without it is private to its module. An
exported declaration is visible to every module that imports its module, but whether it is visible *outside the
package* is decided by the manifest: only the modules listed in `exports:` can be imported by another package, so
an `export` in an unlisted module reaches its siblings only (LR 4.3, LR 4.4).

In `guide.shop` above, `orders` is listed and `pricing.tax` is not, so another package may import `orders` but
not `pricing.tax`; the import is refused, with a record naming the modules that are exported. The LR names no
code for this refusal yet (LR 4.3, X-152).

Members follow the same rule. Fields, methods and associated functions of a class are private unless marked
`export`, and using a declaration or member that is not visible is `E-NAM-004`, reported where it is used
(LR 4.4). A package that another package depends on is a library: it has no `entry` and no `main`. Only the root program's entry `main` creates the capabilities, and a library's functions receive them as parameters (LR 11.6). Here a small library, `guide.menu`, keeps `discountCode` private:

```yaml
# file: guide.menu/project.yaml
schema: fortis.project/v1
name: guide.menu
version: 1.0.0
exports: [dishes]
dependencies:
  fortis: "1.0"
```

```fortis
// accept
// file: guide.menu/src/dishes.ft
export pure func teaPennies() Int => 250

pure func discountCode() StringView from static => "SPRING"

test "tea costs 250 pennies" {
  expect teaPennies() == 250
}
```

An application that depends on it cannot reach the private member:

```yaml
# file: guide.till/project.yaml
schema: fortis.project/v1
name: guide.till
version: 1.0.0
entry: main
exports: []
dependencies:
  fortis: "1.0"
  guide.menu: "1.0.0"
```

```fortis
// reject: E-NAM-004
// file: guide.till/src/main.ft
import fortis.host { Console }
import guide.menu.dishes

func main(console: Console) {
  console.print(dishes.discountCode())        // reject: E-NAM-004
}
```

An exported signature may name only exported types. An exported function returning a private class would hand
callers a value whose type they cannot name, so it is rejected with `E-TYP-030` (LR 4.4):

```fortis
// reject: E-TYP-030
class Hidden {
  export n: Int
}

export pure func make() Hidden => Hidden(n: 1)    // reject: E-TYP-030

func main() {
}
```

Two more visibility rules shape how classes are used across modules (LR 4.4). An exported field can be read
through any loan and written through an `editable` one. And a class's generated constructor,
`Line(item: …, pennies: …)`, is available only where every field is visible, which is how a class with private
fields forces every other module through its own associated functions (LR 4.4, LR 9.2).

## Names and resolution

Each module has one namespace: its declarations, its imported aliases and its imported members. A name resolves to
the innermost enclosing block that declares it, then to the module (LR 4.5). Qualified names take four forms:
`module.member`, `Type.associated`, `Enum.Variant` and `value.field` or `value.method(…)`. There is no `::`.

Two rules keep a name's meaning obvious at every use (LR 4.5). A `let` that would shadow a visible binding is
`E-NAM-003` (see [Statements and expressions](05-statements.md)). And a field is always reached through a
receiver, `self.lines` or `order.lines`; inside a method, a bare field name is not in scope.

A type's identity is its name, its declaring package and version, and its declaring module (LR 4.1). Two modules
may each declare a `Point`, and they are two different types, even with identical fields. Passing one where the
other is expected is a type mismatch, `E-TYP-040` (LR 6.1), and the record names both modules. Here is a library
whose two modules each declare a `Point`, and a test that uses both correctly:

```yaml
# file: guide.geo/project.yaml
schema: fortis.project/v1
name: guide.geo
version: 1.0.0
exports: [flat, screen]
dependencies:
  fortis: "1.0"
```

```fortis
// accept
// file: guide.geo/src/flat.ft
export trivial class Point {
  export x: Int
  export y: Int
}
```

```fortis
// accept
// file: guide.geo/src/screen.ft
export trivial class Point {
  export x: Int
  export y: Int
}

export pure func column(p: Point) Int => p.x
```

```fortis
// accept
// file: guide.geo/src/both.ft
import guide.geo.flat
import guide.geo.screen

test "each module's Point is its own type" {
  let onScreen = screen.Point(x: 3, y: 4)
  let onPaper = flat.Point(x: 1, y: 2)
  expect screen.column(onScreen) == 3
  expect onPaper.x == 1
}
```

A module that passes a `flat.Point` where a `screen.Point` is expected is rejected, here in an application that
uses the library:

```yaml
# file: guide.geomix/project.yaml
schema: fortis.project/v1
name: guide.geomix
version: 1.0.0
entry: app
exports: []
dependencies:
  fortis: "1.0"
  guide.geo: "1.0.0"
```

```fortis
// reject: E-TYP-040
// file: guide.geomix/src/app.ft
import guide.geo.flat
import guide.geo.screen

func main() {
  let onPaper = flat.Point(x: 1, y: 2)
  discard screen.column(onPaper)        // reject: E-TYP-040
}
```

This is what lets two versions of one package, or two packages that happen to use the same names, live in one
program: each version's functions and types run where they are named (LR 4.1, LR 4.4).

## The entry point

An application names its entry module in the manifest (`entry: app`), and that module declares a function called
`main` with one of four heads (LR 4.6):

| Head | Use it when `main` |
|---|---|
| `func main(…)` | cannot fail and does not suspend |
| `try func main(…)` | may fail |
| `async func main(…)` | awaits asynchronous work |
| `async try func main(…)` | both |

`main` asks for what it needs as parameters, and the runtime supplies each one by its type: `List[String]` gets the
command-line arguments, and each capability type of `fortis.host` (`Console`, `FileSystem`, `Clock` and the rest)
gets that capability. Names and order are yours, each type appears once, and a `main` that needs nothing takes no
parameters. `export` is implied on the entry `main` (LR 4.6):

```fortis
// accept
import fortis.host { Console, FileSystem, Path }

try func main(console: Console, files: FileSystem, args: List[String]) {
  let path = Path.parse("note.txt") or fail
  files.writeAll(path, "${args.count()} arguments") or fail
  console.print(files.readText(path) or fail)
}
```

```output
0 arguments
```

A parameter of any other type is rejected (`E-NAM-005`), and so is a second parameter of a type already taken
(`E-NAM-006`, LR 4.6). `main` owns the capabilities it receives: it lends them to the functions it calls, and it
may move one, for example into a `Shared` that several owners hold (LR 4.6, LR 7.8). Older code writes
`export func main(args: List[String]) Void` and creates its capabilities with `Host.capabilities()` in the body;
that form is still accepted, and `fortis fmt` rewrites it to this one.

Any of the four may also carry `recovers`, written like any function whose summary has that effect: a `main` that
reaches a function that recovers a failure writes `try recovers func main` (LR 4.6, LR 11.2). Today's
checkers refuse `recovers` on the `main` of a package, though they accept it in a single file (D-52); until that is
fixed, a package's `main` can report the failures it would recover instead.

`main` gets I/O authority through its capability parameters, and no other function receives capabilities from
the runtime or creates them, which is why every function that does I/O through a capability must be handed one;
`report` and the log statements are the `io` operations that need none (LR 4.6, LR 11.6, LR 10.6, LR 13). A
package whose entry module has no `main` with one of these heads is rejected (LR 4.6); the LR names no code for
that yet (X-152).

If a `try main` fails, the runtime writes exactly one report of the failure to standard error and the process
exits with status 1 (LR 4.6). Nothing else is needed, and there is no `or die`:

```fortis
// accept (exit status 1)
import fortis.host { Path, Console, FileSystem }

try func main(console: Console, fileSystem: FileSystem) {
  console.print("reading settings")
  let path = Path.parse("settings.txt") or fail
  let text = fileSystem.readText(path) or fail
  console.print(text)
}
```

```output
reading settings
```

The file does not exist, so `readText` fails with `fs.not_found`, `or fail` passes the failure to `main`'s caller,
and the runtime reports it once: as text on a terminal, and as one JSON record on a pipe (LR 4.6). The report is a
record like every other finding.

An `async main` runs on the core cooperative executor as the root task, and the process ends when it completes
(LR 4.6):

```fortis
// accept
import fortis.host { Clock, Console }

async func greet(console: Console, clock: Clock) {
  await clock.sleep(10ms)
  console.print("after a short sleep")
}

async func main(console: Console, clock: Clock) {
  console.print("starting")
  await greet(console, clock)
}
```

```output
starting
after a short sleep
```

[Cleanup, async and concurrency](12-async.md) explains `async` and `await`.
