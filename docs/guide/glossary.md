# Glossary

<!-- covers: LR E -->
*Human mode. Agent mode: [Glossary, by source](../agent/glossary.md).*

The words this guide uses with a precise meaning, each with the section that defines it. Where a word has two
meanings, both are given. The Language Reference keeps its own short glossary of the terms that name the parts
of Fortis and its distribution (LR E); this one also defines the words used inside the language.

## The pieces of the system

- **Package**: the unit of distribution, versioning and visibility: a directory with `project.yaml`, a lockfile
  and `src/`. Two versions of one package in one build are two different packages (LR 4.1).
- **Module**: one source file under `src/`; its path below `src/`, with `/` written as `.` and without `.ft`, is
  its name. A module holds only declarations and constants, so importing one never runs code (LR 4.2).
- **Kit**: a named, versioned bundle of packages that ships with the distribution, such as the HTTP kit, the
  Data kit or the Web UI kit. A kit extends Fortis only through libraries, aspects, declaration heads, embedding
  bindings and verifiers, and never changes what a core program means.
- **Compatibility set**: the published set of standard packages, `fortis` and the kits, at the versions tested
  together; a dependency on a standard package must name one of them at the set's version (LR 4.1).
- **Toolchain component**: a tool that ships with Fortis and works on programs rather than inside them: the
  daemon, the language server, the SDK generators. Toolchain components are not kits.
- **Backend**: an implementation that runs checked programs. There are three, which must agree: the
  *definitional interpreter* (the reference, run by `fortis run`), the *development VM* (`fortisc vm`) and
  *native code* (safe Rust, built by `fortis build`) (LR 1).
- **Build mode**: how a program is built: `dev` (on the VM), `native-dev` or `release`. Optimisation, debug
  symbols and profile-guided optimisation are settings of a mode.
- **Lint level**: how strictly lints are reported: under the baseline level an unused import is a warning, under
  the strict level an error (LR 4.3).
- **Target feature set**: which standard modules exist for a compilation target (`core`, `alloc`, `host`,
  `async`, `web`, `browser`); distinct from effects and from capabilities (LR 11.6).

## Declarations and syntax

- **Declaration head** (or **head**): a fixed keyword that introduces a declaration whose body is validated by a
  versioned schema and elaborated into ordinary core declarations before type checking (LR 16). The core reserves
  every head and defines `observation` itself (LR 13); `explore` belongs to developer tooling, a toolchain component
  (LR E); the kits define `application`, `context`, `model`, `component` and `view` (LR 16).
- **Elaboration**: rewriting a head, or a function with aspects applied, into ordinary core declarations before
  ownership and effect checking, so that the result is checked like any other code (LR 16, LR 11.8).
- **Extension slot**: one of the four places a kit's syntax can enter a module: a head block, an `inject` member,
  an aspect application, a `cases` table (LR 16).
- **Raw template region**: the text of a `view { … }` in a head body, handed unlexed to the Web UI kit (LR 16).
- **Aspect**: declared behaviour applied around a function with `@name(args)`: `before` and `after` hooks, an
  `on failure` mapping and metadata. An aspect adds; a contract subtracts (LR 11.8).
- **Contract**: a named conjunction of modifiers, forbidden effects and required or excluded authorities,
  written in the modifier position. Applying one never changes behaviour; it only makes the checker stricter
  (LR 11.5).
- **Mixin**: a set of methods a class applies with `with`; it may require methods but never adds state (LR 9.6).
- **Newtype**: a nominal type declared with `type Name = Representation`; it never interchanges with its
  representation or with another newtype over it (LR 6.9).
- **Refinement**: a predicate on a built-in type, written in `<…>`: `Int<0..100>`, `String<scalars: 1..120>`
  (LR 6.3).
- **Binding** (of a name): a name introduced by `let`, `let mut`, a parameter, a pattern or a capture list
  (LR 5.1, LR 7.2, LR 9.8).
- **Binding** (of an embedded language): a versioned package that gives a language fenced blocks in Fortis,
  through the embedding protocol: its meaning is fixed by the compiler and its checking is a verifier
  (being designed, [Embedded languages](embedded-languages.md)).
- **Verifier**: a package that can only make the check stricter: it reads the checked program and returns
  records, never code; it is pure, deterministic, bounded and holds no capability.

## Values and ownership

- **Place**: a path that denotes storage: a binding, a field path rooted in a binding or `self`, or an `Array`
  element with a literal index (LR 7.2).
- **Owner**: the binding that owns a value until the value is moved out of it (LR 7.2).
- **Loan**: a read-only (`alias`) or exclusive (`editable`) reference to a place; loans are lexical (LR 7.3).
- **Marker**: one of `copy`, `alias`, `move`, `editable` written before a place, saying how the place is used
  (LR 7.5).
- **Loan-carrying value**: a value that stores a loan: the standard views (`StringView`, `BytesView`,
  `ListView[T]`) and cursors (LR 7.7).
- **Trivial**: a type whose values are duplicated implicitly and never need a marker, such as the integers,
  `Bool` and `Duration` (LR 6.11).
- **Copyable**: a type that supports `copy`, an independent field-wise duplicate (LR 6.11).
- **Linear**: a type whose values must be consumed exactly once on every path, such as `Outcome[T]`, `Failure`,
  `Future[T]` and `Task[T]` (LR 6.12).
- **Shareable**: a type whose values may cross to a parallel or blocking pool (LR 12.7).

## Failures and effects

- **Outcome**: the result of a `try` function, `Outcome[T]`: a success value or a failure, linear until consumed
  (LR 10.1).
- **Failure**: the single failure type, with a stable code, a typed payload, an origin and a cause chain
  (LR 10.3).
- **Obligation**: the duty to consume a linear value; the checker tracks each one until it is discharged
  (LR 7.1, LR 10.1).
- **Defect**: a condition that cannot be an outcome because the program's assumptions are broken, such as stack
  exhaustion; never caught (LR 10.8).
- **Effect**: one of the eight facts a function's body may have: `alloc`, `io`, `mutate`, `nondet`, `suspend`,
  `block`, `fail`, `recovers` (LR 11.1).
- **Effect summary**: the subset of the eight effects a function has, computed from its body and its callees
  (LR 11.1, LR 11.3).
- **Write footprint**: the `editable` parameters a function writes through; writes through them are not
  `mutate` (LR 11.3).
- **Fresh storage**: storage created in the current activation and not yet stored anywhere reachable from a
  parameter, capture or `self`; mutating it is not `mutate` (LR 11.1).
- **Authority**: a value that grants input or output, implementing the marker interface `Authority` (LR 11.6).
- **Capability**: a process-wide authority (`FileSystem`, `Network`, `Clock`, `Random`, `Environment`, `Process`,
  `Supervisor`, `Console`, `SecretAccess`, `RawSql`), created once by the runtime and supplied to the entry
  `main`, which declares the ones it needs as parameters (LR 4.6, LR 11.6).
- **Per-activation authority**: an authority minted by a kit's adapter from a capability for one unit of work,
  such as `RequestContext` or `Tx` (LR 11.6).
- **Trusted**: a standard-library intrinsic with no Fortis body, whose claims are reported as trusted, never as
  proved (LR 11.3).

## Concurrency and observation

- **Scope**: a block that owns tasks, `scope name { … }`, and joins or cancels them all before it exits
  (LR 12.5).
- **Task**: a launched future, `Task[T]`, linear until awaited or detached (LR 12.5).
- **Cancellation point**: an `await` (and, on the pools, `checkpoint()`): the only place a task observes
  cancellation (LR 12.4).
- **Observation schema**: an `observation` declaration naming the typed facts an activation records (LR 13).
- **Observation frame**: the record of one activation's captured fields, assigned by `capture` (LR 13).
- **Event**: a record emitted by `record`, once per execution (LR 13).
- **Task reporter**: the diagnostic-plane sink for reports, cleanup reports and frames (LR 15).
- **Diagnostic plane**: where reports and observation records go; nothing on it can feed back into the program
  (LR 11.7).
- **Record**: as a tool's output, a schema-valid object (a finding, a test result, a runtime failure) printed as
  text on a terminal and as one JSON line otherwise.
