# Fortis

![The Fortis emblem: a fortress tower set in a shield](website/assets/logo-256.png)

**Written by AI. Reviewed by people.**

AI agents now write a lot of the code we ship, and they write it faster than anyone can read it. Fortis is a
programming language built for that world. Every decision that matters, who owns a value, what can change it,
what can fail and who handles it, what a function is allowed to do, is written in the source, so a person reading a
file or a diff can see it at a glance and trust what they approve.

[Website](https://fortis.picktech.co.uk) · [Tour](docs/guide/tour.md) · [Guide](docs/guide/README.md) ·
[For agents](docs/agent/README.md) · [Release notes](docs/releases/v0.0.2.md)

## Why Fortis

**You can review it without running it in your head.** Fortis puts the important facts on the line where they
happen. A value that may change is declared so, with `let mut` or `editable`, and a method call that changes it
needs no further mark. A function that changes something you hand it is marked at the call:
`normalise(editable readings)`. A function that can fail says so in its signature. You don't have to trace the
whole program to know what one line does.

**Failures can't hide.** A call that can fail must be handled: passed up, reported, or recovered from, and the
compiler checks that it is. Nothing is silently swallowed, so the unhappy path is as easy to review as the happy
one.

**Memory is safe, with no garbage collector.** Every value has one owner. Borrowing and moving are checked, and
cleanup runs exactly once, at a point you can see in the code.

**Promises are checked, not trusted.** Mark a function `pure` and the compiler proves it has no effects. Printing,
files, the network and the clock all need a capability that is handed in explicitly, so nothing reaches the
outside world by accident.

**One program, three ways to run it, one meaning.** The same checked program runs on a reference interpreter, on a
fast development VM, and as a native executable compiled through safe Rust (`#![forbid(unsafe_code)]`). All three
must agree on every program, and the test suite holds them to it.

**Made for agents as well as people.** The documentation comes in two editions: a guide for people, and a
reference written for coding agents, with rules by ID and a fix for every error. Every compiler message is also a
structured record, so an agent can read it and correct its own code. The [`fortis-code`](skills/fortis-code/SKILL.md)
skill and the website's `llms.txt` give agents the whole language in a form they can use.

**The docs can't drift.** Every example in the documentation is checked and run on all three backends whenever the
code changes. If the docs say a program prints something, it does.

## See it

```fortis
// accept
import fortis.host { Console }

export error Empty {
  code: "demo.empty"
}

// The mean of some readings; an empty list is a failure the caller must handle.
pure try func mean(readings: ListView[Int<0..1000>]) Int {
  let n = readings.count()
  if n <= 0 {
    fail Empty()
  }
  let mut total = 0
  for r in readings {
    total = total + r or fail
  }
  return total / n
}

func main(console: Console) {
  let mut readings: List[Int<0..1000>] = List.of(12, 30, 18)
  readings.push(40)
  let m = mean(readings) catch e {
    report e
    recover 0
  }
  console.print("mean of ${readings.count()} readings: ${m}")
}
```

```output
mean of 4 readings: 25
```

That's a whole program. Reading it, you can tell without running anything:

- `mean` can fail (`try`), and `fail Empty()` is where it does.
- `mean` has no side effects (`pure`), and the compiler checks the claim.
- `readings` changes, and its declaration says so: `let mut`. The `push` needs no other marker.
- The caller decides what a failure means here: `catch` reports it, then carries on with `0`.
- The addition can't silently overflow: `total + r` is checked, and `or fail` passes an overflow up.
- Printing needs the console, which `main` declares as a parameter and the runtime supplies.

## Try it in two minutes

You need Python 3 and a Rust toolchain with `cargo`. From a checkout of this repository:

```sh
cargo build --release --manifest-path compiler/Cargo.toml
```

Save the example above as `mean.ft`, then:

| To | Run |
|---|---|
| check it | `bin/fortis check mean.ft` |
| run it | `bin/fortis run mean.ft` |
| test it | `bin/fortis test mean.ft` |
| build a native executable | `bin/fortis build mean.ft --out mean` |
| format it | `bin/fortis fmt --write mean.ft` |

Then take [the tour](docs/guide/tour.md).

## Where it's going

Fortis is released in steps. Each release is a usable piece of the language, with its own documentation.

| Release | What it gives you |
|---|---|
| v0.0.1 | Real programs: modules, functions, classes, enums, ownership and failures, checked, run, built, formatted and tested |
| **v0.0.2** (now) | Code that says only what matters: fewer markers, capabilities as `main`'s parameters, built-in logging |
| v0.1 | Domain modelling: interfaces, generics, refinements and contracts |
| v0.2 | Effects you can trust, property tests, packages with dependencies |
| v0.3 | Concurrency: async, tasks and structured scopes |
| v0.4 | A complete standard library |
| v0.5 and v0.6 | Production and scale: observation, runtime kits, an incremental compiler |
| v1.0 | Stability: editions and migrations, and performance measured against Rust |

v0.0.2 is an early release. Its [known limitations](docs/guide/known-limitations.md) are listed in full, and the
tier-1 platform is Linux x86-64.

## Learn more

- [The tour](docs/guide/tour.md): Fortis from the first program.
- [The guide](docs/guide/README.md): the language chapter by chapter, with [worked examples](docs/guide/worked-examples.md)
  and notes for people coming from [Rust](docs/guide/from-rust.md), [TypeScript](docs/guide/from-typescript.md) and
  [Python](docs/guide/from-python.md).
- [The agent reference](docs/agent/README.md): the same language, written for coding agents.
- [The Language Reference](docs/Fortis_Language_Reference.md): the specification, and the source of truth.

## For contributors

| Directory | Contents |
|---|---|
| [compiler/](compiler) | the production toolchain in Rust (`fortisc`): parser, checker, formatter, typed IR, VM and native lowering |
| [checker/](checker) | the reference checker in Python, which must agree with the production one on every fixture |
| [interpreter/](interpreter) | the definitional interpreter, the reference the other backends are compared with |
| [library/](library) | the standard package `fortis`, written in Fortis |
| [conformance/](conformance) | the conformance corpus and its harnesses, including the documentation checks |
| [website/](website) | the website, built from this repository |
| docs/releases/ | the release notes |

To work on Fortis itself, start with [CONTRIBUTING.md](CONTRIBUTING.md); agents also follow
AGENTS.md. Changes to the language go through the Language Reference first. To report a security
problem, see [SECURITY.md](SECURITY.md).

## Licence

Fortis is developed by Pickering Technologies Ltd and released under the [MIT License](LICENSE).
