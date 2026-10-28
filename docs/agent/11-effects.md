# Effects and contracts (LR 11)

<!-- covers: LR 11 -->
*Agent mode. Human mode: [Effects and contracts](../guide/11-effects.md). Source of truth: LR 11.*

> Preview: contracts (LR 11.5) arrive in v0.1. Effects, claims, noalloc and capabilities (LR 11.1, 11.2, 11.3, 11.4 and 11.6) arrive in v0.2. The observation plane and aspects (LR 11.7, 11.8) arrive in v0.5.

## Rules

| Rule | Statement | LR |
|---|---|---|
| LANG-11-01 | The effect set is exactly `alloc`, `io`, `mutate`, `nondet`, `suspend`, `block`, `fail`, `recovers`; summaries are computed transitively, closed under recursion by a least fixed point. | LR 11.1, 11.3 |
| LANG-11-02 | Writes to fresh storage, and writes through the function's own `editable` parameters (its write footprint), are not `mutate`; writes through captures, guards or atomics are. | LR 11.1, 11.3 |
| LANG-11-03 | `noalloc` is checked against the allocation-pattern whitelist for the actual target and telemetry plan, covering failures, cleanup, closures and every async phase; a conflicting telemetry policy is rejected at the policy. | LR 11.4 |
| LANG-11-04 | A trusted summary is labelled `trusted`, never `proved`; `trusted` is rejected outside the `fortis` package. | LR 11.3 |
| LANG-11-05 | `async`, `try` and `recovers` are written wherever the summary has the effect; across a module boundary only written `pure`/`noalloc` count; weakening an exported function's written modifiers is a reported API change. | LR 11.2, 11.3 |
| LANG-11-06 | A contract contains only modifiers, `forbids <effect>`, `requires capability A` and `excludes capability A`; configuration never adds predicates. | LR 11.5 |
| LANG-11-07 | `io` other than `report` requires an `Authority` parameter, except in `main` (which calls `Host.capabilities()`) and in an adapter's minting function, which itself requires the capability it derives the authority from, so every authority traces back to `main`; `Host.capabilities()` anywhere else, or twice, is rejected. | LR 11.6 |
| LANG-11-08 | Contracts are applied in the modifier position, before single-word modifiers, without arguments; `@` applies aspects only. | LR 11.5 |
| LANG-11-09 | Aspects have only the parts `needs`, `before`, `after`, `on failure`, `metadata`; they elaborate before checking (`before` in application order, `after` reversed, `on failure` innermost first) and the elaborated function passes every ordinary check. | LR 11.8 |

## Modifier claims

| Modifier | Summary must not contain | Must be written when summary has |
|---|---|---|
| `pure` | `io`, `mutate`, `nondet`, `suspend`, `block` | (optional claim) |
| `noalloc` | `alloc` | (optional claim) |
| `async` | — | `suspend` |
| `try` | — | `fail` |
| `recovers` | — | `recovers` |

Source: LR 11.2. Order in a head: `[export] [Contract …] [pure] [noalloc] [async] [try] [recovers] func` (LR 8.1).

## Summary transfer rules (LR 11.3)

- Call: imports the callee's summary at its actual type arguments and callback types.
- `fail` is imported only when the failure leaves the function: kept by `or fail` and `fail f`; removed by
  `or recover`, `catch … recover`, and a consuming `match`, which add `recovers` unless the failure was reported
  first (LR 10.4).
- Write footprint: per `editable` parameter (and `editable self`). At a call each footprint parameter is
  instantiated with the argument place: fresh in the caller → nothing; reachable from a caller `editable`
  parameter → joins the caller's footprint; otherwise (capture, `Mutex` guard, atomic) → `mutate`.
- Fresh: a `let mut` initialised from a constructor, a literal or an owned call result. A binding holding a loan
  result is a path to the loan's owner.
- `async` call: builds a future, imports only argument evaluation; `await` (or `launch` into the awaiting scope)
  imports the future's summary; `launchParallel`/`launchBlocking` import the work's summary minus `block`.
- Callback parameter: contributes what its function type permits. A lambda literal with no `pure`-excluded effect
  has a `pure` type.
- `cleanup` effects join every function that may drop the type.
- `report` is `io` but needs no capability (LR 10.6); a `pure` function cannot report.
- Instrumentation (`capture`, `record`, completion records) does not change a summary (LR 11.7).

## Contract elements (LR 11.5)

| Element | Effect on the target |
|---|---|
| `pure`, `noalloc`, … | the modifier's claim |
| `forbids recovers` (any of the eight effects) | that effect must be absent from the summary |
| `requires capability A` | the function takes a parameter of `Authority` type `A` |
| `excludes capability A` | the function neither takes `A` nor calls, transitively, a function that requires `A` |

Contracts compose by conjunction; contradictions are rejected; `pure` with `requires capability` is a
contradiction because a `pure` function takes no capability parameter (LR 11.5). The formatter keeps the name
and prints the expansion after it: `export FastCalculation pure noalloc func square` (LR 11.5).

## Authorities (LR 11.6)

- Capabilities (process-wide, created only by `Host.capabilities()` in `main`, once): `FileSystem`, `Network`,
  `Clock`, `Random`, `Environment`, `Process`, `Supervisor`, `Console`, `SecretAccess`, `RawSql`.
- Per-activation authorities (`RequestContext`, `Identity`, `Db`, `Tx`, `TaskContext`, `Mail`): minted by a
  kit's adapter from a capability, passed as a parameter.
- `Shared[A]` of an authority `A` satisfies the rule like `A`; share one capability as
  `let c = Shared.create(move out)`, then `c.retain()` per owner (no field stores a loan, LR 7.7).
- Attenuation only narrows: `fs.subtree(path)`.
- Clock reads are `nondet` and need `Clock`; a `pure` function takes time as a plain parameter.

## Aspect parts (LR 11.8)

| Part | Constraint |
|---|---|
| `needs name: Type` | target has exactly one parameter of `Type` (two: error); hooks use it with the target's mode |
| `needs Contract` | target carries the contract |
| `before { … }` | may `fail` on a non-`try` target only if an `on failure` of this or an outer aspect maps every failure |
| `after(result: R) { … }` | read loan of the result; may `fail` only when `R` is not linear |
| `on failure(f: Failure) R { … }` | owns `f`; must `return` an `R` or `fail` |
| `metadata` | arguments readable by libraries through the toolchain |

Arguments are compile-time constants: literals, record and variant-map literals, enum values, capture-free `fn`
literals, declaration names. First parameter may be positional, the rest named. Specialisation:
`aspect a(params, ...rest) = b(fixed: v, ...rest)`. No `around`, no body access. Core aspect: `observe(schema)`
(LR 11.8, LR 13).

## Diagnostics

| Code | Trigger | Correction | LR |
|---|---|---|---|
| E-EFF-001 | summary exceeds the modifiers or contract: an effect `pure` excludes, `alloc` under `noalloc`, `suspend` without `async`, `fail` without `try`, `recovers` without `recovers`, a forbidden effect, a missing required or a present excluded authority | remove the effect; or, when the effect is intended, write the modifier its type needs (`async`, `try`, `recovers`). Dropping a written `pure`, `noalloc` or contract is an API change for whoever owns the claim, never a correction (LR 11.3) | LR 11.1, 11.3, 11.5 |
| E-EFF-010 | `io` reached through a capability without an `Authority` parameter (`report` needs none); `Host.capabilities()` outside `main` or called twice | take the authority (`console: Console`) as a parameter; call `Host.capabilities()` once in `main` and pass values down | LR 11.6 |
| E-EFF-020 | `block` inside an `async` function (or anything it calls), such as `lockBlocking` | `await m.lock()`, or move the blocking work into `launchBlocking` | LR 11.1 |
| E-EFF-030 | the application's telemetry policy attaches an allocating record to a `noalloc` function | change the policy at the reported site | LR 11.4 |
| E-EFF-040 (warning) | `--api-baseline OLD`: an exported function lost a written `pure`/`noalloc` or gained `async`/`try`/`recovers` | intended: bump the API; otherwise restore the modifier | LR 11.3 |
| E-TYP-150 | unknown aspect, arguments not matching the declaration, unmet or ambiguous `needs`, `...rest` outside an aspect's parameter list | fix the application or the target's parameters | LR 11.8 |
| E-OBL-008 | an `after` hook that may fail on a linear result type | do not fail in `after`, or apply it to a non-linear result | LR 11.8 |
| E-OBL-001 | elaborated path leaves a linear `owned` parameter unconsumed (a failing `before`) | consume it on that path, or take it by loan | LR 11.8 |

## Correction patterns

### E-EFF-001: `pure` function performing I/O (LR 11.1)

```fortis
// reject: E-EFF-001
import fortis.host { Console, Host }

pure func greet(console: Console) Void {      // reject: E-EFF-001 (a pure function takes no capability)
  console.print("hello")              // reject: E-EFF-001
}

export func main(args: List[String]) Void {
  let caps = Host.capabilities()
  greet(caps.console)
}
```

```fortis
// accept
import fortis.host { Console, Host }

func greet(console: Console) Void {
  console.print("hello")
}

export func main(args: List[String]) Void {
  let caps = Host.capabilities()
  greet(caps.console)
}
```

```output
hello
```

### E-EFF-001: missing `recovers` (LR 11.1, LR 11.2)

`main` inherits the effect from its callee and needs the modifier too.

```fortis
// reject: E-EFF-001
export error Odd {
  code: "demo.odd"
}

try func half(n: Int) Int {
  if n % 2 == 1 {
    fail Odd()
  }
  return n / 2
}

func halfOrZero(n: Int) Int => half(n) or recover 0   // reject: E-EFF-001

test "half" {
  expect halfOrZero(4) == 2
}
```

```fortis
// accept
export error Odd {
  code: "demo.odd"
}

try func half(n: Int) Int {
  if n % 2 == 1 {
    fail Odd()
  }
  return n / 2
}

recovers func halfOrZero(n: Int) Int => half(n) or recover 0

test "half" {
  expect halfOrZero(4) == 2
  expect halfOrZero(3) == 0
}
```

### E-EFF-001: `mutate` through a capture in a `pure` function (LR 11.1, LR 11.3)

```fortis
// reject: E-EFF-001
class Cell {
  export value: Int
  pure func set(editable self, value: Int) Void {
    self.value = value
  }
}

pure func viaClosure() Int {
  let mut cell = Cell(value: 0)
  let mut poke = fn mut[editable cell]() Void {
    editable cell.set(2)
  }
  editable poke()                     // reject: E-EFF-001
  return 0
}

test "closure" {
  expect viaClosure() == 0
}
```

```fortis
// accept
class Cell {
  export value: Int
  pure func set(editable self, value: Int) Void {
    self.value = value
  }
}

pure func direct() Int {
  let mut cell = Cell(value: 0)
  editable cell.set(2)
  return cell.value
}

test "direct" {
  expect direct() == 2
}
```

### E-EFF-001: allocation under `noalloc` (LR 11.1, LR 11.4)

```fortis
// reject: E-EFF-001
noalloc func pair(a: Int, b: Int) List[Int] => List.of(a, b)   // reject: E-EFF-001

test "pair" {
  expect pair(1, 2).count() == 2
}
```

```fortis
// accept
pure noalloc func larger(a: Int, b: Int) Int {
  if a > b {
    return a
  }
  return b
}

test "larger" {
  expect larger(1, 2) == 2
}
```

### E-EFF-001: contract violation (LR 11.1, LR 11.5)

```fortis
// reject: E-EFF-001
import fortis.host { Network }

contract Offline {
  excludes capability Network
}

func lookup(net: Network, name: StringView) Int => name.scalarCount()

Offline func viaNetwork(net: Network) Int => lookup(net, "example")   // reject: E-EFF-001

test "offline" {
  expect true
}
```

```fortis
// accept
contract Offline {
  excludes capability Network
}

Offline pure func measure(name: StringView) Int => name.scalarCount()

test "offline" {
  expect measure("example") == 7
}
```

### E-EFF-010: `Host.capabilities()` outside `main` (LR 11.6)

```fortis
// reject: E-EFF-010
import fortis.host { Host }

func sneak() Void {
  let caps = Host.capabilities()      // reject: E-EFF-010
}

export func main(args: List[String]) Void {
  sneak()
}
```

```fortis
// accept
import fortis.host { Console, Host }

func work(console: Console) Void {
  console.print("work")
}

export func main(args: List[String]) Void {
  let caps = Host.capabilities()
  work(caps.console)
}
```

```output
work
```

### E-EFF-010: I/O without an `Authority` parameter (LR 11.6)

Reported at the I/O statement.

```fortis
// reject: E-EFF-010
import fortis.host { Capabilities, Host }

func log(caps: Capabilities, text: StringView) Void {
  caps.console.print(text)            // reject: E-EFF-010
}

export func main(args: List[String]) Void {
  let caps = Host.capabilities()
  log(caps, "hello")
}
```

```fortis
// accept
import fortis.host { Console, Host }

func log(console: Console, text: StringView) Void {
  console.print(text)
}

export func main(args: List[String]) Void {
  let caps = Host.capabilities()
  log(caps.console, "hello")
}
```

```output
hello
```

### E-EFF-020: blocking inside `async` (LR 11.1)

The `block` effect is reported in every `async` function that reaches it, here `wait` and its caller `main`.

```fortis
// reject: E-EFF-020
async try func wait(m: Mutex[Int]) Int {
  let g = m.lockBlocking() or fail    // reject: E-EFF-020
  return 0
}

export async try func main(args: List[String]) Void {
  let m = Mutex.create(1)
  let v = await wait(m) or fail       // reject: E-EFF-020
}
```

```fortis
// accept
async try func wait(m: Mutex[Int]) Int {
  {
    let g = await m.lock() or fail
  }
  return 0
}

export async try func main(args: List[String]) Void {
  let m = Mutex.create(1)
  let v = await wait(m) or fail
}
```

### E-TYP-150: unmet `needs` (LR 11.8)

```fortis
// reject: E-TYP-150
import fortis.host { Console }

export aspect logged(label: StringView) {
  needs console: Console
  before {
    console.print("before ${label}")
  }
}

@logged("count")                      // reject: E-TYP-150
func count(n: Int) Int => n

export func main(args: List[String]) Void {
  discard count(3)
}
```

```fortis
// accept
import fortis.host { Console, Host }

export aspect logged(label: StringView) {
  needs console: Console
  before {
    console.print("before ${label}")
  }
}

@logged("count")
func count(console: Console, n: Int) Int => n

export func main(args: List[String]) Void {
  let caps = Host.capabilities()
  discard count(caps.console, 3)
}
```

```output
before count
```

### E-OBL-008: failing `after` on a linear result (LR 11.8)

```fortis
// reject: E-OBL-008
export error Stop {
  code: "demo.stop"
}

export aspect strict[R]() {
  after(result: R) {
    fail Stop()                       // reject: E-OBL-008
  }
}

@strict()
try func produce(n: Int) Outcome[Int] {
  return Outcome.success(n)
}

export func main(args: List[String]) Void {
}
```

```fortis
// accept
export error Stop {
  code: "demo.stop"
}

export aspect positive() {
  after(result: Int) {
    if result < 0 {
      fail Stop()
    }
  }
}

@positive()
try func produce(n: Int) Int {
  return n
}

test "produce" {
  expect (produce(3) or fail) == 3
}
```

## Facts used when writing code

- Elaboration order for `@a @b func f`: `a.before`, `b.before`, body, `b.after`, `a.after`; a failure goes to
  `b`'s `on failure` first, then `a`'s; a handler that returns completes the function with its value, which is a
  success exit for observation (LR 11.8).
- The hooks' effects join the summary; a hook that allocates under `noalloc`, reports under `pure` or calls
  `Host.capabilities()` is rejected by the ordinary rules (LR 11.8).
- `pure` and `noalloc` are independent (LR 11.2). There is no `nostate`; `no-local-mutation` is an opt-in lint
  (LR 11.2).
- `noalloc` on an `async` function covers construction, every poll, completion and destruction; the VM's own
  allocations do not count against the native claim (LR 11.4).
- Target feature sets (`core`, `alloc`, `host`, `async`, `web`, `browser`) are not effects and not capabilities
  (LR 11.6).
