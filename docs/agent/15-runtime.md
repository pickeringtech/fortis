# Core runtime inventory (LR 15)

<!-- covers: LR 15 -->
*Agent mode. Human mode: [The core runtime](../guide/15-runtime.md). Source of truth: LR 15.*

> Preview: the core runtime inventory (LR 15) arrives in v0.4.

LR 15 has no rule table of its own; its components are bound by rules elsewhere: LANG-04-06 (LR 4.6),
LANG-11-07 (LR 11.6), LANG-12-04 (LR 12.5), LANG-13-01 (LR 13). A conforming implementation ships these so that a
program importing no kit runs; kits add adapters and policy and never replace them (LR 15).

## Components

| Component | Core provides | Kits add | LR |
|---|---|---|---|
| Executor | one cooperative executor per process (root task and every `scope`); parallel and blocking pools; cancellation and deadlines | Tokio adapter, I/O readiness, timers, channels (Runtime host kit) | LR 15, LR 12.5 |
| Task reporter | sink for `report`, cleanup reports, `log` records, `not_started` records, completed `capture` frames; default: reports plus each frame's schema, outcome class and duration to stderr, never a captured field value; `FORTIS_TELEMETRY=permit-all` puts every field on the record stream | exporters, archive, projections (Telemetry kit) | LR 15 |
| Authorities | `Authority`; the ten capabilities, supplied to the entry `main`'s parameters by type; `Host.capabilities()` in `fortis.host`; the rule for per-activation authorities | minting adapters, attenuation helpers, sandboxing | LR 15, LR 11.6 |
| Observation schemas | the `observation` head | registry and pipelines (Telemetry kit) | LR 15, LR 13 |
| Entry | `main` in four forms, its parameters supplied by type; one report and exit status 1 on failure | the `application` head and lifecycle (Runtime host kit) | LR 15, LR 4.6 |
| Standard modules | `fortis.prelude`, `fortis.text`, `fortis.collections`, `fortis.numeric`, `fortis.time`, `fortis.money`, `fortis.host` | everything else under `fortis.*` | LR 15 |
| Secrets | `Secret[T]` (no codec, no capture, fingerprint display), `reveal(secret, cap: SecretAccess)` | `declassify`, `Sanitiser`, authentication adapters (Security kit) | LR 15 |

## Capabilities and their core operations

Created once, only for the entry `main`: the runtime passes each capability `main` declares as a parameter, by its
type (`func main(console: Console, fileSystem: FileSystem)`, LR 4.6), and `main` lends them to the functions it
calls (LR 11.6). Older code calls `Host.capabilities()` once in a `main` with no capability parameter; the fields of
the returned `Capabilities` are `fileSystem`, `network`, `clock`, `random`, `environment`, `process`, `supervisor`,
`console`, `secretAccess`, `rawSql` ([`fortis.host`](../../library/fortis/host.ft)).

| Capability | Operations (LR 15) | Notes |
|---|---|---|
| `FileSystem` | `readText(path) String`, `writeAll(path, text)`, `readAll(path) Bytes`, `open(path) File`, `create(path) File`, `subtree(path) FileSystem` | all but `subtree` are `try`; `File`: `readAll`, `writeAll`, `close` |
| `Network` | `resolve(host) String` | `try` |
| `Clock` | `now() Timestamp` (`nondet`), `frozen(instant) Clock`, `sleep(d)` (`async`), `elapsedSince(t) Duration` (`try`) | a `pure` function takes time as a parameter |
| `Random` | `below(bound: Int<1..>) Int<0..>` (`nondet`), `seeded(seed) Random` | |
| `Environment` | `get(name) Secret[String]?` | |
| `Process` | `run(program, arguments: ListView[String]) Int` | `try`, `block` |
| `Console` | `print(text)` | standard output, one line |
| `RawSql` | `execute(statement) Int` | `try` |
| `Supervisor` | none; authorises `scope.detach(move task, supervisor)` | LR 12.5 |
| `SecretAccess` | none; authorises `reveal(secret, cap)` | |

`Path.parse(text: String) Path` is `try` (LR 15). Per-activation authorities (`RequestContext`, `Identity`, `Db`,
`Tx`, `TaskContext`, `Mail`) are minted by their kit's adapter from a capability (LR 15, LR 11.6).

Development adapters of the reference toolchain (library contracts in `fortis.host`, not language rules): file
system rooted in a fresh empty directory per run or `FORTIS_DEV_ROOT` (relative to the working directory; every
backend); network offline except `localhost`;
`Random` SplitMix64 from seed 0 or `FORTIS_DEV_SEED`; `Process` simulates `true` (0) and `false` (1); `RawSql`
fails `sql.unavailable`.

## Standard module contents named by LR 15

| Module | Contents | LR |
|---|---|---|
| `fortis.money` | `Decimal` amount with run-time `Currency`; `Money.of`, `zero`, `add`, `subtract`, `multiply(money, factor: Int)`, `divide(money, divisor: Int<1..>)`, `greaterThan`; arithmetic fails on currency mismatch | LR 15 |
| `fortis.time` | `Duration.days/hours/minutes/seconds/milliseconds`, fallible `Duration.add/sub/multiply`, `d.nanoseconds()`, comparisons on `Duration`, display in the largest exact unit (`150s`, `1h`, `250ms`, `0s`); `Timestamp.epoch()`, fallible `Timestamp.add(t, d)`, `Timestamp.after`, `Timestamp.before` | LR 15 |
| `fortis.host` | capabilities, `Path`, `Secret[T]`, `reveal` | LR 15 |

## Diagnostics

| Code | Trigger | Correction | LR |
|---|---|---|---|
| E-EFF-010 | `Host.capabilities()` outside `main`, a second call in `main` (reported at `main`), a call in a `main` that takes a capability parameter, or `io` other than `report` and `log` reached without an `Authority` parameter | declare the capability as a parameter of `main`; pass it (or a `Shared` of it) down as a parameter | LR 11.6 |
| E-TYP-090 | a `Secret[T]` (or an authority) in an `Encodable` position, such as an observation field | capture or encode a non-secret projection | LR 15, LR 6.10 |

## Correction patterns

### E-EFF-010: capabilities created outside `main` (LR 11.6)

```fortis
// reject: E-EFF-010
import fortis.host { Host }

func greet() {
  let caps = Host.capabilities()     // reject: E-EFF-010
}

func main() {
  greet()
}
```

```fortis
// accept
import fortis.host { Console }

func greet(console: Console) {
  console.print("hello")
}

func main(console: Console) {
  greet(console)
}
```

```output
hello
```

### E-EFF-010: `Host.capabilities()` in a `main` that takes capabilities (LR 11.6)

A `main` with a capability parameter already has its capabilities; a call of `Host.capabilities()` there is a
second creation.

```fortis
// reject: E-EFF-010
import fortis.host { Console, Host }

func main(console: Console) {
  let caps = Host.capabilities()     // reject: E-EFF-010
  console.print("ready")
}
```

```fortis
// accept
import fortis.host { Console }

func main(console: Console) {
  console.print("ready")
  console.print("once")
}
```

```output
ready
once
```

## Patterns

Failing `try main` (exit status 1, one `runtime-failure` record on stderr naming code, payload and origin;
LR 4.6):

```fortis
// accept (exit status 1)
import fortis.host { Path, FileSystem, Console }

try func main(fileSystem: FileSystem, console: Console) {
  let path = Path.parse("missing.txt") or fail
  let text = fileSystem.readText(path) or fail
  console.print(text)
}
```

Revealing a secret (LR 15):

```fortis
// accept
import fortis.host { reveal, Environment, SecretAccess, Console }

func main(environment: Environment, secretAccess: SecretAccess, console: Console) {
  let home = environment.get("HOME")
  if home != null {
    let value = reveal(home, secretAccess)
    console.print("${value.byteCount() > 0}")
  }
}
```

```output
true
```

Time arithmetic (LR 15):

```fortis
// accept
pure try func deadline(start: Timestamp, slack: Int<0..60>) Timestamp {
  let wait = Duration.add(Duration.minutes(5), Duration.seconds(slack)) or fail
  return Timestamp.add(start, wait) or fail
}

test "deadline is later" {
  let t = deadline(Timestamp.epoch(), 30)
  expect Timestamp.after(t, Timestamp.epoch())
}
```
