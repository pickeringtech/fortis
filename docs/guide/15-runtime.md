# The core runtime

<!-- covers: LR 15 -->
*Human mode. Agent mode: [Core runtime inventory, by rule](../agent/15-runtime.md). Source of truth: LR 15.*

> Preview: the core runtime inventory (LR 15) arrives in v0.4.

Fortis is not only a compiler. A conforming implementation ships a small runtime with it, so that a program that
imports no kit at all can still start, run tasks, read files, print, fail cleanly and report what happened.
Kits add adapters and policy on top of these pieces; they never replace them (LR 15). This chapter walks through
the pieces: the executor, the task reporter, the authorities, the entry point, the standard modules and secrets.

## The executor

There is one cooperative executor per process. It runs the root task, which is `main` when `main` is `async`,
and every `scope` the program opens; beside it the core provides a parallel pool and a bounded blocking pool,
and the signalling for cancellation and deadlines (LR 15, LR 12.5).

```fortis
// accept
import fortis.host { Clock, Console, Host }

async func pause(clock: Clock, console: Console) Void {
  console.print("sleeping")
  await clock.sleep(10ms)
  console.print("awake")
}

export async func main(args: List[String]) Void {
  let caps = Host.capabilities()
  await pause(caps.clock, caps.console)
  caps.console.print("main done")
}
```

```output
sleeping
awake
main done
```

An `async` entry runs on the core cooperative executor as the root task, and its completion ends the process
(LR 4.6). A minimal synchronous program starts no executor it did not ask for (PROG-NFR-14).
[Cleanup, async and concurrency](12-async.md) covers scopes, launches and cancellation.

## The task reporter

Some output is not the program's: a failure that `report` writes, a failure raised during cleanup, a
`not_started` record for work cancelled before it began, and the frames that `capture` fills. All of it goes to
the *task reporter*, the sink of the diagnostic plane, which can never feed anything back into the program
(LR 15, LR 11.7).

By default the reporter writes reports, and each frame's schema identity, outcome class and duration, to
standard error. It never writes a captured field value: field values leave the process only through the
Telemetry kit's pipeline, after redaction, and without that kit they are kept in memory for the debugger and
then discarded (LR 15). While developing you can choose the plan `permit-all` by setting
`FORTIS_TELEMETRY=permit-all`, which puts every field on the record stream unchanged; it is an explicit choice,
like any plan (LR 15).

Every record is written as text on a terminal and as one JSON line otherwise, so a program run inside an agent
session is as easy to query as one run under an exporter (LR 4.6, Programme 7.4).

## Authorities and capabilities

Input and output need authority, and authority is a value. The core defines the marker interface `Authority` and
ten *capabilities*, which are process-wide authorities: `FileSystem`, `Network`, `Clock`, `Random`,
`Environment`, `Process`, `Supervisor`, `Console`, `SecretAccess` and `RawSql` (LR 15, LR 11.6). They are
created by exactly one call, `Host.capabilities()` from `fortis.host`, which only `main` may make, and only once
(LR 11.6). From there they are passed, as ordinary parameters, to whatever needs them.

```fortis
// accept
import fortis.host { FileSystem, Host, Path }

try func saveAndLoad(fs: FileSystem) String {
  let path = Path.parse("notes.txt") or fail
  fs.writeAll(path, "first line") or fail
  return fs.readText(path) or fail
}

export try func main(args: List[String]) Void {
  let caps = Host.capabilities()
  let text = saveAndLoad(caps.fileSystem) or fail
  caps.console.print("read back: ${text}")
  let address = caps.network.resolve("localhost") or fail
  caps.console.print("localhost is ${address}")
  let status = caps.process.run("true", List.of()) or fail
  caps.console.print("true exits ${status}")
  let frozen = caps.clock.frozen(Timestamp.epoch())
  caps.console.print("a frozen clock reads ${frozen.now()}")
  let seeded = caps.random.seeded(42)
  caps.console.print("seeded draw: ${seeded.below(100)}")
}
```

```output
read back: first line
localhost is 127.0.0.1
true exits 0
a frozen clock reads 1970-01-01T00:00:00Z
seeded draw: 13
```

`saveAndLoad` can touch the file system because, and only because, it takes a `FileSystem`. A reader can see
from its signature everything it may reach; a function whose body reaches I/O through a capability without taking
an authority is rejected (`E-EFF-010`, LR 11.6), and so is a call to `Host.capabilities()` anywhere but `main`.
`report` is the one exception: it needs the `io` effect but no capability, because it writes only to the
diagnostic plane (LR 11.6, LR 10.6).

```fortis
// reject: E-EFF-010
import fortis.host { Host }

func sneak() Void {
  let caps = Host.capabilities()     // reject: E-EFF-010
}

export func main(args: List[String]) Void {
  sneak()
}
```

The capability types and their core operations are part of the core runtime inventory (LR 15):

| Capability | Core operations |
|---|---|
| `FileSystem` | `readText`, `writeAll`, `readAll`, `open`, `create`, `subtree`, and the `File` that `open` and `create` return |
| `Network` | `resolve` |
| `Clock` | `now`, `frozen`, `sleep`, `elapsedSince` |
| `Random` | `below(bound: Int<1..>)`, `seeded` |
| `Environment` | `get` |
| `Process` | `run` |
| `Console` | `print` |
| `RawSql` | `execute` |
| `Supervisor`, `SecretAccess` | no operations of their own: they authorise `scope.detach` (LR 12.5) and `reveal` |

`fortis.host` also defines `Path`, built from text by `Path.parse`, which fails on invalid text (LR 15). Kits add
the remaining operations, attenuation helpers and the adapters behind them (LR 15).

*Per-activation authorities* are the other kind: `RequestContext`, `Identity`, `Db`, `Tx`, `TaskContext` and
`Mail`, each minted by its kit's adapter from a capability, for one unit of work, and handed to it as a parameter.
The core states only the rule; the kits define the types (LR 15, LR 11.6).

### The development adapters

The reference toolchain's adapters are built for development and tests, and each one's contract is written above
it in [`fortis.host`](../../library/fortis/host.ft): the file system is rooted in a fresh empty directory for each
run, or in the directory `FORTIS_DEV_ROOT` names (a path relative to where the command runs), on every backend,
so a program's input files live in a directory of their own, given with `FORTIS_DEV_ROOT=data fortis run app`;
the network is offline except that `localhost` resolves, `Random` is a seeded
generator (`FORTIS_DEV_SEED`), `Process` only simulates `true` and `false`, and `RawSql` has no database. These
are the development adapters' behaviour, not rules of the language. The language's part is the pair of core
operations `Clock.frozen` and `Random.seeded` (LR 15); with them, the example above prints the same thing on
every run.

## The entry point

`main` comes in four forms: plain, `try`, `async` and `async try` (LR 4.6, LR 15). If a `try main` fails, the
runtime writes exactly one report of the failure to standard error and exits with status 1 (LR 4.6). Here the
development file system starts empty, so the file is not there:

```fortis
// accept (exit status 1)
import fortis.host { Host, Path }

export try func main(args: List[String]) Void {
  let caps = Host.capabilities()
  caps.console.print("reading config.txt")
  let path = Path.parse("config.txt") or fail
  let text = caps.fileSystem.readText(path) or fail
  caps.console.print(text)
}
```

```output
reading config.txt
```

The one report on standard error names the failure's code, `fs.not_found`, its payload (the path) and the
function it came from. There is no `or die`: a program that wants to stop on a failure propagates it to `main`
(LR 4.6).

## The standard modules

The core package `fortis` provides these modules; everything else under `fortis.…` belongs to a kit (LR 15,
LR 4.3). The [core library](../agent/library.md) lists every operation they export, with its contract. The text
and collection operations are few: there is no line splitting, trimming, searching or number parsing yet, and a
program writes those itself (D-58):

| Module | Provides |
|---|---|
| `fortis.prelude` | the core types, the optional and outcome operators, the built-in interfaces; imported into every module |
| `fortis.text` | `Builder`, `Scalar`, grapheme counts and `Text.words`; LR 6.6 also places normalisation and case mapping here, which the library does not have yet (D-58) |
| `fortis.collections` | `sorted`, for a list of `Int` (LR 6.7) |
| `fortis.numeric` | `abs`, `min` and `max`; LR 6.5 places transcendental functions in `fortis.numeric.math`, which does not exist yet (D-58) |
| `fortis.time` | `Duration` and `Timestamp` operations |
| `fortis.money` | `Money`: a `Decimal` amount with a run-time `Currency`; arithmetic fails on a currency mismatch |
| `fortis.host` | the capabilities, `Host.capabilities()`, `Path`, `Secret[T]` and `reveal` |

`fortis.time` gives `Duration.days`, `hours`, `minutes`, `seconds` and `milliseconds`, the fallible
`Duration.add`, `sub` and `multiply`, comparison operators on `Duration`, and `Timestamp.epoch`, `add`, `after`
and `before`. A `Duration` displays in the largest unit that divides it exactly (LR 15):

```fortis
// accept
import fortis.host { Host }

export try func main(args: List[String]) Void {
  let caps = Host.capabilities()
  let half = Duration.milliseconds(500)
  let total = Duration.add(half, Duration.seconds(1)) or fail
  caps.console.print("${Duration.minutes(90)} ${total} ${half}")
  let later = Timestamp.add(Timestamp.epoch(), Duration.days(1)) or fail
  caps.console.print("${Timestamp.after(later, Timestamp.epoch())}")
}
```

```output
90min 1500ms 500ms
true
```

## Secrets

`Environment.get` does not return a `String`: it returns a `Secret[String]?`, because environment variables are
where credentials live (LR 15). A `Secret[T]` has no codec, cannot be captured into an observation frame, and
displays as a per-process keyed fingerprint, never its value. Reading the value is `reveal(secret, cap)`, which
needs the `SecretAccess` capability, so every place a secret is opened is visible in a signature (LR 15).

```fortis
// accept
import fortis.host { Host, reveal }

export func main(args: List[String]) Void {
  let caps = Host.capabilities()
  let home = caps.environment.get("HOME")
  if home != null {
    let value = reveal(home, caps.secretAccess)
    caps.console.print("HOME is set: ${value.byteCount() > 0}")
  }
}
```

```output
HOME is set: true
```

Declassifying, sanitising and authentication adapters belong to the Security kit (LR 15).
