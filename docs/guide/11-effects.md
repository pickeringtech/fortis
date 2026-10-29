# Effects and contracts

<!-- covers: LR 11 -->
*Human mode. Agent mode: [Effects and contracts, by rule](../agent/11-effects.md). Source of truth: LR 11.*

> Preview: contracts (LR 11.5) arrive in v0.1. Effects, claims, noalloc and capabilities (LR 11.1, 11.2, 11.3, 11.4 and 11.6) arrive in v0.2. The observation plane and aspects (LR 11.7, 11.8) arrive in v0.5, except the diagnostic plane that `report` and the `log` statements write to (LR 11.7, LR 13), which is in v0.0.2. `main`'s capability parameters (LR 4.6, 11.6) and the core aspect `@ignorable` (LR 5.5, 11.8) are in v0.0.2.

A reader of a Fortis function wants to know what it may do besides compute its result: allocate, touch the
outside world, change something it was not handed, read the clock, wait, fail, or quietly swallow a failure. The
checker works this out for every function, from its body and everything it calls, and compares the answer with
what the function's head claims. This chapter covers that machinery: the eight effects, the modifiers that state
claims about them, contracts that name a set of claims, capabilities that carry the authority to do I/O, and
aspects, which add declared behaviour around a function.

## The eight effects

Every function has an **effect summary**: a subset of a closed set of eight effects, computed from its body and
its callees (LR 11.1). No package or configuration can add a ninth.

| Effect | Present when the function may… |
|---|---|
| `alloc` | allocate heap memory, including inside callees, failures, cleanup and the closures it creates |
| `io` | perform input or output through a capability, including `report` |
| `mutate` | write to storage that is neither fresh in this call nor reachable from one of its own `editable` parameters |
| `nondet` | read a non-deterministic source: a clock, randomness, the environment, an address |
| `suspend` | reach an `await` |
| `block` | block the calling thread, such as waiting for a lock synchronously |
| `fail` | return a failure |
| `recovers` | discharge a failure that was not reported |

When the summary contains something the function's modifiers do not admit, the function is rejected with
`E-EFF-001`, which names the effect and the statement that introduces it (LR 11.1). Here a function claims to be
`pure` and prints:

```fortis
// reject: E-EFF-001
import fortis.host { Console }

pure func greet(console: Console) {      // reject: E-EFF-001 (a pure function takes no capability)
  console.print("hello")              // reject: E-EFF-001
}

func main(console: Console) {
  greet(console)
}
```

### Fresh storage is not mutation

`mutate` means writing to something the caller can see. Writing to storage the function created itself, and has
not yet stored anywhere reachable from a parameter, a capture or `self`, is not `mutate` (LR 11.1). So building a
list locally and returning it is pure:

```fortis
// accept
import fortis.host { Console }

pure func squares(n: Int<0..10>) List[Int] {
  let mut out: List[Int] = List.default()
  for i in 1..n {
    out.push(i * i)
  }
  return out
}

func main(console: Console) {
  for s in squares(4) {
    console.print("${s}")
  }
}
```

```output
1
4
9
16
```

## Modifiers are claims

The function head carries five words that speak about effects (LR 11.2):

| Modifier | Claim the checker verifies |
|---|---|
| `pure` | none of `io`, `mutate`, `nondet`, `suspend`, `block` |
| `noalloc` | no `alloc` |
| `async` | `suspend` is permitted |
| `try` | `fail` is permitted |
| `recovers` | `recovers` is permitted |

`pure` and `noalloc` are independent: a `pure` function may allocate, and a `noalloc` function may do I/O
(LR 11.2). The two kinds of modifier are also treated differently. `async`, `try` and `recovers` change a
function's type, so a function must write them whenever its summary has the effect (LR 11.3). `pure` and
`noalloc` are guarantees a function *may* offer: leaving one off never breaks a caller, it only withholds a
promise (LR 11.3).

A function that silently swallows a failure must say so. `half(n) or recover 0` discharges the failure without
reporting it, which is the `recovers` effect, so the function needs the `recovers` modifier (LR 11.2):

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

recovers func main() {
  discard halfOrZero(3)
}
```

```fortis
// accept
import fortis.host { Console }

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

recovers func main(console: Console) {
  console.print("${halfOrZero(8)} ${halfOrZero(3)}")
}
```

```output
4 0
```

`recovers` is contagious in the same way as the other effects: `main` calls `halfOrZero`, so it inherits the
effect and writes the modifier too. A reviewer reading a signature diff can therefore see every function that
swallows a failure somewhere below it (LR 8.1).

## How summaries are computed

A function's summary is built from its statements and closed over recursion (LR 11.3). A call imports the
callee's summary, with two adjustments:

- `fail` is imported only if the call's failure leaves the function. `or fail` keeps it; `or recover`, a `catch`
  that ends in `recover`, and a `match` that consumes the failure remove it and add `recovers` where the failure
  was not reported (LR 11.3).
- `mutate` is imported through the callee's **write footprint**, described next.

### Write footprints

A method with `editable self`, or a function with an `editable` parameter, writes through a loan its caller
handed it. Inside the function that write is not `mutate`; it is recorded in the function's write footprint
(LR 11.3). At each call the footprint is matched against the actual argument: if the caller's place is fresh,
nothing is imported; if it is reachable from the caller's own `editable` parameter, it joins the caller's
footprint; anything else, such as a mutable capture, makes the caller `mutate` (LR 11.3). This is what lets a
`pure` function build an object with mutating methods:

```fortis
// accept
import fortis.host { Console }

class Cell {
  export value: Int
  pure func set(editable self, value: Int) {
    self.value = value
  }
}

pure func build() Cell {
  let mut cell = Cell(value: 0)
  cell.set(1)          // cell is fresh here, so the footprint imports nothing
  return cell
}

func main(console: Console) {
  let cell = build()
  console.print("${cell.value}")
}
```

```output
1
```

The same call through a closure that captured the cell `editable` writes storage the closure's caller can see,
so it is `mutate`, and a `pure` function cannot do it (LR 11.3):

```fortis
// reject: E-EFF-001
class Cell {
  export value: Int
  pure func set(editable self, value: Int) {
    self.value = value
  }
}

pure func viaClosure() Int {
  let mut cell = Cell(value: 0)
  let mut poke = fn mut[editable cell]() Void {
    cell.set(2)
  }
  poke()                        // reject: E-EFF-001
  return 0
}

func main() {
  discard viaClosure()
}
```

### Callbacks and lambdas

A callback parameter contributes the effects its function type permits, not those of whatever argument is
passed. A lambda literal whose inferred summary has none of the effects `pure` excludes is itself `pure`, so it
can be passed where a `pure fn` is required without writing the modifier (LR 11.3). `countWhere` takes a
`pure fn` (LR 6.7):

```fortis
// accept
import fortis.host { Console }

pure func countAbove(items: List[Int], threshold: Int) Int {
  return items.countWhere(fn[threshold](n: Int) Bool => n > threshold)
}

func main(console: Console) {
  console.print("${countAbove(List.of(3, 8, 1, 9), 5)}")
}
```

```output
2
```

### What crosses a module boundary

Inside a module, callers see each function's inferred summary. Across a module boundary only the written claims
count: a caller in another module may rely on `pure` or `noalloc` only when the callee writes it (LR 11.3).
Removing a written `pure` or `noalloc` from an exported function, or adding `async`, `try` or `recovers`, is a
change to its public API; `fortis check --api-baseline OLD` reports it as the warning `E-EFF-040` at the
declaration (LR 11.3).

A few functions in the `fortis` package have no Fortis body: they are declared `trusted` and the backend supplies
their implementation. A trusted claim is reported as `trusted` evidence, never as `proved`, and `trusted` is
rejected outside the `fortis` package (LR 11.3).

## `noalloc`

`noalloc` claims that a function never allocates (LR 11.4). The checker decides it by matching what the function
lowers to against a whitelist of allocation-free patterns (LR C.3), not by inspecting optimised code: integer
arithmetic, comparisons, field access and reading a list element are allocation-free, while building a list or a
string is not.

```fortis
// accept
import fortis.host { Console }

pure noalloc func clamp(v: Int, lo: Int, hi: Int) Int {
  if v < lo {
    return lo
  }
  if v > hi {
    return hi
  }
  return v
}

pure noalloc func firstOrZero(xs: List[Int]) Int => xs.get(0) ?? 0

func main(console: Console) {
  console.print("${clamp(150, 0, 100)} ${firstOrZero(List.of(7, 8))}")
}
```

```output
100 7
```

```fortis
// reject: E-EFF-001
noalloc func pair(a: Int, b: Int) List[Int] => List.of(a, b)   // reject: E-EFF-001

func main() {
  discard pair(1, 2)
}
```

`noalloc` is a property under a target plan and a telemetry plan: a library is checked with its own plan, and the
build checks the claim again against the application's. If the application's telemetry policy would attach an
allocating record to a `noalloc` function, the build is rejected at the policy (`E-EFF-030`), rather than the
instrumentation being dropped quietly (LR 11.4). For an `async` function, `noalloc` covers creating the future,
every poll, completion and destruction (LR 11.4).

## Contracts

A **contract** names a set of claims so that it can be applied by name (LR 11.5). It may contain only modifiers,
forbidden effects (`forbids recovers`), required authorities (`requires capability Clock`) and excluded
authorities (`excludes capability Network`). Because a contract is a set of modifiers, it is written where
modifiers are written, before the single-word modifiers, and never with `@` (LR 11.5).

```fortis
// accept
import fortis.host { Console }

export contract FastCalculation {
  pure
  noalloc
}

export FastCalculation pure noalloc func square(value: Int<-1000000..1000000>) Int => value * value

func main(console: Console) {
  console.print("${square(-12)}")
}
```

```output
144
```

The formatter keeps the contract's name and prints the modifiers it expands to after it, so a reader of
formatted code sees both (LR 11.5).

A contract only subtracts: applying one never changes what a function does; it makes the checker reject a body
that does something the contract rules out (LR 11.5). Here `NoSilentRecovery` forbids the `recovers` effect, so
the function must handle the failure some other way:

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

contract NoSilentRecovery {
  forbids recovers
}

NoSilentRecovery recovers func quiet(n: Int) Int => half(n) or recover 0   // reject: E-EFF-001

recovers func main() {
  discard quiet(4)
}
```

An `excludes capability` clause is transitive: the function may not take the excluded authority, and may not
call, directly or through any callee, a function that requires it (LR 11.5).

```fortis
// reject: E-EFF-001
import fortis.host { Network }

contract Offline {
  excludes capability Network
}

func lookup(net: Network, name: StringView) Int => name.scalarCount()

Offline func viaNetwork(net: Network) Int => lookup(net, "example")   // reject: E-EFF-001

func main(network: Network) {
  discard viaNetwork(network)
}
```

Contracts compose by conjunction, and contradictory ones are rejected; a `pure` function may not take a
capability parameter at all, so `pure` together with `requires capability` is a contradiction (LR 11.5).
Project configuration never attaches or selects a contract: a contract applies to a function only by appearing
in its head (LR 11.5).

## Capabilities

I/O needs authority, and authority is a value. A function with the `io` effect must take an **authority value**
as a parameter: a nominal, non-copyable value implementing the marker interface `Authority` (LR 11.6). The
process-wide authorities are the **capabilities** `FileSystem`, `Network`, `Clock`, `Random`, `Environment`,
`Process`, `Supervisor`, `Console`, `SecretAccess` and `RawSql`. The runtime creates them once and hands them to the
entry `main`, which declares the ones it needs as parameters: `func main(console: Console, files: FileSystem)`
(LR 4.6, LR 11.6). No other function receives them from the runtime, and none can create them. Older code creates
them in `main` with `Host.capabilities()`, which `fortis fmt` rewrites into parameters; anywhere else that call is
rejected (LR 11.6):

```fortis
// reject: E-EFF-010
import fortis.host { Host }

func sneak() {
  let caps = Host.capabilities()      // reject: E-EFF-010
}

func main() {
  sneak()
}
```

From `main` the authorities are passed down explicitly, so a function's signature names every authority it
uses. A function that reaches I/O without taking an `Authority` value, for example by reaching into the
`Capabilities` record `Host.capabilities()` returns, is rejected with the same code, at the statement that performs
the I/O (LR 11.6):

```fortis
// reject: E-EFF-010
import fortis.host { Capabilities, Host }

func write(caps: Capabilities, text: StringView) {
  caps.console.print(text)            // reject: E-EFF-010
}

func main() {
  let caps = Host.capabilities()
  write(caps, "hello")
}
```

Pass the `Console` itself; the parameter is the visible statement that this function writes to the console:

```fortis
// accept
import fortis.host { Console }

func say(console: Console, text: StringView) {
  console.print(text)
}

func main(console: Console) {
  say(console, "hello")
}
```

```output
hello
```

Capabilities are attenuated by their own methods, never widened: `fs.subtree(path)` gives a `FileSystem`
confined below `path` (LR 11.6). The static rule is only that the authority's type is in the signature; which
directory or host a value reaches is decided by the value and enforced by the runtime adapter (LR 11.6).

### Sharing one capability between owners

A capability is not copyable, and no field stores a loan (LR 7.7). When several values each need the console,
hold it through an explicit `Shared[Console]`: a `Shared[A]` of an authority carries `A`'s authority, so a
parameter of that type satisfies the rule exactly as an `A` parameter does (LR 11.6). Each `retain()` is visible
where a new owner takes a handle (LR 7.8).

```fortis
// accept
import fortis.host { Console }

func announce(console: Shared[Console], who: StringView) {
  console.print("hello from ${who}")
}

func main(out: Console) {
  let console = Shared.create(move out)
  announce(console.retain(), "one")
  announce(console, "two")
}
```

```output
hello from one
hello from two
```

**Per-activation authorities** such as `RequestContext`, `Db` or `TaskContext` are minted by a kit's adapter
from a capability for one unit of work and passed to it as a parameter, so every authority still traces back to
`main` (LR 11.6).

Reading a clock is `nondet` and requires a `Clock`; a `pure` function receives the time as an ordinary
parameter instead (LR 11.6). Capabilities, effects and **target feature sets** are three different things: a
feature set (`core`, `alloc`, `host`, `async`, `web`, `browser`) decides which standard modules exist on a
compilation target, an effect is a fact about a function, and a capability is a run-time value (LR 11.6).

## The observation plane

Observation is the one place where a function may "do" something without its summary changing. The records the
compiler generates for `capture`, `record` and completion events go to a **diagnostic plane** that can never feed
a value back into the program, so instrumenting a `pure` function leaves it `pure` (LR 11.7). Optimisation must
preserve a program's results and application effects always, and the records an observation schema declares
required as well (LR 11.7). Observation schemas and the two statements are the subject of
[Observation](13-observation.md).

`report` sits on the same plane. It is `io`, so a `pure` function cannot report, but it is the one I/O operation
that needs no capability value (LR 10.6, LR 11.6):

```fortis
// accept
import fortis.host { Console }

export error Odd {
  code: "demo.odd"
}

try func half(n: Int) Int {
  if n % 2 == 1 {
    fail Odd()
  }
  return n / 2
}

func halfOrReport(n: Int) Int {
  return half(n) catch e {
    report e
    recover 0
  }
}

func main(console: Console) {
  console.print("${halfOrReport(10)} ${halfOrReport(3)}")
}
```

```output
5 0
```

`halfOrReport` does not need `recovers`: the failure was reported before `recover`, so nothing was swallowed
(LR 10.4). The report itself is a structured record on standard error, not part of the program's output.

## Aspects

Where a contract subtracts, an **aspect** adds: code that runs before a function's body, code that runs after it
succeeds, a handler for failures the body leaves unhandled, and metadata a library can read (LR 11.8). Aspects
are how kits attach routes, authorisation and limits to ordinary functions, and how a team writes its own
conventions. They are ordinary declarations, typed, checked and lowered like any other code (LR 11.8).

### Declaring an aspect

`aspect name(params) { parts }` declares one, optionally with a type parameter `R` after the name. The name is
lowercase, `R` is bound at each application to the target function's success type, and the parts come from a
closed set (LR 11.8):

| Part | Meaning |
|---|---|
| `needs name: Type` | the target must have exactly one parameter of this type; the hooks use it by this name |
| `needs Contract` | the target must carry this contract |
| `before { … }` | runs before the body |
| `after(result: R) { … }` | runs after a successful body, with a read loan of the result |
| `on failure(f: Failure) R { … }` | receives any failure left unhandled, and must `return` an `R` or `fail` |
| `metadata` | marks the arguments as data a library reads through the toolchain |

There is no `around` hook and no access to the body: an aspect can never change what a line inside the function
means, and cannot run the body twice (LR 11.8).

### Applying aspects, and in which order they run

`@name(args)` goes on the lines directly above a function head (LR 11.8). Several aspects elaborate, before any
ownership or effect checking, into one wrapper around the body: the `before` hooks run in application order
(first line first), the `after` hooks in reverse, and the `on failure` handlers innermost first, so the first
line is the outermost wrapper (LR 11.8).

```fortis
// accept
import fortis.host { Console }

export error TooBig {
  code: "demo.too_big"
  payload { value: Int }
}

export aspect logged(label: StringView) {
  needs console: Console
  before {
    console.print("before ${label}")
  }
  after(result: Int) {
    console.print("after ${label}: ${result}")
  }
}

export aspect capped(limit: Int) {
  needs console: Console
  on failure(f: Failure) Int {
    console.print("capped handles ${f.code}")
    report f
    return limit
  }
}

@logged("outer")
@logged("inner")
func square(console: Console, n: Int<0..1000>) Int {
  console.print("body")
  return n * n
}

@capped(limit: 100)
try func checked(console: Console, n: Int) Int {
  if n > 100 {
    fail TooBig(value: n)
  }
  return n
}

try func main(console: Console) {
  console.print("square: ${square(console, 7)}")
  let small = checked(console, 5) or fail
  let big = checked(console, 500) or fail
  console.print("${small} ${big}")
}
```

```output
before outer
before inner
body
after inner: 49
after outer: 49
square: 49
capped handles demo.too_big
5 100
```

The handler reports the failure once and completes the function with `limit`, so for an observer the call
succeeded (LR 11.8). The hooks' effects join the function's summary, and the elaborated function is checked as a
whole: a hook that allocates under `noalloc`, reports under `pure`, or calls `Host.capabilities()` is rejected by
the ordinary rules (LR 11.8). Nothing of an aspect remains at run time.

An application that names an unknown aspect, passes arguments that do not match its declaration, or leaves a
`needs` unmet or ambiguous is `E-TYP-150` (LR 11.8):

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

func main() {
  discard count(3)
}
```

An `after` hook may fail only when the result type is not linear, because a failing `after` would leave a linear
result with no consumer: `E-OBL-008` (LR 11.8).

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

func main() {
}
```

### Specialisations and the core aspect

`aspect name(params, ...rest) = other(fixed: value, ...rest)` declares one aspect as another with some arguments
fixed; `...rest` forwards every remaining named argument (LR 11.8). The core declares exactly one aspect,
`observe(schema)`, which attaches an observation schema to a function; every other aspect is a library
(LR 11.8, LR 13).

```fortis
// accept
import fortis.host { Console }

export aspect traced(label: StringView, depth: Int = 1) {
  needs console: Console
  before {
    console.print("enter ${label} at depth ${depth}")
  }
}

export aspect topLevel(label: StringView, ...rest) = traced(label: label, depth: 0, ...rest)

@topLevel("work")
func work(console: Console, n: Int) Int => n

func main(console: Console) {
  console.print("${work(console, 3)}")
}
```

```output
enter work at depth 0
3
```
