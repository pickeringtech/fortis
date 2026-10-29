# Glossary (agent mode)

<!-- covers: LR E -->
*Agent mode. Human mode: [Glossary](../guide/glossary.md).*

One meaning per term; where a word has two meanings both rows are listed. A bundle of packages is a kit.
The Language Reference's glossary is LR E; this page adds the in-language terms.

## System

| Term | Definition | Source |
|---|---|---|
| package | unit of distribution, versioning, visibility: `project.yaml`, lockfile, `src/`; two versions are two packages | LR 4.1 |
| module | one `.ft` file under `src/`; name = path below `src/` with `.` separators; declarations and constants only | LR 4.2 |
| kit | named, versioned bundle of packages shipped with the distribution; extends Fortis only by libraries, aspects, heads, embedding bindings, verifiers; never changes core semantics | LR 16 |
| compatibility set | the standard packages at the versions tested together; a standard dependency names the set's version | LR 4.1 |
| toolchain component | daemon, LSP, SDK generators: tools, not kits | LR 16 |
| backend | interpreter (`fortis run`, the reference), development VM (`fortisc vm`), native (`fortis build`); must agree | LR 1 |
| build mode | `dev` (VM), `native-dev`, `release`; optimisation and debug settings belong to a mode | LR 1 |
| lint level | baseline (an unused import warns) or strict (it is an error) | LR 4.3 |
| target feature set | which standard modules exist for a target (`core`, `alloc`, `host`, `async`, `web`, `browser`) | LR 11.6 |

## Declarations

| Term | Definition | Source |
|---|---|---|
| head (declaration head) | reserved keyword introducing a schema-validated declaration elaborated to core declarations before type checking; owner: the core (`observation`, LR 13), developer tooling (`explore`, LR E), or a kit (the rest) | LR 16, LR 13, LR E |
| elaboration | rewriting a head, or a function with aspects, into core declarations before ownership and effect checking | LR 16, LR 11.8 |
| extension slot | head block; `inject` member; aspect application; `cases` table: the only four | LR 16 |
| raw template region | text of `view { … }` in a head body, handed unlexed to the Web UI kit | LR 16 |
| aspect | `@name(args)` behaviour around a function: `needs`, `before`, `after`, `on failure`, `metadata`; adds | LR 11.8 |
| contract | named conjunction of modifiers, forbidden effects, required or excluded authorities; subtracts | LR 11.5 |
| mixin | methods applied with `with`; requires methods, adds no state | LR 9.6 |
| newtype | `type Name = Repr`; nominal, private representation | LR 6.9 |
| refinement | predicate in `<…>` on a built-in type | LR 6.3 |
| binding (name) | name introduced by `let`, `let mut`, a parameter, a pattern or a capture | LR 5.1, LR 7.2 |
| binding (embedded language) | versioned package giving a language fenced blocks via the embedding protocol; meaning compiler-fixed, checking a verifier (SP-23, not yet in the LR) | [Embedded languages](embedded-languages.md) |
| verifier | stricter-only package: reads the checked program, returns records never code; pure, deterministic, bounded, no capability | [Embedded languages](embedded-languages.md) |

## Ownership

| Term | Definition | Source |
|---|---|---|
| place | binding; field path rooted in a binding or `self`; `Array` element with a literal index | LR 7.2 |
| owner | binding holding a value until it is moved out | LR 7.2 |
| loan | `alias` (read) or `editable` (exclusive) reference to a place; lexical | LR 7.3 |
| child loan | a loan taken through a loan-holding binding, recorded against the owner | LR 7.3 |
| marker | `copy`, `alias`, `move`, `editable` before a place; left out where the declaration already shows it (an `editable self` receiver on a `let mut` root, `return local`) | LR 7.5 |
| loan-carrying value | standard views and cursors; store a read loan; owned by their binding | LR 7.7 |
| trivial | duplicated implicitly, never marked | LR 6.11 |
| copyable | supports `copy` | LR 6.11 |
| linear | consumed exactly once on every path | LR 6.12 |
| shareable | may cross `launchParallel` / `launchBlocking` | LR 12.7 |

## Failures, effects, authority

| Term | Definition | Source |
|---|---|---|
| outcome | `Outcome[T]` from a `try` function; linear | LR 10.1 |
| checked region | `Int` arithmetic as the left operand of `or fail`/`or recover`/`catch`; an unproved operator in it is the checked operation | LR 6.4 |
| failure | `Failure`: code, payload, origin, cause chain; linear | LR 10.3 |
| obligation | the duty to consume a linear value | LR 7.1, LR 10.1 |
| defect | broken-assumption condition (stack, memory, trusted contract, runtime invariant); never caught | LR 10.8 |
| effect | one of `alloc`, `io`, `mutate`, `nondet`, `suspend`, `block`, `fail`, `recovers` | LR 11.1 |
| effect summary | a function's effect subset, computed transitively | LR 11.1, LR 11.3 |
| write footprint | `editable` parameters a function writes through | LR 11.3 |
| fresh storage | created in this activation, not yet reachable from a parameter, capture or `self` | LR 11.1 |
| authority | value implementing `Authority`; required for `io` | LR 11.6 |
| capability | process-wide authority (ten types), supplied to the entry `main`'s parameters by type | LR 11.6, LR 4.6 |
| per-activation authority | minted by a kit adapter from a capability for one unit of work | LR 11.6 |
| trusted | bodiless standard intrinsic; claims reported as trusted, never proved | LR 11.3 |

## Concurrency and observation

| Term | Definition | Source |
|---|---|---|
| scope | `scope name { … }`: owns tasks; joins or cancels them before exit | LR 12.5 |
| task | `Task[T]` from a launch; linear | LR 12.5 |
| cancellation point | `await`; on the pools also `checkpoint()` | LR 12.4 |
| observation schema | `observation` declaration of typed frame fields | LR 13 |
| observation frame | one activation's captured fields | LR 13 |
| event | one `record` execution | LR 13 |
| task reporter | diagnostic-plane sink for reports, logs and frames | LR 15 |
| log record | one `log.info`/`log.warn`/`log.error` execution: level, message, function; standard error, never standard output | LR 13 |
| diagnostic plane | output channel nothing can read back | LR 11.7 |
| record (tool output) | schema-valid object; text on a terminal, JSON line otherwise | [records](records.md) |
