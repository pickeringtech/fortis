# Modules and packages (LR 4)

<!-- covers: LR 4 -->
*Agent mode. Human mode: [Modules and packages](../guide/04-modules.md). Source of truth: LR 4.*

> Preview: dependencies and lock files (LR 4.1) arrive in v0.2. An async entry point (LR 4.6, LR 12.3) arrives in v0.3. One package, its modules, imports, visibility and the ordinary entry point (LR 4.1, 4.2, 4.3, 4.4, 4.5 and 4.6) are in v0.0.1.

## Rules

| Rule | Statement | LR |
|---|---|---|
| LANG-04-01 | A module is one file under `src/`; its path is its identity (`src/a/b.ft` is `a.b`, imported from another package as `PKG.a.b`). A module holds only declarations and `const` bindings with compile-time constant initialisers. | LR 4.2 |
| LANG-04-02 | Importing a module executes no code (there is no module initialisation). | LR 4.2 |
| LANG-04-03 | Across packages, only modules listed in the exporting package's `exports:` may be imported. | LR 4.3 |
| LANG-04-04 | Two versions of one package are two packages; a type's identity is its name, package, version and declaring module, so same-named types of two modules or versions never interchange. | LR 4.1 |
| LANG-04-05 | A `let` may not shadow a visible binding (`E-NAM-003`). | LR 4.5 |
| LANG-04-06 | The entry point is the exported `main` of the manifest's `entry` module, with one of the four signatures of LR 4.6, each optionally `recovers` (X-155; refused in packages today, D-52). | LR 4.6 |

## Package and import facts

| Fact | LR |
|---|---|
| Package = directory with `project.yaml`, lockfile `fortis.lock`, `src/`. Manifest keys: `schema: fortis.project/v1`, `name`, `version`, `exports: [modules]`, `dependencies: {name: "version"}`, optional `entry: module` (application), optional `root: module`. | LR 4.1 |
| A standard dependency (`fortis`, `fortis.…`) names a package of the compatibility set (`library/fortis/compatibility.yaml`) at the set's version, else `E-NAM-031`. | LR 4.1 |
| Lockfile: when present, resolution uses only it and fetches nothing. Unpinned dependency, unused pin or unreadable lockfile: `E-NAM-020`. Digest mismatch (package, compiler, standard library, Rust toolchain): `E-NAM-021`. `fortis check` verifies package, compiler and standard-library pins; a native build also verifies the Rust toolchain. | LR 4.1 |
| `--target T`: an import of a standard module the target's feature set lacks is `E-NAM-030`, before any native step. A package shipping a Cargo build script is refused by a native build: `E-NAM-032`. | LR 4.1 |
| Limits: manifest over 1 MiB or 10,000 lines `E-LIM-003`; lockfile over 8 MiB or 100,000 lines `E-LIM-004`; toolchain JSON over 256 MiB or nested over 128 `E-LIM-005`. Toolchain failures are records, never crashes: `E-INT-001` (checker defect, with a minimised `reproduction`), `E-INT-002` (generated Rust does not compile), `E-INT-003` (a native tool missing or failing). | LR 4.1 |
| Import forms: `import p.m` binds alias `m`; `import p.m as x` binds `x`; `import p.m { A, b }` binds exactly the listed members. Imports only at the top of a module, before any declaration. | LR 4.3 |
| Path split: the longest prefix naming the current package or a declared dependency is the package; the rest is the module path. Sibling modules therefore import each other by the full path (`import guide.shop.orders`). No matching prefix: an error naming the undeclared package. Ambiguous splits across the dependency set: `E-NAM-010`. | LR 4.3 |
| `root: m` in the manifest makes `import p { … }` mean module `m` of `p`; without it an empty module path is an error. | LR 4.3 |
| `fortis.prelude` is imported implicitly (core types, optional operators, built-in interfaces); `fortis.text`, `fortis.collections`, `fortis.numeric`, `fortis.time`, `fortis.money`, `fortis.host` need imports. Every kit package is named `fortis.<kit>`. | LR 4.3 |
| Importing one name twice, or a name that collides with a local declaration, is an error. Unused import: warning at the baseline lint level, error at the strict one. | LR 4.3 |
| `export` is the only visibility modifier. Unexported: private to the module. Exported: visible to importers; outside the package only if the module is in `exports:`. Class fields, methods, associated functions: private unless `export`. Enum variants, interface members and mixin-required methods share their declaration's visibility. | LR 4.4 |
| Generated constructor `C(field: …)` is callable only where every field is visible. An exported field is readable through any loan and writable through an `editable` one. | LR 4.4 |
| Names resolve to the innermost enclosing block, then the module namespace. Qualified forms: `module.member`, `Type.associated`, `Enum.Variant`, `value.field`, `value.method(…)`. No `::`. A bare field name is not in scope inside a method: use `self.field`. | LR 4.5 |
| `main(args: List[String]) Void` with modifiers `try`, `async` or `async try`; it calls `Host.capabilities()` exactly once. A failing `try main` prints one failure record to standard error and exits 1. An `async main` is the root task on the core cooperative executor. | LR 4.6 |

## Diagnostics

| Code | Trigger | Correction | LR |
|---|---|---|---|
| E-NAM-003 | `let` shadows a visible binding | rename | LR 4.5 |
| E-NAM-004 | use of a declaration or member not visible at the use site | `export` it (and list its module in `exports:` for other packages), or use an exported API | LR 4.4 |
| E-TYP-030 | exported signature (function, method, field, constant) names a non-exported type | `export` the type, or keep the function private | LR 4.4 |
| E-NAM-010 | dependency set with names that make an import's package/module split ambiguous | rename or drop a dependency | LR 4.3 |
| E-NAM-020, E-NAM-021 | lockfile does not pin the manifests' dependencies, or a digest differs | `fortis lock` after reviewing the change | LR 4.1 |
| E-NAM-030 | import of a standard module the `--target` feature set lacks | remove the import or choose a target that has it | LR 4.1 |
| E-NAM-031 | standard dependency outside the compatibility set or at another version | use the set's version | LR 4.1 |
| E-NAM-032 | package ships a Cargo build script (native build) | remove the build script | LR 4.1 |
| E-LIM-003, E-LIM-004, E-LIM-005 | manifest, lockfile or toolchain JSON over its limit | reduce it | LR 4.1 |
| E-INT-001, E-INT-002, E-INT-003 | toolchain defect, generated Rust rejected, native tool missing | report it with the record's reproduction; not a source error | LR 4.1 |

LR 4.3 and 4.6 state some rules without naming a code: an import of an undeclared package or of a module the
exporting package does not list, an import that collides with a local declaration, and a missing or wrongly typed
`main` are rejected, but which code reports each is not yet settled (X-152). Read the record's `message` and
`fields.candidates` rather than relying on its code.

## Correction patterns

### A package (LR 4.2, LR 4.3)

```yaml
# file: agent.shop/project.yaml
schema: fortis.project/v1
name: agent.shop
version: 1.0.0
entry: main
exports: [prices]
dependencies:
  fortis: "1.0"
```

```fortis
// accept
// file: agent.shop/src/prices.ft
export pure func gross(net: Int<0..1000>) Int => net + net / 5

pure func margin() Int => 3

test "gross adds a fifth" {
  expect gross(100) == 120
}
```

```fortis
// accept
// file: agent.shop/src/main.ft
import fortis.host { Host }
import agent.shop.prices

export func main(args: List[String]) Void {
  let caps = Host.capabilities()
  caps.console.print("${prices.gross(100)}")
}
```

```output
120
```

### E-NAM-004: private member of another module (LR 4.4)

```yaml
# file: agent.till/project.yaml
schema: fortis.project/v1
name: agent.till
version: 1.0.0
exports: [prices]
dependencies:
  fortis: "1.0"
```

```fortis
// accept
// file: agent.till/src/prices.ft
export pure func gross(net: Int<0..1000>) Int => net + net / 5

pure func margin() Int => 3

test "gross adds a fifth" {
  expect gross(100) == 120
}
```

`agent.till` is a library. A package that another package depends on is a library: it has no `entry` and no `main`. Only the root program's entry `main` creates the capabilities, and a library's functions receive them as parameters (LR 11.6).

```yaml
# file: agent.tillmix/project.yaml
schema: fortis.project/v1
name: agent.tillmix
version: 1.0.0
entry: main
exports: []
dependencies:
  fortis: "1.0"
  agent.till: "1.0.0"
```

```fortis
// reject: E-NAM-004
// file: agent.tillmix/src/main.ft
import fortis.host { Host }
import agent.till.prices

export func main(args: List[String]) Void {
  let caps = Host.capabilities()
  caps.console.print("${prices.gross(100)}")
  discard prices.margin()               // reject: E-NAM-004
}
```

Correction: write `export pure func margin() Int => 3` in `prices`, or stop using it.

### E-TYP-030: exported signature names a private type (LR 4.4)

```fortis
// reject: E-TYP-030
class Hidden {
  export n: Int
}

export pure func make() Hidden => Hidden(n: 1)    // reject: E-TYP-030

test "make" {
  expect make().n == 1
}
```

```fortis
// accept
export class Shown {
  export n: Int
}

export pure func make() Shown => Shown(n: 1)

test "make" {
  expect make().n == 1
}
```

### Import placement and collisions (LR 4.3)

```fortis
// reject: E-SYN-000
pure func one() Int => 1

import fortis.text                // reject: E-SYN-000

test "one" {
  expect one() == 1
}
```

An imported name may not collide with a declaration of the module (`import fortis.money { Money }` beside a local
`class Money` is rejected); the LR names no code for it yet (X-152). Rename the local declaration:

```fortis
// accept
import fortis.money { Money, Currency }

class Price {
  export pennies: Int
}

test "money" {
  expect Price(pennies: 1).pennies == 1
  expect Money.zero(Currency.GBP).currency == Currency.GBP
}
```

### Entry point (LR 4.6)

A failing `try main` exits with status 1 and one failure record on standard error; standard output keeps what was
printed before the failure.

```fortis
// accept (exit status 1)
import fortis.host { Host, Path }

export try func main(args: List[String]) Void {
  let caps = Host.capabilities()
  caps.console.print("start")
  let text = caps.fileSystem.readText(Path.parse("missing.txt") or fail) or fail
  caps.console.print(text)
}
```

```output
start
```

## Facts used when writing code

- No module initialisation: a module-level value is a `const` with a constant initialiser (LR 4.2, LR 5.1).
- Within a package, imports may be cyclic for type declarations (LR 4.2).
- Renaming an exported declaration or parameter, or moving a module out of `exports:`, breaks other packages
  (LR 4.4, LR 5.8).
- Reach the process's authority only through `main`'s `Host.capabilities()` and pass capabilities down as
  parameters (LR 4.6, LR 11.6).
