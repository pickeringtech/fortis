# Fortis

Fortis is a statically checked, compiled, general-purpose programming language designed so that software
written mostly by AI agents can be read, reviewed and trusted by people. A reader of a file or a diff can answer,
without tooling, who owns each value, which calls can change it, which calls can fail and who handles the
failure, what effects a function has, and when an observation is recorded: every one of those decisions is
written in the source ([Language Reference 1](docs/Fortis_Language_Reference.md#1-what-fortis-is)). The checked
program runs on a development VM and compiles to safe Rust.

```fortis
// accept
import fortis.host { Host }

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
    total = Int.add(total, r) or fail
  }
  return total / n
}

export func main(args: List[String]) Void {
  let caps = Host.capabilities()
  let mut readings: List[Int<0..1000>] = List.of(12, 30, 18)
  editable readings.push(40)
  let m = mean(readings) catch e {
    report e
    recover 0
  }
  caps.console.print("mean of ${readings.count()} readings: ${m}")
}
```

```output
mean of 4 readings: 25
```

Every decision is on the page: `editable` marks the one line that changes `readings`; `try` says `mean` can fail,
and its caller must consume that outcome (here with `catch`, which reports the failure and recovers); `pure` is a
promise the checker verifies; integer arithmetic is either proved safe (the division, after the test on `n`) or
checked (`Int.add`); and output needs the `Console` capability, which only `main` can obtain.

## Status

Fortis Language 1.0, the core language, is being built and is not released. The specification is settled apart
from the parts still being decided by open spikes, and the toolchain implements it with three backends that must
agree on every program: a definitional interpreter (the reference), a development VM, and a native backend that
lowers the checked program to a `#![forbid(unsafe_code)]` Rust crate. The tier-1 platform for Language 1.0 is
Linux x86-64 (register row X-143). The platform kits (HTTP, data, web UI, telemetry and the others) are planned
separately and follow the core. Progress is tracked chunk by chunk in the delivery plan
and its status board.

## Building the tools and running a program

The tools need Python 3 (the reference toolchain, no third-party packages) and a Rust toolchain with `cargo`
(the production compiler, which has no external crates). From the repository root:

```sh
cargo build --release --manifest-path compiler/Cargo.toml
```

Save the example above as `mean.ft`, then:

| To | Run |
|---|---|
| check it: syntax, names, types, ownership, outcomes, effects | `bin/fortis check mean.ft` |
| run it on the definitional interpreter | `bin/fortis run mean.ft` (words after the file are `main`'s arguments) |
| run its tests (a file or a package directory) | `bin/fortis test mean.ft` |
| build a native executable | `bin/fortis build mean.ft --out mean` |
| run it on the development VM | `compiler/target/release/fortisc vm mean.ft` |
| read Fortis source as the compiler sees it | `bin/fortis view mean.ft` |
| lay it out canonically | `bin/fortis fmt --write mean.ft` |

Every finding and runtime report is a record: text on a terminal, JSON lines on a pipe, and `--format json` or
`--format text` overrides the choice (Programme 7.4).

## Documentation

The documentation comes in two modes, built from the same sources and checked against the toolchain: every
example is run, and every rule cites the Language Reference section that states it.

- **Human mode**: [the guide](docs/guide/README.md), starting with [a tour](docs/guide/tour.md), with a chapter
  per section of the Language Reference, [worked examples](docs/guide/worked-examples.md), and notes for readers
  coming from [Rust](docs/guide/from-rust.md), [TypeScript](docs/guide/from-typescript.md) and
  [Python](docs/guide/from-python.md).
- **Agent mode**: [the reference for agents](docs/agent/README.md): rules by ID, record schemas and correction
  patterns, for AI agents writing Fortis. Agents should also install the `fortis-code` skill
  ([skills/fortis-code](skills/fortis-code/SKILL.md)) with `bin/fortis skill install`.
- **Website**: `python3 -I conformance/tools/site.py` builds a static site from both modes, with search, offline.
- **Specifications**, the source of truth: the [overview](docs/Fortis_Specification.md), the
  [Language Reference](docs/Fortis_Language_Reference.md), the
  [Platform Specification](docs/Fortis_Platform_Specification.md) (the kits), the
  Delivery Programme (requirements, gates and evidence), and the
  decision register.

## Repository map

| Directory | Contents |
|---|---|
| [compiler/](compiler) | the production toolchain in Rust (`fortisc`): parser, checker, formatter, `fortis view`, typed IR, validator, VM and native lowering |
| [checker/](checker) | the reference checker in Python, which must agree with the production one on every fixture |
| [interpreter/](interpreter) | the definitional interpreter, the oracle the other backends are compared with |
| [library/](library) | the standard package `fortis`, written in Fortis with trusted intrinsics |
| [reference/](reference) | the grammar (`fortis.ebnf`), the record schemas and the library signatures |
| [conformance/](conformance) | the conformance corpus and its harnesses, including the documentation checks |
| [acceptance/](acceptance) | the acceptance applications, each run on every backend |
| [examples/](examples) | small runnable programs |
| docs/delivery/ | the delivery plan: chunks, gates, status and evidence |
| spikes/ | the proof spikes: preregistrations, experiments and decision records |
| [skills/](skills) | the official agent skill `fortis-code` |

## Contributing

Agents and people working on Fortis itself follow [AGENTS.md](AGENTS.md); changes to the language go through the
specification and the decision register first, and every chunk of work passes its own scripted criteria
(the delivery plan).

## Licence

Fortis is developed by Pickering Technologies Ltd and released under the [MIT License](LICENSE).
