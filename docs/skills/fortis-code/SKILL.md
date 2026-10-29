---
name: fortis-code
description: Read, check, fix and format Fortis source (.ft files) through the language's own tools and its agent-mode documentation. Use whenever a task involves reading, searching, reviewing, writing or changing Fortis code - `fortis view` replaces cat, sed, head and grep on .ft files, `fortis check --format json` replaces guessing at errors, `docs/agent/codes.md` gives the correction for each record's code, and `fortis fmt` lays the code out.
---

# Working with Fortis code

Fortis source is read through the language's tools, never through text tools. `fortis view` sees the code as the
compiler does: it knows which `{` opens a body, which word is a keyword used as a name, and where a statement ends.
`grep` and `cat` do not, and they miss what matters in Fortis (parameter modes, effects, failure disposition).

## Read: `fortis view`

Run from the repository root (`bin/fortis`, or `fortis` when installed). It never writes.

| To | Run |
|---|---|
| read a file, a directory or a glob | `fortis view src/orders.ft`, `fortis view src`, `fortis view 'src/**/*.ft'` |
| list declarations (kind, name, modifiers, contracts, aspects, documentation, head) | `fortis view --decls src` |
| read one declaration whole | `fortis view --symbol process src` or `--symbol Order.total` |
| answer a structural question | `fortis view --query async-try src` |
| list the queries | `fortis view --queries` |

The queries answer what text search cannot: `async`, `try`, `async-try`, `pure`, `noalloc`, `trusted`,
`recovers`, `editable-params`, `owned-params`, `or-fail`, `or-recover`, `catch`, `fail`, `return`, `defer`,
`scope`, `capture`, `expect`, `move`, `lambda`, `tests`, `errors`, `classes`, and `aspect:NAME` (`aspect:get` finds
every `@get` route). Each answer is `file:line:column: name`.

Add `--json` for machine-readable output: one JSON object per line, with `kind` one of `line`, `declaration`,
`symbol`, `match` or `query`. JSON is the primary form, and the terminal highlighting is derived from it.

## Check: `fortis check --format json`

`fortis check FILE.ft` or `fortis check DIR` runs every static check: syntax, names, types, ownership,
obligations, effects, contracts and aspects. With `--format json`, each finding is one record per line. A record
has `code` (for example `E-OWN-001`, LR 7.5), `class`, `rule` (the Language Reference rule it enforces), `file`, `line`,
`column`, `message`, `status` (`violated`, `unproved` or `unknown`) and class-specific `fields`; the schema is
`reference/schemas/diagnostic.schema.json`. A record with a `correction` carries the exact edit that fixes it.
Read the record before changing code: `unproved` means the checker could not prove a refinement, not that the
code is wrong. Run `fortis test FILE.ft` or `fortis test DIR` for the tests.

## Format: `fortis fmt`

`fortis fmt --check PATH` lists the files whose layout is not canonical, and `fortis fmt --write PATH` fixes them.
Formatting changes only whitespace: it refuses, and writes nothing, if a result would not be the same program.

## Rules, codes and corrections: the agent mode

The documentation's agent mode, `docs/agent/` (start at `docs/agent/README.md`), states every rule of the Language
Reference by ID, one page per section, with the diagnostic codes of that section and a rejected-then-accepted
pair of programs for each correction. Every example there is checked and run by the real toolchain.

| To | Read |
|---|---|
| fix a record | `docs/agent/codes.md`: the code's class, rule, Language Reference section, trigger and correction, with a link to its correction pair |
| read a record's fields | `docs/agent/records.md`: the diagnostic record and every runtime record (failures, tests, observation, builds) |
| know the rules of a construct | `docs/agent/NN-*.md`: the rules of Language Reference section NN by ID |
| check a rule's exact words | `docs/Fortis_Language_Reference.md`, the source of truth: `LR 7.3` is its section 7.3 |

A correction pattern shows the smallest change that satisfies the rule. Apply the one whose trigger matches the
record's `code` and `fields`; when the record has a `correction`, apply that edit. Never apply a pattern that
weakens a claim (`pure`, `noalloc`, a contract) or discards an outcome to silence a record.

## Prohibitions

- Do not read `.ft` files with `cat`, `head`, `tail`, `sed -n` or `less`, and do not search them with `grep` or
  `rg`. Use `fortis view`.
- Do not weaken a contract, remove an `expect`, or change an expected output to make a check or test pass.
- Do not hand-format Fortis code; run `fortis fmt --write`.
