# Observation

<!-- covers: LR 13 -->
*Human mode. Agent mode: [Observation, by rule](../agent/13-observation.md). Source of truth: LR 13.*

> Preview: observation schemas, `capture` and `record` (LR 13) arrive in v0.5. Log statements, `log.info`, `log.warn` and `log.error` (LR 13), are in v0.0.2.

Logs and metrics are usually an afterthought: a line printed here, a counter bumped there, and nobody can say
which facts every request is guaranteed to record. Fortis makes observation part of the language. A function
declares, with a schema, which facts its activation records; the body assigns them with `capture`; and the
checker proves that every required fact is assigned on every way out of the function (LR 13).

The core defines only three things: the `observation` schema declaration and the two statements `capture` and
`record`. Where the records go, how they are redacted, sampled and exported is the Telemetry kit's job, not the
language's (LR 13, LR 15).

## Declaring a schema

A schema is a declaration with typed fields. Each field says when it is required, and the schema may carry a
version (1 when omitted) and an encoding bound (LR 13).

```fortis
// accept
import fortis.host { Console }

observation Checkout {
  version: 1
  items: Int                       required: all
  total: Int                       required: success
  coupon: String<scalars: 0..16>   required: none
  encoding: bounded(256)
}

@observe(Checkout)
pure func checkout(prices: List[Int<0..1000>], coupon: String) Int {
  capture items = prices.count()
  capture coupon = coupon
  let mut total = 0
  for price in prices {
    total = Int.saturatingAdd(total, price)
  }
  capture total = total
  return total
}

func main(console: Console) {
  let prices: List[Int<0..1000>] = List.of(120, 45, 300)
  console.print("total ${checkout(prices, "SPRING")}")
}
```

```output
total 465
```

`required` takes one of four values (LR 13):

| `required:` | The field must be assigned |
|---|---|
| `all` | on every path, before every exit |
| `success` | before every successful exit |
| `failure` | before every failing exit |
| `none` | never; it is optional |

Every field type must be `Encodable`, checked recursively like any derivation (LR 13, LR 6.10). Secrets,
authorities, loans, resources, closures, futures and tasks are not `Encodable`, so none of them can ever appear
in a captured value (LR 13). A `String` field must declare a bound in the schema, here
`String<scalars: 0..16>`; a longer value is truncated or omitted as the schema says, and the frame records which
happened (LR 13).

The schema is attached to the function with the core aspect `@observe(Checkout)` on the line above its head
(LR 13, LR 11.8). Notice that `checkout` is `pure`: capturing does not change a function's effects, because the
records go to a diagnostic plane that can never feed a value back into the program (LR 11.7).

## `capture`: a snapshot into the frame

`capture field = expression` copies the value of the expression into the named field of the *observation
frame* of the current activation, the record the function produces when it finishes (LR 13). The copy is taken
at the statement, so changing the place afterwards does not change the frame:

```fortis
// accept
import fortis.host { Console }

observation Counter {
  start: Int        required: all
  finish: Int       required: all
}

@observe(Counter)
pure func countUp(start: Int<0..100>) Int {
  let mut n: Int<0..200> = start
  capture start = n
  n = n + 50
  capture finish = n
  return n
}

func main(console: Console) {
  console.print("counted to ${countUp(7)}")
}
```

```output
counted to 57
```

The frame for that call holds `start` 7 and `finish` 57: the first capture copied 7 before `n` changed.

The expression of a `capture` must be pure, infallible and non-suspending, and of the field's declared type; it
takes a read loan for the statement only (LR 13). A fallible call or an `await` is written on its own line first,
and the binding is captured.

### What you see when it runs

The frame is not printed by your program; it is a record for the task reporter. By default the reporter writes
each frame's schema and outcome to standard error and never a captured field value: values leave the process
only through the Telemetry kit's pipeline, after redaction (LR 15). To see the values while developing, run with
the development plan `FORTIS_TELEMETRY=permit-all`, which writes every field unchanged to the record stream on
standard error (LR 15).

## Every required field, on every exit

The checker treats the frame as a set of variables that must be definitely assigned, over every path of the
function, including the paths through `fail`, `return`, `await` and cancellation (LR 13). A required field that
is left unassigned on an exit it applies to is `E-OBS-001`, reported at that exit (LR 13):

```fortis
// reject: E-OBS-001
export error Rejected {
  code: "shop.rejected"
}

observation Payment {
  amount: Int        required: all
  reason: Int        required: failure
}

@observe(Payment)
try func pay(amount: Int<0..10000>) Int {
  capture amount = amount
  if amount == 0 {
    fail Rejected()          // reject: E-OBS-001
  }
  return amount
}

recovers func main() {
  let paid = pay(10) or recover 0
}
```

`reason` is required on failure, and the `fail` leaves without it. Capturing it before the `fail` fixes the
function:

```fortis
// accept
import fortis.host { Console }

export error Rejected {
  code: "shop.rejected"
}

observation Payment {
  amount: Int        required: all
  reason: Int        required: failure
}

@observe(Payment)
try func pay(amount: Int<0..10000>) Int {
  capture amount = amount
  if amount == 0 {
    capture reason = 1
    fail Rejected()
  }
  return amount
}

recovers func main(console: Console) {
  let paid = pay(10) or recover 0
  let refused = pay(0) or recover -1
  console.print("paid ${paid}, refused ${refused}")
}
```

```output
paid 10, refused -1
```

An `await` is a cancellation point, so it is an exit too: a `required: all` field must be captured before the
first `await` that could cancel the activation (LR 13, LR 12.4):

```fortis
// reject: E-OBS-001
import fortis.host { Clock }

observation Wait {
  tier: Int        required: all
}

@observe(Wait)
async func slow(clock: Clock, tier: Int) Int {
  await clock.sleep(5ms)     // reject: E-OBS-001
  capture tier = tier
  return tier
}

async func main(clock: Clock) {
  discard await slow(clock, 2)
}
```

## One capture per field, and none in loops

A field is captured at most once on any path: a second `capture` of the same field is `E-OBS-010` (LR 13). A
`capture` inside a loop body is `E-OBS-011`, because the loop might run it many times (LR 13):

```fortis
// reject: E-OBS-011
observation Batch {
  last: Int        required: none
}

@observe(Batch)
pure func sum(items: List[Int<0..100>]) Int {
  let mut total = 0
  for item in items {
    capture last = item      // reject: E-OBS-011
    total = Int.saturatingAdd(total, item)
  }
  return total
}

func main() {
  discard sum(List.of(1, 2))
}
```

Repeated measurements are events, and events are what `record` is for.

## `record`: events

`record Name(field: value, …)` emits an event of the named schema at once, every time it runs, so it belongs
inside loops (LR 13). Its arguments have the same purity requirement as a `capture`.

```fortis
// accept
import fortis.host { Console }

observation Batch {
  count: Int        required: all
}

observation Item {
  value: Int        required: all
}

@observe(Batch)
pure func sum(items: List[Int<0..100>]) Int {
  capture count = items.count()
  let mut total = 0
  for item in items {
    record Item(value: item)
    total = Int.saturatingAdd(total, item)
  }
  return total
}

func main(console: Console) {
  console.print("sum ${sum(List.of(4, 5, 6))}")
}
```

```output
sum 15
```

A `capture` or `record` whose expression has an effect, or that names a field the attached schema does not
declare, is `E-OBS-012`; so is a `capture` in a function with no schema attached, since there is no field it
could name (LR 13):

```fortis
// reject: E-OBS-012
observation Batch {
  count: Int        required: all
}

@observe(Batch)
pure func size(items: List[Int]) Int {
  capture count = items.count()
  capture colour = 3         // reject: E-OBS-012
  return items.count()
}

func main() {
  discard size(List.of(1))
}
```

## Logs

`log.info`, `log.warn` and `log.error` write a log record: its level, its message and the function that wrote it
(LR 13, X-345). Logging is the platform's concern, not the program's, so a log needs no capability and no setup:
the record goes to standard error, as text on a terminal and as one JSON line on a pipe, where the platform's log
collection picks it up. It never mixes into standard output, which stays the program's own product:

```fortis
// accept
import fortis.host { Console }

func total(prices: List[Int]) Int {
  log.info("totalling ${prices.count()} prices")
  let mut sum = 0
  for p in prices {
    sum = Int.wrappingAdd(sum, p)
  }
  return sum
}

func main(console: Console) {
  console.print("total ${total(List.of(3, 4, 5))}")
}
```

```output
total 12
```

A log statement has the `io` effect, like `report`, so a `pure` function cannot log. A failure is recorded with
`report e`; a log is for what is not a failure, such as a fallback taken, progress or a summary. `log` is not a
keyword: a program that already names something `log` keeps it, and there `log.info(...)` is an ordinary call.

## What the compiler adds

Source never writes an activation's start, end, duration or outcome class: the compiler generates them, so they
cannot be forged (LR 13). An activation that is cancelled before it starts produces a frame marked `not_started`
with no captured fields (LR 13). Two backends do not yet do all of this, and both gaps are recorded as defects:
no backend supplies the duration yet (D-28), and native code writes no `not_started` frame (D-27).

Optimisation must keep every record that a schema declares required, so the trace you get from an optimised
build is the trace the source promises; instrumentation that is not required may be merged or elided (LR 11.7).
