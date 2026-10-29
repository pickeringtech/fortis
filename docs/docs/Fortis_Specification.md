# Fortis — Specification Overview

**Document:** FSPEC-0.2 • **Date:** 22 October 2026 • **Supersedes:** FSPEC-0.1 (`Fortis_Full_Specification.md`, now this file) • **Status:** normative baseline for Fortis Language 1.0 and the kits; not an implemented or formally proved language.

## Contents

- [1. What Fortis is and why it exists](#1-what-fortis-is-and-why-it-exists)
- [2. How the language works, in one page](#2-how-the-language-works-in-one-page)
- [3. What is in the core and what is a library](#3-what-is-in-the-core-and-what-is-a-library)
- [4. Document map](#4-document-map)
- [5. Epistemic conventions](#5-epistemic-conventions)
- [6. Traceability index](#6-traceability-index)
- [7. Status of decisions and open items](#7-status-of-decisions-and-open-items)
- [8. Glossary](#8-glossary)

## 1. What Fortis is and why it exists

Fortis is a general-purpose, statically checked programming language that compiles to safe Rust for production and runs on a development VM with hot reload. It exists for one reason: most new code is now written by AI agents, and the people who must review, operate and be accountable for that code cannot keep up unless the code answers the reviewer's questions on its face.

A reviewer of a Fortis file or diff can answer these questions from the file and the signatures it imports, without an IDE, a debugger or the bodies of other functions:

| Question | Where the answer is visible |
|---|---|
| Who owns this value, and did this line copy it, borrow it or give it away? | `copy` / `alias` / `move` on every assignment of an existing ownership-bearing value (trivial values such as integers need nothing); `move x`, `editable x` and `alias x` at call sites |
| Can this call change my data? | `editable` on the parameter, at the call site, and before a receiver whose method mutates it |
| Can this call fail, and who deals with it? | `try` on the function; `or fail`, `or recover`, `catch` or `match` at the call; dropping a failure does not compile |
| Does this function do I/O, suspend, fail, swallow a failure, or write outside its own storage? | `pure`, `async`, `try`, `recovers` on the signature; `editable` on the parameters it may write; a capability parameter for any I/O other than `report`. `noalloc` answers the separate allocation question |
| When is this observation recorded, and from which value? | `capture field = expression` at the exact point |
| Is this value optional, false or failed? | Three different types with three different operators (`??`, `||`, `or`) |

The design accepts more words on the page in exchange for fewer things a reader must infer. It rejects the usual sources of hidden behaviour: inheritance, exceptions, implicit conversions (with one exception: a string literal becomes a `String` where one is expected), macros, reflection, hidden clones and reference counts, truthiness, ambient service locators.

Fortis is **library-first**. A program is a set of packages; a package is a set of modules; a module is one file with explicit imports and explicit exports. Everything beyond the core (HTTP, databases, web UI including SPA and PWA, telemetry, jobs, dependency injection) is a library that an application imports. The distribution ships those libraries ("batteries included"), but the language does not depend on them and an application links only what it imports.

## 2. How the language works, in one page

```fortis
// accept — src/report.ft in package acme.reporting
import fortis.text
import fortis.host { FileSystem, Console }
import acme.billing.invoices { Invoice, CustomerId }
import acme.reporting.store { Archive, Invoices }   // illustrative dependency

// A nominal type: a ReportId is an Int, but it is not an Int.
export type ReportId = Int<1..>

export error NothingToReport {
  code: "reporting.empty"
  message: "No invoices were supplied"
}

// `try` says this can fail. `pure` says it does no I/O, reads no clock,
// and mutates nothing it did not create.
export pure try func summarise(invoices: List[Invoice]) String {
  if invoices.isEmpty() {
    fail NothingToReport()
  }
  let mut out = text.Builder.create()           // fresh local storage: mutation is still pure
  for invoice in invoices {                     // read loan of each invoice
    let total = invoice.total() or fail         // propagate failure, keep the success value
    out.appendLine("${invoice.customer}: ${total}")
  }
  return out.finish()
}

// Ownership is explicit when an existing value is involved.
export func archive(store: editable Archive, invoices: owned List[Invoice]) {
  let count = invoices.count()
  store.add(move invoices)                      // `store` changes, as its parameter says; `invoices` is given away
  store.note("archived ${count}")
}

// The entry point. `try main` that fails prints one report and exits 1.
try func main(fileSystem: FileSystem, console: Console, args: List[String]) {
  let invoices = Invoices.load(fileSystem, args) or fail
  let reportText = summarise(invoices) catch e { // explicit binding; must end in recover/return/fail
    report e                                    // consumes the failure; it cannot also be propagated
    recover "no report"
  }
  console.print(reportText)
}
```

Reading top to bottom:

1. **Modules and imports.** `import` binds a module alias or named members; nothing runs at import time. `export` is the only visibility word.
2. **Types are nominal.** `type ReportId = Int<1..>` is a new type, not an alias. Refinements like `Int<1..>` are checked by a published interval analysis; when the analysis cannot prove a bound, the program must call an explicit `validate`.
3. **Functions declare what they are.** `async`, `try` and `recovers` are written wherever a function has those effects, because they change its type; `pure` and `noalloc` are checked claims a function makes when it wants callers to rely on them. All five are computed from a closed set of eight effects, and across a module boundary only what is written counts, so a body change cannot silently change a caller's contract.
4. **Ownership is written down.** `copy`, `alias`, `move` on assignment of an existing value; `editable` and `owned` on parameters, mirrored by `editable x` and `move x` at the call and before a receiver whose method mutates or consumes it. Loans are lexical: a loan held by a binding ends with that binding's block, a loan taken for a synchronous call ends when the call returns, and a loan taken for an `async` call is held by its future until that completes; nothing is inferred from later uses.
5. **Failures are linear.** An `Outcome` must be consumed exactly once. `or fail` propagates; `or recover v` discharges; `catch e { … }` handles; `report e` consumes it into the diagnostic plane. There is no way to drop one.
6. **Three different "no".** `T?` is optional and uses `??`; `Bool` uses `||`; `Outcome` uses `or`. None converts to another.
7. **Cleanup and async are structured.** `defer { }` runs at every exit of its block; `async`/`await` with cancellation observed only at `await` (and at `checkpoint()` on the parallel and blocking pools); `scope name { }` joins or cancels every task it launched before it exits.
8. **Observation is a statement.** `capture field = expr` snapshots a value into the activation's frame; the checker proves every required field is captured exactly once on every exit.

The Language Reference defines each of these precisely, with accept/reject/execute examples and the rules a conformance suite will test.

## 3. What is in the core and what is a library

| Layer | Contents | Versioned as |
|---|---|---|
| **Fortis Language 1.0 (core)** | grammar; modules, packages, imports; statements; types and refinements; ownership and loans; closures; classes, interfaces, mixins, enums; linear outcomes; closed effect set and contracts; synchronous `defer`; `async`/`await`/`scope` on the core cooperative executor and pools; observation schemas and statements; test declarations; the core runtime inventory (executor, task reporter, capability types, `Host.capabilities()`); the `fortis` core modules (`prelude`, `text`, `collections`, `numeric`, `time`, `money`, `host`); VM; Rust lowering; conformance suite | one language version with a compatibility promise |
| **Kits** (libraries) | Runtime host; Dependency injection; HTTP; Data (PostgreSQL, SQLite, jobs, cache, mail); Telemetry (OTel, archive, FinOps); Web UI (`fortis.web.ssr`, `fortis.web.components`, `fortis.web.forms`, `fortis.web.spa`, `fortis.web.pwa`); Security; Configuration and deployment (`fortis.config`) | each kit has its own version, gate and acceptance application; published together as a compatibility set |
| **Toolchain components** | Developer tooling (daemon, LSP, reload, replay, agent protocol, dashboard; the `explore` head); Packages and scaffolds (registry client, lockfile, scaffold engine); SDK generators | delivered with their own gates; not kits (PROG-PRD-38) |

A kit is an ordinary set of packages: it obeys every language rule, enters a module only through the four extension slots the core reserves (heads, `inject` members, aspects, test tables), and can ship a new version without a new language version. The Runtime host kit supplies adapters (Tokio, I/O readiness, timers) for the executor interface the core defines; the core runs without it. The web UI kit, including SPA navigation and PWA support, is deliberately a library with a lower delivery priority than the HTTP and data kits; an application that does not import it gets none of it.

## 4. Document map

| Document | Purpose | Normative IDs |
|---|---|---|
| `Fortis_Specification.md` (this file) | purpose, orientation, conventions, traceability | none |
| `Fortis_Language_Reference.md` | the core language: syntax, static rules, execution semantics, lowering reference, keyword inventory, closed decisions | `LANG-<section>-<nn>` |
| `reference/grammar/fortis.ebnf` | the normative grammar | — |
| `Fortis_Platform_Specification.md` | the kits as libraries: runtime host, DI, HTTP, data, telemetry, web UI, security, packages, configuration, developer tooling, standard contracts | `PLAT-<kit>-<nn>` |
| the Delivery Programme (kept with the maintainers' planning records) | release boundary, planned artefacts, requirements register, compiler and VM implementation, diagnostics, testing, exploration and agent protocol, non-functional requirements, threat model, proof spikes, gates, acceptance applications, evidence rules, risks, governance | `PROG-<family>-<nn>` |
| the decision register (kept with the maintainers' planning records) | the FSPEC-0.1 review register with the decision recorded for every item | review item IDs |

Read the Language Reference first if you want to write or review Fortis code. Read the Platform Specification if you are building an application. Read the Delivery Programme if you are implementing the compiler or planning the release.

## 5. Epistemic conventions

These conventions apply to all four documents and are not repeated in them.

- **MUST / MUST NOT** are requirements of a conforming implementation or release. **SHOULD** needs a recorded justification for an exception. **MAY** is a permitted choice.
- **Evidence states** are `proved`, `generated_checked`, `tested`, `measured`, `trusted`, `unknown` and `violated`. They are never collapsed into a score. `proved` means that the production checker and the definitional interpreter agree on the conformance corpus for the rule in question (Delivery Programme, section VM, parity and the definitional interpreter); it is never a claim about business correctness.
- **No example has been compiled.** Every code block is illustrative until the conformance suite derived from the Language Reference runs it. Examples carry `// accept`, `// reject: E-…` or `// execute: …` markers stating what the suite must establish, or `// illustrative` when they are fragments or templates that are not fixtures. The `conformance/` directory holds the G0 corpus extracted from those markers and the syntax-layer runner that parses it against the grammar.
- **Guarantees have scopes.** Every guarantee names its checker, its scope and its trust assumptions. The language does not guarantee correct requirements, termination, delivery over a network, absence of denial of service or optimal performance for every abstraction. Safe memory does not sandbox a process.
- **A contradiction is a defect.** Where two documents disagree, the Language Reference wins on language semantics, the Platform Specification on kit behaviour and the Delivery Programme on process; the disagreement is recorded and fixed, never resolved silently by an implementer.
- **Green agent summaries are not evidence.** Release evidence is executed, content-addressed and independently verified (Delivery Programme, evidence and anti-gaming rules).

## 6. Traceability index

Requirement identifiers from FSPEC-0.1 were renumbered into three families. Each document carries its own mapping table; this index says where to look.

| Old family | New family | Mapping table |
|---|---|---|
| SYN, TYP, OWN, FUN, OO, GEN, MOD, ERR, EFF, RES, ASY, OBS-001/002/006 | `LANG-` | Language Reference, Appendix D |
| API, DAT, JOB, CAC, WEB, SEC (adapters), PKG, SCF, CFG, SCH, DEP, TEL, FIN, DI, STD, APP, OBS-003/004/005, VM-002…004 | `PLAT-` | Platform Specification, Appendix A |
| FND, ARC, CMP, VM-001/005, DIA, TST, EXP, AGT, NFR, SEC (threat model), DEL, DOC | `PROG-` | Delivery Programme, section Requirements register |

Review items (`R-`, `G-`, `T-`, `O-`, `F-`, `E-`, `C-`, `D-`, `I-`, `B-`, `P-`, `V-`, `N-`) map to the sections that resolved them in the decision register, under each item's **Decision** line.

## 7. Status of decisions and open items

**Decided in FSPEC-0.2** (full list in the Language Reference, Appendix B): `let`/`let mut` declarations; `give` for match-arm values; explicit `catch e`; call-site `move`/`editable`; closure kinds `fn`/`fn mut`/`fn once`; lexical loans with no partial moves; linear outcomes with `or fail` / `or recover` / `catch`; a closed set of eight effects and the removal of `nostate`; block-form synchronous `defer` only; cooperative single-thread executor with `scope`; `Shareable` for parallel work; identifier case deciding grammatical category; `main` as the entry point and the removal of `or die`; the trivial, copyable, shareable and linear type tables; the interval domain for refinements.

**Open, with an owner in the Delivery Programme:**

| Item | Where it is tracked |
|---|---|
| Annotation burden of proof-required arithmetic on ordinary code | spike SP-05b |
| Asynchronous cleanup (`defer await`) | spike SP-06; parse error until closed |
| Implementation of the specified executor, `scope` and `Shareable` rules | spike SP-16 (the rules are fixed; the spike proves the lowering) |
| Borrowed work on the parallel pools (a scoped scheduler) | not in 1.0; cooperative `launch` and `join` cover borrowed siblings |
| Definitional interpreter and the meaning of "proved" | spike SP-15 |
| Trademark check for the name "Fortis" (classes 9 and 42) | governance, release prerequisite |
| JSON schemas, work-package graph, conformance scenarios, skill definitions, ambiguity corpus | planned artefacts table |

## 8. Glossary

| Term | Meaning |
|---|---|
| **activation** | one execution of a function body; observation frames and freshness are per activation |
| **capability** | a nominal, unforgeable value that carries authority for I/O (`FileSystem`, `Network`, `Clock`, …), passed explicitly |
| **contract** | a named conjunction of modifiers, forbidden effects and required capabilities, written in the modifier position of a function head |
| **aspect** | declared behaviour applied around a function with `@name(args)`: a `before` hook, an `on failure` mapping and metadata |
| **declaration head** | a reserved word (`component`, `context`, `model`, …) whose body a kit validates and elaborates into ordinary declarations |
| **effect** | one of eight Boolean facts about a function: `alloc`, `io`, `mutate`, `nondet`, `suspend`, `block`, `fail`, `recovers` |
| **frame** | the per-activation record that `capture` writes into |
| **linear** | a value that must be consumed exactly once (`Outcome`, `Failure`, `Future`, `Task`, and anything containing them) |
| **loan** | a read-only (`alias`) or exclusive (`editable`) reference to a place, lexically scoped |
| **marker** | `copy`, `alias`, `move`, `editable`: a word that states how a place is used |
| **obligation** | the duty to consume a linear value |
| **place** | a binding, a field path or a constant-indexed array element |
| **kit** | a separately versioned set of packages providing one area of platform behaviour |
| **refinement** | a bound on a built-in type, written `<…>`, checked by interval analysis |
| **scope** | a block that owns tasks and joins or cancels them on exit |
| **shareable** | a type whose values may cross to a parallel or blocking task |
| **trivial** | a type copied implicitly without a marker (closed table in the Language Reference) |
| **view** | a borrowed slice: `StringView`, `BytesView`, `ListView[T]` |
