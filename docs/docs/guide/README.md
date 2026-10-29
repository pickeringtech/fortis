# The Fortis guide

<!-- covers: LR 0 -->
*Human mode. Agent mode: [the Fortis reference for agents](../agent/README.md). Source of truth: the
[Language Reference](../Fortis_Language_Reference.md).*

This guide teaches Fortis Language 1.0, the core language, from the beginning. It is written for people: each
chapter explains what a construct is for, shows it working, and shows what the checker says when it is misused.
Its companion, the [agent mode](../agent/README.md), states the same rules densely, by rule ID, with the records
the tools print and the corrections that fix them, for AI agents writing Fortis.

Neither mode replaces the [Language Reference](../Fortis_Language_Reference.md) ("the LR"), which is the source
of truth. Every rule in this guide cites the LR section that states it, written `LR 7.3`; if the guide and the LR
ever disagree, the LR wins and the guide has a defect (LR 0).

## Chapters

Start with the tour, then read in order; each chapter follows one section of the LR.

| Chapter | LR |
|---|---|
| [A tour of Fortis](tour.md): what the language is for, the tools, and a whole program | 1, 2 |
| [Lexical structure](03-lexical.md) | 3 |
| [Modules and packages](04-modules.md) | 4 |
| [Statements and expressions](05-statements.md) | 5 |
| [Types](06-types.md) | 6 |
| [Proving arithmetic and indexing](06-proofs.md) | 6.3, 6.4, 6.7 |
| [Ownership](07-ownership.md) | 7 |
| [Functions and closures](08-functions.md) | 8 |
| [Classes, interfaces, mixins and enums](09-classes.md) | 9 |
| [Outcomes and failures](10-outcomes.md) | 10 |
| [Effects and contracts](11-effects.md) | 11 |
| [Cleanup, async and concurrency](12-async.md) | 12 |
| [Observation](13-observation.md) | 13 |
| [Tests](14-tests.md) | 14 |
| [The core runtime](15-runtime.md) | 15 |
| [Kits and declaration heads](16-kits.md) | 16 |
| [Embedded languages](embedded-languages.md): what 1.0 has, and what a later edition plans | 3, 16 |
| [Keywords](appendix-a-keywords.md), [closed decisions](appendix-b-decisions.md), [lowering to Rust](appendix-c-lowering.md), [rule index](appendix-d-rules.md) | A, B, C, D |

Further reading:

- [Worked examples](worked-examples.md): larger programs, each explained line by line.
- Notes for readers coming from [Rust](from-rust.md), [TypeScript](from-typescript.md) and [Python](from-python.md).
- The [glossary](glossary.md): kit, package, head, binding, verifier and the other terms.
- [Known limitations of v0.0.2](known-limitations.md): what this release refuses or does not yet do, and the workaround for each.
- The [core library](../agent/library.md): every operation of the standard modules, with its contract, generated
  from the library's declarations.

## How the examples are checked

Every example in this guide is a program that the real toolchain checks and runs, so the guide cannot drift
from the language. An example's first line says what is expected of it, in the same notation as
the LR (LR 0):

- `// accept`: both checkers accept it, and it runs. A program with `main` runs on all three backends (the
  definitional interpreter, the development VM and native code) and prints exactly the output shown below it.
  `// accept (exit status 1)` marks a program that ends by failing, on purpose. A module without `main` has tests,
  and they pass on all three backends.
- `// reject: E-XXX-NNN` (a real code, such as `E-OWN-011`, LR 7.3): the checkers reject it with exactly that
  code, on the line that carries the same marker, and report nothing else.
- `// illustrative`: a shape with placeholders such as `…`, not a program.

The check is `python3 -I conformance/tools/docset_check.py`; where a backend still has a recorded defect for an
example, the check names the defect, and the example stays as the language says it should behave.
