# Fortis Platform Specification

**Document:** FSPEC-0.2 / Platform • **Date:** 22 October 2026 • **Status:** normative baseline for the kits. Companion to the Fortis Language Reference and the Fortis Delivery Programme. The overview document (`Fortis_Specification.md`) defines the epistemic conventions that apply here; every code block in this document is illustrative and has not been run through a Fortis compiler.

## Contents

- [1. Overview: kits, packages and the compatibility set](#1-overview-kits-packages-and-the-compatibility-set)
- [2. Runtime host kit](#2-runtime-host-kit)
- [3. Dependency injection kit](#3-dependency-injection-kit)
- [4. HTTP kit](#4-http-kit)
- [5. Data kit](#5-data-kit)
- [6. Telemetry kit](#6-telemetry-kit)
- [7. Web UI kit (a library)](#7-web-ui-kit-a-library)
- [8. Security kit](#8-security-kit)
- [9. Packages, scaffolds and supply chain](#9-packages-scaffolds-and-supply-chain)
- [10. Configuration and deployment](#10-configuration-and-deployment)
- [11. Developer tooling](#11-developer-tooling)
- [12. Standard contracts](#12-standard-contracts)
- [Appendix A. Requirement ID rename table](#appendix-a-requirement-id-rename-table)

---

## 1. Overview: kits, packages and the compatibility set

A **kit** is a named, separately versioned set of packages that an application imports to get one area of platform behaviour: an HTTP server, a database layer, telemetry, a web UI. Kits sit on top of Fortis Language 1.0 and are ordinary libraries: they are written in Fortis (with trusted Rust adapters where the Language Reference permits), they obey every language rule, and nothing in the language core depends on any of them. An application that imports no kit is a plain Fortis program.

### 1.1 What a kit is

Each kit consists of:

- one or more packages whose names begin with `fortis.` and are listed in 1.2 (`fortis.http` with modules such as `server`; the Web UI kit's five packages `fortis.web.ssr`, `fortis.web.components`, `fortis.web.forms`, `fortis.web.spa`, `fortis.web.pwa`, each with a root module); no kit package is named after a core module (`prelude`, `text`, `collections`, `numeric`, `time`, `money`, `host`), and the Language Reference's longest-declared-prefix rule splits every import unambiguously because 1.2 is the only source of package names;
- a kit version, independent of the language version;
- a published feature-set table stating on which targets the kit's packages exist;
- its own gate in the Delivery Programme and its own acceptance application;
- the aspects it declares (such as `get`, `post`, `transactional`) and, for declarations that are not function-shaped, the declarative heads it defines (`model`, `component`, `context`), each with a versioned validator and a documented elaboration into ordinary typed declarations.

A declarative head is not a parser extension. The Language Reference, section Reserved declaration heads for kits, reserves the head words and fixes their outer syntax; the kit defines what the body means and how it elaborates. The core grammar provides exactly four extension slots, and a kit MUST use only these:

1. **Head blocks**: `model`, `component`, `view`, `context`, `application`, `explore`, each optionally prefixed by `server`, `client` or `hybrid`. The body is validated by the owning kit's versioned schema.
2. **The `inject` member slot**: `[export] inject [lazy] name: Type` inside a class body (Dependency injection kit; `export` exports the field the slot becomes, X-138).
3. **Aspects**: declared by the kit with the core `aspect` mechanism (Language Reference, section Effects and contracts) and applied with `@name(args)` above a function head; these are ordinary declarations and need no slot of their own. There are no attributes on enum variants.
4. **Test tables**: the `cases` syntax of test declarations.

A kit MUST NOT introduce tokens, grammar rules or effects that the core does not already know.

Head words and the observation schema words (`observation`, `required`, `version`, `encoding`) are reserved words of the core and are never imported; an `import` lists the types, functions and aspects a kit package exports.

### 1.2 Kits defined by this document

| Kit | Packages (minimum) | Depends on | Heads it defines |
|---|---|---|---|
| Runtime host | `fortis.runtime` (root module with `Application`, `Server`-style lifecycle types; module `executor`); capability types stay in the core module `fortis.host` | core | `application` |
| Dependency injection | `fortis.di` | host | `context`; the `inject` and `provide` declarations |
| HTTP | one package `fortis.http` with a root module and the modules `server`, `client`, `openapi`, `sdk` (imported as `fortis.http.server`, …) | host, di, telemetry | none; aspects `http`, `get`, `post`, `put`, `patch`, `delete` |
| Data | one package `fortis.data` with a root module and the modules `postgres`, `sqlite`, `jobs`, `cache`, `mail` | host, di, telemetry | `model`; aspects `job`, `schedule` |
| Telemetry | one package `fortis.telemetry` with a root module and the modules `otel`, `archive`, `finops` | host | none (the `observation` head is core) |
| Web UI | five separate packages, each with a root module: `fortis.web.ssr`, `fortis.web.components`, `fortis.web.forms`, `fortis.web.spa`, `fortis.web.pwa` | http, di, telemetry | `view` (standalone, `fortis.web.ssr`); `component` (with `state`, `resource`, `action`, `view` inside its body, `fortis.web.components`) |
| Security | one package `fortis.security` with a root module and the modules `auth`, `secrets` | host | none |
| Codecs | one package `fortis.codec` with a root module and the modules `json`, `yaml`, `csv`, `toml`, `msgpack`, each providing its `…Serialised`/`…Deserialised` mixins over the core `Encodable`/`Decodable` tree | core (`alloc` feature set) | none |
| Packages and scaffolds | toolchain (`fortis package`, `fortis scaffold`) | core | none (scaffold manifests are YAML) |
| Configuration and deployment | toolchain and `fortis.config` | host | none |
| Developer tooling | toolchain (VM, daemon, agent protocol, dashboard) | core | `explore` |

The last three rows are mostly toolchain components (the package manager, the scaffold engine, the VM, the daemon, the agent protocol), which are not kits (Programme PROG-PRD-38); they are listed for what a project uses of them: the `fortis package` and `fortis scaffold` commands, `fortis.config` and the `explore` head.

### 1.3 The compatibility set

A **compatibility set** is a published tuple: one language version, one version of each kit, one toolchain version. Kits are tested together only within a published set. An application's `project.yaml` names the set it builds against; the lockfile pins every package digest within it.

A kit MAY release a new version without a new language version. A kit MUST NOT require a change to the language core; a kit that needs one files a language change through the Delivery Programme's evolution process and waits for it.

Kit versions follow semantic versioning. A compatibility set is immutable once published.

### 1.4 How a kit is used

```fortis
// illustrative
import fortis.host { Host }
import fortis.runtime { Application }
import fortis.http.server { Router }
import fortis.data.postgres as pg

try func main() {
  let caps = Host.capabilities()            // see section 2
  let app = Application.build(move caps)
  app.run() or fail                         // a failing main reports once and exits 1
}
```

Importing a kit package has no side effects: the Language Reference's rule that imports resolve declarations only applies unchanged. A kit's runtime work begins only when the application calls into it.

### 1.5 Target feature sets

Target feature sets select which kit packages exist for a target; they never change the meaning of code that compiles. The Language Reference separates three things that were previously called "capability": target feature sets (this section), effects (the closed effect set in the Language Reference) and runtime authority values (section 2.2 below). A reference to a package that does not exist on the selected target is a compile error before any native compilation starts.

| Feature set | Available kits |
|---|---|
| `core` | none (language core only) |
| `alloc` | codecs (`fortis.codec`); enables `fortis.collections` growth APIs |
| `host` | runtime host, di, data, telemetry, security, configuration (`fortis.config`), packages |
| `async` | required by host; enables `fortis.runtime.executor` |
| `web` | http, web UI (server side) |
| `browser` | web UI client subset (`fortis.web.spa`, `fortis.web.pwa` client half) |
| `desktop` | desktop host adapter (scaffold; see section 9.4) |

| ID | Requirement | Acceptance evidence | Old ID |
|---|---|---|---|
| PLAT-01-01 | Kits MUST be separately versioned and tested only within a published compatibility set. | Set manifest validation; mixed-version negative tests. | FND-001 (part), STD-001 |
| PLAT-01-02 | A kit MUST NOT add grammar, tokens or effects beyond those the core reserves. | Extension boundary tests. | PKG-003 |
| PLAT-01-03 | A kit package that is absent on the selected target MUST fail at `check`, before native compilation. | Cross-target negative corpus. | ARC-003 |
| PLAT-01-04 | Importing a kit MUST NOT perform runtime work. | Module initialisation tests. | restates LANG-04-02 for kits |
| PLAT-01-05 | Unused kits MUST NOT be linked into a native executable. | Binary/dependency/startup inspection. | STD-002, NFR-014 |

---

## 2. Runtime host kit

The runtime host kit turns a checked Fortis program into a long-running process: it supplies the native executor adapter (Tokio), I/O readiness, timers and signals, the `application` head and an orderly start and stop. It builds on the **core runtime inventory** that the Language Reference defines and every Fortis program already has without any kit: the cooperative executor interface and its cancellation rule, the task reporter that `report` writes to, the `observation` schema head, the capability types (`FileSystem`, `Network`, `Clock`, `Random`, `Environment`, `Process`, `Supervisor`, `Console`, `SecretAccess`, `RawSql`), `Host.capabilities()`, and `async try func main` as an entry form that the core executor runs. A command-line program that reads files and runs scoped async work therefore needs no kit; an application imports this kit for the production executor adapter, lifecycle and the `application` head.

### 2.1 Executor adapter

`fortis.runtime.executor` implements the executor interface that the Language Reference, section Cleanup, async and concurrency, defines: one cooperative thread per executor for `launch`; `await` as the only cancellation point on the cooperative executor; `launchParallel` and `launchBlocking` as separate pools whose tasks require `Shareable` captures and observe cancellation at `await` and at explicit `checkpoint()` calls. Tokio is the reference native adapter and is wrapped behind this interface; application code never names Tokio.

The executor provides: task scheduling; I/O readiness; timers and deadlines; channels; task supervision; cancellation signalling; shutdown; profiling hooks. Every adapter operation documents cancellation safety and partial effects.

### 2.2 Capabilities are authority values

A **capability** is a value of a nominal type that carries authority to perform a class of I/O. The ten capability types (`FileSystem`, `Network`, `Clock`, `Random`, `Environment`, `Process`, `Supervisor`, `Console`, `SecretAccess`, `RawSql`) are defined by the language core (Language Reference, section Effects and contracts) and re-exported by `fortis.host`; this kit adds the attenuation methods and the adapter that enforces each one. A function with the `io` effect that reads a file MUST receive a `FileSystem` value (or an attenuated derivative) as a parameter; the standard library's I/O functions take their authority explicitly (a capability, or a per-activation authority such as `Db`).

Rules:

- Capabilities are created once per process: by the runtime, which supplies them to the entry `main` as the parameters it declares, or, in an entry `main` that declares none, by one direct call of `Host.capabilities()` (Language Reference 4.6, 11.6). The checker rejects every other use of `Host.capabilities()` (`E-EFF-010`); a backend asked for a second creation stops the run with the runtime-invariant defect (Language Reference 10.8).
- Capabilities are attenuated by methods that return a narrower value: `fs.subtree(path)`, `net.allowHosts(list)`, `clock.frozen(instant)`.
- Capabilities are `Shareable` and may be transferred to tasks; a task that was not given a capability cannot acquire one.
- A capability cannot be constructed from its fields, serialized, captured by an observation, or recovered from a `Failure`.
- The static check is by type: an `io`-effect call type-checks only when the required `Authority` type (a capability, or a per-activation authority minted from one) is supplied. OS-level enforcement (sandboxing) is outside this version; the Delivery Programme lists it as future work.

```fortis
// illustrative
try func loadConfig(fs: FileSystem, path: Path) Config {
  let text = fs.readText(path) or fail
  return Config.parse(text) or fail
}
```

### 2.3 The `application` head and lifecycle

```fortis
// illustrative
application Billing {
  entry: billing.main
  capabilities: [FileSystem, Network, Clock]
  shutdownDeadline: 30s
}
```

`application` elaborates to a generated `main` wrapper that: parses typed configuration; obtains capabilities; builds the provider graph (if `fortis.di` is imported); opens resources; registers routes and workers; signals readiness. Startup failure disposes what was constructed, in reverse order, and reports once at the entry boundary. The process exits with code 1 and one report, as the Language Reference specifies for a failing `main`.

Shutdown, on signal or on request: stop intake; cancel and join tasks under the deadline; finalise resources; flush permitted telemetry; exit. Liveness and readiness are distinct endpoints; readiness names the critical dependencies it checks.

| ID | Requirement | Acceptance evidence | Old ID |
|---|---|---|---|
| PLAT-02-01 | The executor MUST implement the Language Reference execution model exactly: cooperative single-thread `launch` with `await`-only cancellation; `Shareable`-gated parallel and blocking pools with cancellation at `await` and `checkpoint()`. | Executor conformance suite shared with the VM. | implements LANG-12-03, LANG-12-05 in the adapter |
| PLAT-02-02 | Capabilities MUST be unforgeable nominal values obtainable only as the entry `main`'s parameters, or from one direct call of `Host.capabilities()` in an entry `main` that takes none. | Construction/serialization/capture negative tests. | new |
| PLAT-02-03 | Every I/O function in `fortis` MUST take its authority (a capability or a per-activation authority) explicitly. | API inventory lint. | new |
| PLAT-02-04 | Startup and shutdown MUST follow the typed order with accountable cleanup and one report on failure. | Partial startup/drain/deadline tests. | APP-001 |
| PLAT-02-05 | Every executor adapter MUST document cancellation safety and partial effects. | Adapter metadata and fault tests. | ASY-005 |
| PLAT-02-06 | Detached work MUST hold a `Supervisor` capability and a valid owned context. | Request escape/shutdown tests. | ASY-004 |

---

## 3. Dependency injection kit

The dependency injection kit lets an application declare what a class needs (`inject`), what supplies it (`provide`) and in which environment (`context`), and then generates the constructors and scope classes that wire everything together. An application imports it to replace hand-written wiring with declarations the compiler checks. It is a library with a code generator, not a runtime registry. This section is the kit's first version (DI schema v1), revised to the subset SP-08 established (Delivery Programme 12; SP-08's decision record).

### 3.1 Elaboration

`inject`, `provide` and `context` are declarative heads defined by this kit. They elaborate to ordinary code **before** ownership checking, so the Language Reference has no DI-specific borrow or effect rules, and the kit never changes a type: a slot is filled with the most direct ordinary expression, which the core checks like any other, and a core diagnostic inside generated code is reported at the declaration it was generated from (with the generated line as `generated_line`).

- every `inject` slot becomes a field of its class where it is written (only the word `inject` leaves the line), so it is a parameter of the class's generated constructor; `export inject` makes it an exported field (X-138), which a context in another module needs, since a generated constructor is callable only where every field is visible (Language Reference 9.2);
- every `context` becomes a class `C` with the associated function `application(inputs)` and one class per scope level it uses (`CApplication`, `CRequest`, …) holding the instances that live at that level;
- every `provide` becomes a factory: a field of its level's class for an instance held at a level, or a method on the outermost level class from which it is resolvable for a transient binding.

A consumer can therefore be constructed manually in tests by calling its generated constructor with explicit arguments.

```fortis
// illustrative
class CheckoutService {
  export inject gateway: Shared[dynamic PaymentGateway]
  export inject clock: Clock
}

context Production(clock: owned Clock) {
  provide Clock using instance move clock
  provide Shared[dynamic PaymentGateway] using StripeGateway { lifetime: application }
  provide Shared[dynamic PaymentGateway] using AdyenGateway
    to [CheckoutService.gateway, PreviewService.gateway]
  provide CheckoutService
}
```

A slot's type is an ordinary field type and says how the slot is filled: an owned value is built fresh by a transient binding or copied from a held one (the core rejects a copy of a value that is not `Copyable`); a `Shared[…]` or `Weak[…]` slot takes `retain()` of a held instance; a factory's plain parameter takes a loan for the call. Since no field stores a loan (Language Reference 7.7, X-137), instances shared between consumers are shared through `Shared`, visible in the consumer's own source, and an interface is injected as `dynamic I` (one boxed value per consumer) or `Shared[dynamic I]` (one instance).

A context's body is one statement per line, in any order, a line that starts with `to`, `replaces` or `{` continuing the `provide` above it; a group, a level's inputs or a context declared twice, or a context whose generated classes the module already declares, is `E-DI-010`. A class or factory a binding names is one the context's module declares or imports by name (`import pkg.module { Name }`).

| Statement | Meaning |
|---|---|
| `parent Name` | the context extends another of its module: its bindings, groups and inputs, the parent's first |
| `session(params)`, `request(params)`, `task(params)`, `transaction(params)` | the inputs of that level's scope; the head's parameters are the application level's |
| `group Name = [Target, …]` | a named set of consumers and slots |
| `provide Key [using Source] [to Target \| to [Target, …]] [replaces] [{ lifetime: level }]` | a binding; `Source` is a class (the key itself when omitted), `instance <expression>` or `factory <function>` |

Binding forms have distinct typed meanings: `using Concrete` constructs through the class's generated constructor with every `inject` slot filled (an ordinary field needs a default, `E-DI-009`); `using instance value` evaluates an existing owned value in its level's constructor, where that level's inputs are in scope; `using factory Factory.create` calls a function whose parameters are filled like slots and which may be `try` and `async`: its effects and fallibility appear on every generated function that calls it (`try` measured by SP-08; `async` factories pending DEL-40s).

| Source | no `lifetime:` | `lifetime: transient` | `lifetime: L` |
|---|---|---|---|
| class or `factory` | built per use | built per use | held at `L` |
| `instance` | held at `application` | `E-DI-010` | held at `L` |

### 3.2 Scopes

Scopes are `application`, `session`, `request`, `task`, `transaction`, `transient`. They form a partial order by containment: `application` contains `session` and `request`; `request` contains `task` and `transaction`; `session` and `request` MUST NOT contain each other; `transient` is contained by everything. A provider at a longer-lived scope MUST NOT hold a value from a shorter-lived scope or from an incomparable one (`E-DI-005`). A contained level is created by a method of its container (`app.request(inputs)`) and holds its own handle of each enclosing instance it uses (a `retain()` of a `Shared`, a copy of a `Copyable` value), so a level class stores no loan; an enclosing instance that is neither is bound as a `Shared` to be used below its level (the core rejects the copy otherwise, at that instance's binding). Instances a scope holds are destroyed in reverse dependency order when the scope ends; an instance a consumer holds through its own `Shared` handle lives as long as that handle, which its declared type says.

### 3.3 Provider selection

Selection is static and deterministic: nearest context first; within a context, exact slot, then exact consumer, then explicit group, then untargeted binding; then the parent context. Equal specificity is an error (`E-DI-002`, as is a binding declared twice); no binding is `E-DI-001`. Registration, declaration, module and import order are irrelevant: the generated code and the plan are the same for every order. A child binding that overlaps a more specific parent binding MUST declare `replaces` (`E-DI-003`, corrected by adding it); `replaces` that overlaps nothing is `E-DI-004`. The resolved graph, with candidates and the reason each was chosen or rejected, is part of the build manifest (3.5).

Activated contexts are immutable. Changing configuration creates a new context value used by scopes created afterwards; existing instances keep their dependencies.

Runtime-dependent provider choice MUST be a closed enum declared in the manifest (`select PaymentGateway by config.payment.provider in [Stripe, Adyen]`); the chosen variant is recorded on the scope instance. SP-08 did not run it (pending DEL-40s): its elaboration is a closed enum and a `match` in the generated factory, which raises no ownership question.

### 3.4 Construction

Eager construction happens at scope creation in dependency order; an eager cycle, or a cycle through transient bindings, is a compile error (`E-DI-006`). A construction that fails (`or fail` on a `try` factory) drops what was already constructed, in reverse order, and reaches the caller once (Platform 2.3).

**Lazy edges are not in DI schema v1** (`inject lazy`, `E-DI-011`). SP-08 found no representation that meets this section's lazy rule (one instance per scope, bounded cycle detection, nothing registered after a failed construction, retry as a new resolution) without sharing or locking the source does not write: a provider that borrows its scope stores a loan, which Language 1.0 never allows; one that owns what it needs constructs on every resolve; caching the first construction needs a lock-protected shared cell whose cost the slot would not show. A later version may add lazy edges with a provider type whose sharing and locking are written at the slot.

### 3.5 Provenance

Provenance is a `host` capability feature, not part of the type checker. **Static provenance** is the plan (`fortis.di.plan/v1`, `fortisc elaborate --plan`): for every context, level and slot, the consumer, the slot, the chosen binding, its package and version, the context, the scope, the specificity and the selection reason, and every rejected candidate with the reason it lost; it has no line numbers, so it is the same for every declaration order, and nothing of it is in the generated code. **Live provenance** is recorded only when the `fortis.di.provenance` package is imported and the `Supervisor` capability enables it (the `application` function then takes it as its last input): the generated code records one event per creation (of a held instance or a transient value), per injection of a held instance and per release of a scope's handle (its `cleanup`: a `Shared` instance another holder retains lives on), carrying the event kind, the binding's index in the plan's order and the level, trivial values only, so no instance's field value is ever recorded and nothing needs redaction. A `record` of trivial values is allocation-free natively, so a `noalloc` caller keeps its claim with live provenance on. Correlating the events of one scope instance (instance IDs), the disposal of transient values and of instances a failed construction drops, and the authenticated, redacted inspection below are not in DI schema v1's first measurement and are owned by DEL-40s. Inspection is authenticated, read-only and redacted, and in production is opt-in. History reads the recorded build and configuration, never the current source.

| Code | Rule | Meaning |
|---|---|---|
| E-DI-001 | PLAT-03-02 | no binding provides a slot's key |
| E-DI-002 | PLAT-03-02 | two bindings of equal specificity for one slot, or one binding declared twice |
| E-DI-003 | PLAT-03-02 | a child binding overlaps a more specific parent binding without `replaces` |
| E-DI-004 | PLAT-03-02 | `replaces` overlaps no parent binding |
| E-DI-005 | PLAT-03-04 | an instance depends on one held at a level that is neither its own nor a container of it |
| E-DI-006 | PLAT-03-04 | a construction cycle |
| E-DI-008 | PLAT-03-04 | a `lifetime:` that is not a scope level, or a `parent` that is not a context of the module, or a parent cycle |
| E-DI-009 | PLAT-03-03 | `using Class` of a class the module neither declares nor imports by name, a generic class, or a class with an ordinary field without a default; a `factory` the module neither declares nor imports |
| E-DI-010 | PLAT-03-01 | a statement that is not in DI schema v1, an instance bound transient, or a name declared twice (an input, a group, a context, a generated class) |
| E-DI-011 | PLAT-03-03 | `inject lazy`: lazy edges are not in DI schema v1 (3.4) |

(The numbering skips 007: the design reviewed before SP-08 ran had a loan rule there, a DI-specific borrow rule, and it was removed.)

| ID | Requirement | Acceptance evidence | Old ID |
|---|---|---|---|
| PLAT-03-01 | DI declarations MUST elaborate to constructor parameters and scope classes before ownership checking. | The core's verdict on each elaborated text equals the program's; manual construction in tests (`conformance/di`, `di_check.py`). | new (D-01) |
| PLAT-03-02 | Provider selection MUST be deterministic and ambiguity-free. | Slot/group/default/context matrix; permutation tests with byte-identical generated code and plans. | DI-001 |
| PLAT-03-03 | Factories MUST satisfy type, effect and failure contracts. | Construction-failure and interface tests. | DI-002 |
| PLAT-03-04 | Longer-to-shorter scope edges and eager cycles MUST fail at `check`. | Scope graph negative corpus. | DI-003 |
| PLAT-03-05 | Active contexts MUST be immutable and versioned. | Existing/new-instance reload tests. (pending DEL-40s: not run by SP-08) | DI-004 |
| PLAT-03-06 | Live provenance MUST be opt-in, authenticated and redacted. | Multi-tenant reconstruction; redaction tests. (pending DEL-40s: not run by SP-08) | DI-005, DI-006 |
| PLAT-03-07 | Test overrides MUST NOT leak across parallel tests. | Parallel contamination suite. (pending DEL-40s: not run by SP-08) | DI-007 |

---

---

## 4. HTTP kit

The HTTP kit gives an application a typed HTTP server and client, generated OpenAPI documents and generated native SDKs, all derived from ordinary Fortis declarations. An application imports it to declare endpoints whose request, response and failure shapes are checked at compile time and published as a contract.

### 4.1 Endpoints are functions with aspects

An endpoint is an ordinary exported function carrying one of the kit's aspects: `@get`, `@post`, `@put`, `@patch`, `@delete` (specialisations of `@http` for methods without a shortcut). The aspect carries the route, the authorisation requirement, the status map for the response enum, the failure destination, limits and an optional observation schema; there is no `endpoint` head, no DTO requirement and no attribute on enum variants.

```fortis
// illustrative
import fortis.data { Db }
import fortis.http.server { PublicApi, PublicProblem, get, post }
import fortis.security { Authorisation, Identity, Role }

export enum CustomerAccess implements Authorisation {
  Read
  Create
  export pure func grantedTo(self, who: Identity) Bool => match self {
    Read => who.hasRole(Role.Staff) || who.hasRole(Role.Admin)
    Create => who.hasRole(Role.Admin)
  }
}

export enum GetCustomerResponse {
  Found(customer: Customer)             // the model encodes itself (section 4.5)
  NotFound
  Unavailable(problem: PublicProblem)
}

export enum CreateCustomerResponse {
  Created(id: CustomerId)
  Unavailable(problem: PublicProblem)
}

@get("/customers/{id}",
  auth: CustomerAccess.Read,
  responses: { Found: 200, NotFound: 404, Unavailable: 503 },
  fallback: Unavailable,
  observe: CustomerOps,
)
export PublicApi async try func getCustomer(db: Db, who: Identity, id: CustomerId) GetCustomerResponse {
  let customer = await Customer.find(db, id) or fail
  if customer == null {
    return NotFound
  }
  capture tier = customer.tier
  return Found(customer: move customer)
}

@post("/customers",
  auth: CustomerAccess.Create,
  responses: { Created: 201, Unavailable: 503 },
  fallback: Unavailable,
  limits: { bodyBytes: 65536, timeout: 5s },
)
export PublicApi async try func createCustomer(
  db: editable Db,
  who: Identity,
  name: owned String<scalars: 1..120>,
  tier: CustomerTier?,
) CreateCustomerResponse {
  let created = await Customer.create(editable db,
    name: move name,
    tier: tier ?? CustomerTier.Standard,
    accountId: who.accountId,
  ) or fail
  return Created(id: created.id)
}
```

**Request binding.** A handler parameter is bound from exactly one place, decided in this order: (1) **server context**, if its type implements the marker interface `ServerContext` (Language Reference 9.5: a marker is implemented only by explicit declaration, so `String` never qualifies). The implementations are the capability types and the per-request authorities `RequestContext`, `Identity`, `Db` and `Tx`, which the server adapter mints for each request from the process capabilities; a kit adds one with `implement ServerContext for MyContext {}` in the module that declares the type; a route template or request field naming such a parameter is an error at the aspect (`E-HTTP-004`); (2) the **path**, if the template names it (`{id}`); (3) otherwise the **query string** on `GET`/`DELETE` and the **request body** on `POST`/`PUT`/`PATCH`. A body or query field with no matching parameter is rejected (422), a required parameter the request does not supply is rejected, an optional parameter (`T?`) may be absent, a parameter may be `owned` (the decoder produces the value), and refinements and nominal constructors run before the body executes. A handler that wants a whole structured body takes one parameter of a `Decodable` type instead; a `model` type is never accepted as a request parameter (`E-HTTP-002`), so a request shape is always written out. What this guarantees is exactly that no request is ever assigned into an existing object or a row: the values a request may supply are the handler's parameters and nothing else. It does not decide which of those values the caller is *entitled* to send; whether `accountId` may come from the request or must come from `who.accountId` is the handler's authorisation decision, and writing it as a named argument makes that decision visible, not automatically correct.

The kit declares the aspects (the Language Reference, section Effects and contracts, defines the mechanism):

| Aspect | Properties | Behaviour |
|---|---|---|
| `http[R](method, path, responses, auth?, fallback?, observe?, limits, retry?, cache?, location?, lenient?)`; `needs who: Identity` | `responses: VariantMap[R, HttpStatus]` or entries `{ status, media }`; `auth` is any `Authorisation`; `fallback` is the variant constructor that receives an unhandled failure; `limits`, `retry` and `cache` are nested records with defaults; `lenient` lists parameters that ignore unknown fields | `before`: bind and validate the request into the handler's parameters, enforce limits, check `auth` against `who` (every handler therefore takes `who: Identity`; an anonymous request binds the anonymous identity). `on failure`: with a `fallback`, report once, build a `PublicProblem` from the receipt and return the variant; without one, re-fail |
| `get`, `post`, `put`, `patch`, `delete` | `path` positional, then every `http` property except `method` | `= http(method: …, …rest)`; the canonical spelling, which the formatter produces from `@http(method: GET, …)` |

The aspects `needs PublicApi`, so every handler carries the `PublicApi` contract in its modifier position (section 12).

**Route table and conflicts.** Routes are registered as a compile-time constant so that conflicts are compile errors:

```fortis
// illustrative — src/routes.ft
export const routes = Routes.of(customers.getCustomer, customers.createCustomer, invoices.getInvoice)
```

Two routes conflict when they have the same method and some request path matches both templates. Literal segments must be equal; a `{param}` segment matches any single segment **that parses as the parameter's type**. So that this is decidable, a path parameter's type must come from the closed **path-type domain**: integer types and refinements (decided by interval intersection), `Bool`, enums without payloads (decided by literal sets), `String` with or without a scalar-count refinement (overlaps any literal within the count), and nominal newtypes over those, compared by representation (so `CustomerId` and `OrderId`, both `Int<1..>`, overlap). Any other parameter type in a path is rejected (`E-HTTP-003`). Hence `/customers/{id}` with `id: CustomerId` and `/customers/new` do not conflict, `{a: Int<1..9>}` and `{b: Int<10..20>}` do not, and `{id: String}` with `/customers/new` does (`E-HTTP-001`, naming both functions). There is no precedence rule between literal and parameter segments. Paths are normalised once, by the same rule in the checker and the server: split on `/` first, then percent-decode each segment, then reject a segment that is empty, `.`, `..`, or contains a decoded `/`, then NFC; so `/files/a%2Fb` is a 400, never a one-segment or two-segment match. A template parameter with no handler parameter of that name is an error at the aspect, as is a path-bound parameter the template does not mention. `Server.create(network, listen:, routes:, …)` takes the constant; dynamic registration (`Router.add(...)` at run time) is outside PLAT-04-09 and carries the documented reduced guarantee that conflicts are a startup failure.

### 4.2 Boundary behaviour

- External input is decoded, validated and converted to refined nominal types before the handler runs.
- Wire encodings for 64-bit integers, `Decimal`, timestamps, enums, nullability and bytes are explicit per schema; a client that cannot represent a value receives an error, never a truncated value.
- Body, header, decompression, depth and time quotas and streaming backpressure have safe defaults.
- Identity and authorisation are separate; object and tenant checks run at the action boundary.
- Domain outcomes map to declared variants; operational failures go to one reporter/response adapter. Public problems follow RFC 9457 with safe codes and correlation IDs.
- Pagination uses one encoding: an opaque cursor (`Cursor` nominal type, base64url of a signed, versioned token).
- A timeout after a mutating request is an unknown remote outcome; retry requires an idempotency key.

### 4.3 Closed enums on the wire

A closed source enum cannot hold a value a newer server adds. For every enum that crosses the wire, the codec generator emits a transport enum with an explicit unknown case and a fallible conversion to the domain enum:

```fortis
// illustrative, generated
export enum CustomerTierWire {
  Standard
  Business
  Enterprise
  Unknown(raw: String<scalars: 0..64>)

  export try func toDomain(owned self) CustomerTier {
    return match move self {
      Standard => CustomerTier.Standard
      Business => CustomerTier.Business
      Enterprise => CustomerTier.Enterprise
      Unknown(raw: value) => fail UnknownWireValue(field: "CustomerTier", raw: move value)
    }
  }
}
```

Compatibility is directional. The generator classifies every change as safe-for-requests, safe-for-responses, safe-for-stored-values, or breaking, and the release verifier blocks a breaking change without a version bump.

### 4.4 Schema and SDKs

OpenAPI is generated from the source contract. The pinned version is OpenAPI 3.1.1 (`https://spec.openapis.org/oas/v3.1.1.html`). Constructs that cannot be expressed produce a diagnostic, never an omission.

Swift, Kotlin and TypeScript SDKs preserve nominal wrappers, optionality, response cases, validators, auth hooks, pagination, cancellation and idempotent retry. Each SDK package ships contract fixtures and compatibility metadata. The oldest supported SDK version is a test target of every release.

| ID | Requirement | Acceptance evidence | Old ID |
|---|---|---|---|
| PLAT-04-01 | Endpoints MUST be checked against handler signatures and the `responses` map of their aspect. | Route/schema mismatch fixtures. | API-001 |
| PLAT-04-02 | Framework-generated responses MUST appear in the resolved OpenAPI document. | Actual-versus-OpenAPI contract tests. | API-002 |
| PLAT-04-03 | Wire enums MUST have a generated unknown case and fallible domain conversion. | Old-client/new-server roundtrip. | new (T-08) |
| PLAT-04-04 | Generated SDKs MUST preserve wire semantics and precision. | Cross-language roundtrip tests. | API-003 |
| PLAT-04-05 | API changes MUST undergo directional compatibility analysis. | Breaking/nonbreaking corpus. | API-004 |
| PLAT-04-06 | Boundaries MUST validate, authorise and report under explicit contracts. | Tenant/object/error integration tests. | API-005 |
| PLAT-04-07 | The OpenAPI version MUST be pinned to 3.1.1 by versioned URL. | Generator golden tests. | new (P-01) |

---

### 4.5 Serialisation

Request and response bodies use the structural codec of the Language Reference (section Types, built-in interfaces). A response enum is an **envelope**, not a body: it is never encoded, and it does not implement `Encodable`. What must be encodable, recursively, is every payload field of every variant that produces a body, and every parameter bound from a request must be `Decodable`; a type failing that is rejected at the aspect, not at run time. `Secret[T]` implements neither, so it can appear in no body. The wire rules are fixed by the kit, not chosen per type, so two services never disagree:

| Fortis | JSON |
|---|---|
| nominal `type` newtype | its representation |
| enum without payloads | the variant name as a string |
| enum with payloads | `{ "kind": "Issued", "on": "…" }` |
| `T?` | the value or `null` |
| `Decimal`, `Timestamp` | strings (`"19.99"`, RFC 3339 UTC) |
| `Int64` / `UInt64` | a number within the IEEE-safe range, otherwise a string; the schema states which, so no client truncates silently |
| `Bytes` | base64url |

A response body is derived from the `responses:` map: a variant with one payload field encodes that value as the body; with several, an object keyed by field name; with none, an empty body. `PublicProblem` encodes as RFC 9457 with `application/problem+json`. Media type is JSON unless the map says otherwise (`Found: { status: 200, media: "text/csv" }`); every media encoder consumes the `Encodable` tree, so the payload type must still implement `Encodable`, and the encoder only decides the bytes. Headers are never part of the response enum: standard ones come from aspect properties (`cache: { maxAge: 60s }`, `location: "/customers/{id}"`), and a payload field of type `ResponseHeaders` is removed before the body is derived (so one remaining field is still encoded as a bare value); `ResponseHeaders` holds only `HeaderValue` fields (a bounded ASCII `String` refinement), so the same eligibility walk covers headers and a `Secret[T]` cannot be placed in one. Unknown fields on input are rejected by default; the `http` property `lenient: [name]` lists the parameters that accept and ignore them. Hand-written `encode`/`decode` implementations must also implement `Schema` (`func schema() CodecSchema`), which is what OpenAPI generation reads. Request enums decode through their generated transport type (4.3), so a value from a newer client is a 422 problem, not a crash. Projections from a row to a response use `derive From[Model]` and `to()`; the model itself may be returned directly when the row is the wire shape.

| ID | Requirement | Acceptance evidence | Old ID |
|---|---|---|---|
| PLAT-04-08 | Request binding MUST follow the one-place rule of 4.1: a request can supply only the handler's path-, query- and body-bound parameters, never a server-context parameter and never a field of an existing object or row. A body- or query-bound parameter whose name equals the key, a timestamp or a `belongsTo` foreign key of a model the handler writes MUST raise `W-HTTP-002`, since that is where entitlement decisions are usually missed. | Binding corpus; mass-assignment negative tests; lint fixtures. | new |
| PLAT-04-09 | Route conflicts MUST be detected at `check` over the constant route table using parameter types. | Conflict corpus. | new |
| PLAT-04-10 | Wire encodings MUST follow the table in 4.5; every body payload type, header field and media-encoder input MUST be `Encodable` and every request-bound parameter `Decodable`, checked recursively at the aspect; the response enum itself is an envelope and is never encoded. | Codec golden tests; eligibility negatives. | new |

## 5. Data kit

The data kit gives an application typed access to PostgreSQL and SQLite, migrations, transactions, background jobs, schedules, caches and mail. An application imports it to declare models whose storage, queries and migrations are checked offline against a schema manifest, with every query an explicit, visible effect.

### 5.1 Models and queries

A `model` head declares a table-backed class. It elaborates to an ordinary exported class with the declared fields exported, explicit generated implementations of `Equatable` and `Codec` that cover the columns only (not ordinary `derive`, which would require every field; the generated decoder initialises relations as unloaded and the dirty-set as empty), a typed query builder `CustomerQuery`, finders, relations, scopes and the migration input; `fortis explain Customer` prints the generated class. The body admits the closed vocabulary below plus field lines and ordinary methods (`func`); a model cannot have private fields (the database can see every column), a `cleanup`, or be `trivial`. The generated layout is: the columns as exported fields; one exported `Relation[T]` field per relation (`customer.invoices`), where `Relation[T]` implements neither codec nor `Equatable`, which is why the model's implementations are generated column-only rather than derived; the associated constants `columns` and `relations` holding the typed descriptors for `filter`, `orderBy` and `including`; and a private dirty-set. Returning a model directly from an endpoint therefore exposes exactly its columns, and adding a column is a wire change wherever that happens, which the semantic diff reports.

```fortis
// illustrative
export model Customer {
  table: "customers"
  timestamps                                  // createdAt / updatedAt, maintained by save
  softDelete                                  // deletedAt; queries exclude deleted rows by default

  key id: CustomerId                          // primary key; integer newtypes are generated by the database
  name: String<scalars: 1..120>
  tier: CustomerTier = CustomerTier.Standard
  accountId: AccountId

  belongsTo account: Account via accountId
  hasMany invoices: Invoice via customerId

  scope business = where(tier: CustomerTier.Business)
}
```

The head's vocabulary is closed: `table`, `key`, `timestamps`, `softDelete`, `belongsTo`, `hasMany`, `via`, `scope`. `key` marks the primary key (exactly one); a key whose type is an integer newtype is generated by the database and is not an argument of `create`, any other key is. `timestamps` adds `createdAt: Timestamp` and `updatedAt: Timestamp`, set by `create` and `save` from the connection's clock and not arguments of either; `softDelete` adds `deletedAt: Timestamp?`, set by `delete` and likewise never an argument. The generated class also carries two associated constants, `columns` (one typed descriptor per column: `Customer.columns.createdAt.after(x)`) and `relations` (one descriptor per relation: `Customer.relations.invoices`), so descriptors never collide with field names. Column names map from camelCase to snake_case. The generated API: query-building members are `pure`; every member that executes is generic over its connection, `func find[C](conn: C, …) where C implements Connection` for reads and `func create[C](conn: editable C, …) where C implements Connection` for `create`, `save` and `delete`, where `Db` and `Tx` both implement `Connection` and the parameter is taken by loan, so the linear `Tx` may instantiate `C` (Language Reference 6.12) and stays with the caller for `commit`; the type argument is inferred, so a read inside a transaction is `Customer.find(tx, id)` and a write is `Customer.create(editable tx, …)`, with `tx` still available for `commit` afterwards. The table writes `db` for brevity; `tx` is accepted in every position:

| Member | Meaning |
|---|---|
| `Customer.find(db, id)` → `Customer?` | by primary key |
| `Customer.where(field: value, …)` → `CustomerQuery` | equality predicates by named field; `Customer.columns.createdAt.after(x)` and the other typed descriptors for ranges, membership and null tests via `.filter(…)`; never operators, never strings |
| `.orderBy`, `.limit`, `.offset`, `.including(Customer.relations.invoices)` | chainable; `including` eager-loads a relation with one extra query |
| `.all(db)`, `.first(db)`, `.count(db)`, `.paginate(db, cursor:)` | execution; each is `async try` and returns owned values |
| `Customer.create(editable db, name: …, …)` → `Customer` | insert; takes every column except a generated key and the server-maintained columns (`createdAt`, `updatedAt`, `deletedAt`) as named arguments, and nothing else |
| `customer.save(editable db)` | writes the changed columns (dirty tracking is generated); nothing is written without this call |
| `move customer.delete(editable db)` | soft-deletes when `softDelete` is declared |
| `Customer.business()` | a declared scope, chainable |
| `Customer.withDeleted()`, `Customer.unscoped()` | explicit opt-outs, chainable |
| `customer.invoices` | a `Relation[Invoice]`: `loaded()` gives `List[Invoice]?` (`null` unless the query used `including`); `await customer.invoices.load(db)` returns `List[Invoice]` without caching it, so it takes a read loan |

Property access never runs SQL, there is no global connection and no lazy loading: an N+1 is a `load` written inside a loop, which the kit reports statically (`W-DATA-001`) and the test budget enforces. Rows are validated on the way in with the same refinement and nominal checks as HTTP input, so schema drift surfaces at the query. There is no `$fillable`/`$guarded`: `create` and `save` take declared fields only, and untrusted input reaches them as named arguments the handler writes (4.1). Raw SQL exists only as `Sql.raw(cap: RawSql, text, params)`, an `io` + `fail` operation with no static checking, excluded by the `PublicApi` and `BusinessOperation` contracts.

### 5.2 Type differences

| Fortis type | PostgreSQL | SQLite | Notes |
|---|---|---|---|
| `Int`, `IntN` | `bigint`/`integer`/`smallint` | `INTEGER` | SQLite stores up to 64-bit; `UInt64` above `Int64.max` is rejected for SQLite. |
| `Float` | `double precision` | `REAL` | NaN rejected on write for both. |
| `Decimal` | `numeric(38, s)` | `TEXT` (canonical string) | SQLite has no decimal; comparisons in SQL are rejected, only equality. |
| `Bool` | `boolean` | `INTEGER` 0/1 | |
| `String` | `text` | `TEXT` | UTF-8 enforced on read. |
| `Bytes` | `bytea` | `BLOB` | |
| `Timestamp` | `timestamptz` | `TEXT` RFC 3339 UTC | Ordering by text is correct only in canonical form; the codec guarantees it. |
| `Duration` | `interval` (restricted) | `INTEGER` nanoseconds | |
| nominal `type` | underlying representation | underlying representation | Nominal identity restored by the codec. |
| closed `enum` (no payload) | `text` with check constraint | `TEXT` with check | Unknown stored values surface as `Unknown` via the transport enum. |
| `T?` | nullable column | nullable column | |

Isolation: PostgreSQL defaults to `read committed`, SQLite to `serializable` (its only level). Locking differences (`SELECT ... FOR UPDATE` absent on SQLite) are documented per API; an API unavailable on one backend is a compile error for that target, not a runtime no-op.

### 5.3 Migrations

Published migrations are ordered, checksummed and immutable. The diff tool classifies data loss, lock and backfill implications and old/new binary compatibility. Applying requires a `Process` capability and a migration lock. Expand/migrate/contract is the supported rolling pattern. Backfills are resumable, idempotent, bounded and observed. Rollback never promises to restore deleted data. Production startup checks compatibility and never applies destructive migrations silently.

### 5.4 Transactions

A transaction is a linear handle: the generated `Tx` holds a private field of the core witness type `Must` (Language Reference 6.12), so it is linear by the ordinary rule, and it implements `Abandonable` (Language Reference 12.4), so it may be live across the `await` of the queries it carries and across an `or fail`: on cancellation or failure the unwind calls `abandon`, which marks the transaction abandoned and the pool rolls it back when the connection is returned, so `await Customer.find(tx, id) or fail` inside a transaction is the ordinary spelling and a failure always rolls back. `db.transaction()` returns it and it MUST be consumed by `move tx.commit()` (fallible) or `move tx.rollback()`, which discharge the witness. Falling off a block with a live `Tx` is `E-OBL-001`, so a forgotten commit is a compile error rather than an implicit rollback. Nested transactions use savepoints explicitly. Rollback failure preserves the primary failure and is reported with `report` from the cleanup path; the supervisor labels it as raised during cleanup. Isolation level is a parameter. External effects (payments, mail) inside a database-only transaction are excluded by the `DatabaseOnly` contract (`excludes capability Network`, `excludes capability Process`); outbox and saga helpers are the supported pattern.

### 5.5 Jobs, schedules, cache, mail

A job is a function carrying the `@job(...)` aspect, whose properties name the payload parameters, deadline, retry/backoff, idempotency key, acknowledgement boundary, attempt limit and dead-letter destination; `@schedule(...)` adds the timing rules. Delivery is at-least-once; no adapter is labelled exactly-once. Queues: in-process, PostgreSQL durable, Redis adapter. Schedules pin time-zone data and declare DST, missed-run, overlap and lease rules. Detached work receives a task context, never an HTTP request. Caches have typed keys, tenant scope, TTL/invalidation, and explicit effects; a hit never bypasses authorisation. Feature flags are typed contextual state. Mail has typed templates, test sinks, rate limits and failure/idempotency policy.

| ID | Requirement | Acceptance evidence | Old ID |
|---|---|---|---|
| PLAT-05-01 | Field access and rendering MUST NOT execute queries. | Query-count tests. | DAT-001 |
| PLAT-05-02 | The typed builder MUST be the only statically checked query path; raw SQL MUST require the `RawSql` capability so contracts can exclude it. | Injection/schema/cancellation tests. | DAT-002 |
| PLAT-05-03 | The PostgreSQL/SQLite type table MUST be enforced by codecs and target checks. | Per-type roundtrip on both backends. | new (P-02) |
| PLAT-05-04 | Migrations MUST be checksummed, reviewed and restart/concurrency safe. | Upgrade/crash/destructive-diff corpus. | DAT-003 |
| PLAT-05-05 | Transactions MUST be linear handles; SQL rollback and external effects MUST remain distinct. | Forgotten-commit compile-fail; outbox/ack-loss tests. | DAT-004 |
| PLAT-05-06 | Jobs and schedules MUST define retries, delivery, idempotency and lifecycle. | Duplicate/lease/DST/dead-letter suite. | JOB-001 |
| PLAT-05-07 | Cache and flag state MUST be typed and tenant-scoped. | Cross-tenant/staleness tests. | CAC-001 |

---

## 6. Telemetry kit

The telemetry kit carries the typed observations that source code captures through validation, archive, projection, aggregation and export. An application imports it to get OpenTelemetry-compatible signals, a complete archive of permitted detail, operator-defined projections and cost attribution, all driven by schemas declared in source and policy declared in YAML.

### 6.1 Division of responsibility

The Language Reference, section Observation statements, defines the whole language-level rule: `capture` is a pure, infallible snapshot into a bounded per-activation frame, checked by definite assignment over every exit including failure and `await`; privacy transforms run before any sink. Everything in this section happens after the frame is complete and is a library and pipeline concern.

An observation schema is versioned by a `version: n` field in its body; a schema without one is version 1. A field change that is not backward compatible (a removed field, a changed type, a tightened bound) requires a new version, and stored records carry the version they were captured under. Projections and exporters name a schema as `Name` (latest) or `Name.vN` (pinned). The `@` symbol applies aspects only and is not used for versions.

Two checks are distinct. **Type checking** of source against its declared contracts (required fields, outcome classes, `noalloc` claims) is configuration-independent: policy that affects it is written in source as a contract applied to the function. **Plan compatibility checking** happens at build time: the target plan and the telemetry plan (destinations, sampling, retention, cardinality limits, provider implementations selected in YAML) are checked against the declared claims, and an incompatible plan fails the build with a diagnostic at the plan or policy site. A plan can fail a build; it can never change whether source type-checks. A YAML file MUST NOT reference object fields, choose capture timing, or add a required field.

**Privacy** is declared in the telemetry plan, as `privacy: { permitted_fields: all | none, transforms: { <Schema> | <Schema.vN>: { <field>: redact | hash | drop } } }`. A transform under `Schema` applies to every version of the schema; one under `Schema.vN` applies to that version alone, ahead of the schema's entry. The runtime's **telemetry adapter** applies the policy inside the producing process, before any byte of a captured value leaves it:
- `redact` writes `[redacted]`;
- `hash` writes `h1:<key-id>:` and the first 16 bytes of HMAC-SHA256 under the deployment key, over the field's declared type and its value;
- `drop` omits the field;
- a field the policy does not name is dropped.

A transformed field's truncation to its bound is never told. The adapter writes the **canonical permitted record** to the producer's channel. It carries the producer's **incarnation** (128 random bits for each process start) and its sequence, and the schema version the record was captured under. Without a policy, the task reporter withholds every captured value (Language Reference 15). The toolchain's development plan `permit-all` permits every field unchanged on the record stream.

What each transform reveals is part of the contract:
- `drop`: nothing;
- `redact`: that the field was captured;
- `hash`: equality of values of one declared type under one deployment key (so distinct counts and the order of first occurrence), never the value.

A hashed field is never a metric label.

```fortis
// illustrative
observation OrderProcessing {
  version: 1                          // optional; 1 when omitted
  customerTier: CustomerTier          required: all
  region: Region                      required: all
  provider: PaymentProvider           required: failure
  transactionId: TransactionId?       required: success
  encoding: bounded(512)
}

@observe(OrderProcessing)
async try func processOrder(order: Order) Receipt {
  capture customerTier = order.customer.tier
  capture region = order.customer.address.region
  capture provider = order.payment.provider
  let receipt = await Payments.charge(order) or fail
  capture transactionId = receipt.transactionId
  return receipt
}
```

### 6.2 Completeness per stage

Completeness is defined per stage, because each stage admits different loss:

| Stage | Guarantee | Admitted loss |
|---|---|---|
| Capture | Every applicable required field assigned exactly once (static). | None. |
| Frame completion | Duration and outcome supplied by the runtime; cannot be forged. | None. |
| Aggregation (standard metrics) | Every completed frame counted. | None within process memory bounds; overflow is counted. |
| Persistence (archive/spool) | Per delivery class (6.5). | Declared drop rule. |
| Export | Per delivery class and exporter. | Declared; loss counters emitted. |

"Standard metrics remain complete" means complete at the aggregation stage. Sampling applies to detailed records on a separate branch; sampled counts are labelled as estimates.

### 6.3 Pipeline

capture → validate/redact (in the producing process: the adapter, 6.1) → canonical permitted record → channel → validation and the ledger → optional archive/spool and projections → aggregation → exporters. Archive-before-filter preserves information; it does not make every request write to object storage synchronously. Durable acknowledgement is a separate visible contract.

The archive holds every valid record of every registered schema, whether or not a projection selects it. A record is valid when its schema version is registered, every field it carries is declared there, and every value has its post-transform type (the declared type in full, with its integer range and scalar bound, for a permitted field). A record that is not valid goes to the **ledger** as its identity and a code, never its content. A record with a key written twice at any depth is not valid. The archive keeps the validated record's canonical serialization, never the delivered bytes.

Each consumer keeps the exact set of identities it has applied and commits it atomically with its own state. The consumers are the archive, each projection epoch and the ledger. A delivery is acknowledged only when every consumer has committed it, so after any crash, redelivery is applied by exactly the consumers that lack it. Every directory a consumer writes in, and every ancestor, is durable before anything is written in it. On recovery, every state file the pipeline reads is made durable before anything is acknowledged. At every start, each epoch applies the archived records it lacks, in archive order.

Projections do not depend on the archive. With the archive disabled, full or failing, aggregation continues. A full archive either blocks acknowledgement until there is room, or drops; a drop is final for that identity and is counted.

### 6.4 Projections

```yaml
schema: fortis.project/v1
telemetry:
  privacy:
    permitted_fields: all
    transforms:
      OrderProcessing:
        customerEmail: hash               # joinable within a deployment, never readable
  archive:
    enabled: true
    permitted_fields: all
    spool: filesystem
    format: parquet
    destination: archive.primary
  projections:
    premium_orders:
      source: OrderProcessing.v1          # pinned version; `OrderProcessing` alone means latest
      bind_at: lifecycle_start
      where:
        all:
          - field: region
            in: [UK, IE]
          - field: customerTier
            in: [Business, Enterprise]
      dimensions: [region, customerTier, provider, outcome]
      metrics:
        - name: business.premium_order.duration
          instrument: histogram
          value: duration
          unit: s
      cardinality:
        limit: 1000
        overflow: aggregate
      export: operational.otlp
```

Predicates: typed equality, membership, exclusion, numeric ranges, presence, `all`/`any`/`not`, bounded regex on text fields declared regex-safe, bucketing, explicit sampling. No coercion, no code. A misspelled enum value fails validation against the source schema.

Predicates and dimensions are checked against each field's **post-transform type**:

| Policy action | In the permitted record | Allowed in a plan |
|---|---|---|
| `permit` | the declared type | everything the type allows |
| `hash` | a hash | `present`; a lifecycle key |
| `redact` | `[redacted]` | `present` |
| `drop` | absent | nothing |

A comparison on an absent field is false; `present` tests presence.

Stateful correctness: an up-down projection MUST declare `bind_at: lifecycle_start`. Its membership and dimension keys are evaluated on the lifecycle's start event only, so no field of the end event is ever consulted, and nothing can change between increment and decrement. A lifecycle is one metric's, identified by the producer incarnation and the key field, which must be permitted or hashed. These are counted as anomalies, and a series' balance is never negative:
- an end that arrives before its start;
- a duplicate start or end;
- a key reused;
- a missing key.

Every admitted record takes its tuple's series, in the order the epoch applies records, up to `cardinality.limit`; past the limit, records go to the overflow series. An epoch applies records in archive order wherever the archive holds them, so a live epoch allocates as a replay of the archive would. Dropping a dimension reaggregates. Gauge merging needs an explicit rule.

Plans are versioned. A projection epoch is identified by the projection's definition without `export`. A change to `export` alone is compatible; any other change is a new epoch, which needs a metric name no other epoch has used. A new or reactivated epoch catches up from the archive. Its **coverage** is:
- `complete` when every record ever accepted reached it;
- `archive_partial` after archive drops;
- `since_activation` with no archive to catch up from.

Privacy actions are fixed per schema version for the life of an archive, so a changed policy needs a new schema version. A rejected plan changes nothing. Backfills use isolated destinations or the live destination's deduplicated epochs.

| Code | Rule | Meaning |
|---|---|---|
| E-TEL-001 | PLAT-06-04 | a predicate value not of the field's type (not a variant of its enum, a non-number for `range`), a `range` or histogram `value` on a field that is not numeric, or an unknown `unit` |
| E-TEL-002 | PLAT-06-04 | a schema, version or field the registry does not declare (for an unpinned source, in its latest version) |
| E-TEL-003 | PLAT-06-05 | an up-down projection without `bind_at: lifecycle_start`, or a lifecycle without its key, start or end |
| E-TEL-004 | PLAT-06-01 | a plan outside its own schema (an unknown key, a wrong shape, an option the instrument does not take, an unsupported YAML escape); a transform naming a field or version not declared, or an unknown action; a key that adds a required field, chooses capture timing or names an object field |
| E-TEL-005 | PLAT-06-04 | a use of a transformed field that its post-transform type does not allow, including a redacted or dropped lifecycle key |
| E-TEL-006 | PLAT-06-06 | an incompatible projection change whose epoch reuses another epoch's metric name |
| E-TEL-007 | PLAT-06-08 | a privacy action changed for a schema version the state has already taken records under |

### 6.5 Delivery classes and archive

| Class | Meaning | Admitted loss |
|---|---|---|
| `best_effort` | bounded memory queue | drop-oldest/newest, sampling or bounded block, declared |
| `spooled` | local durability point with retry | loss before spool fsync; loss on spool-media failure |
| `durable` | explicit application operation acknowledging a replicated log, object store or outbox | loss only beyond the acknowledged boundary |

Required sinks: S3-compatible object storage and filesystem. Parquet is the analytic format, JSON Lines for inspection, a versioned binary spool for buffering. Partitioning uses bounded time/service/schema keys. Writes are checksummed with atomic manifest publication; restarts are idempotent. Record identity is the producer incarnation plus its sequence, both fixed by the adapter at capture; redelivery resends the same bytes, and at-least-once delivery is deduplicated by each consumer's applied set (6.3). Replay checks every closed segment's digest and that the archive's records were transformed under the plan's privacy actions, and refuses on a mismatch, naming it, before anything is applied. An exporter outage cannot block application logic. Loss counters use an emergency path that cannot recurse.

### 6.6 OpenTelemetry conventions

The distribution pins the OpenTelemetry API, OTLP protocol and semantic-convention versions in the compatibility set. HTTP duration is in seconds with route templates. Instrument kinds, aggregation and temporality are separate settings. Views select attributes and aggregations with defined overflow. Logging is structured and typed; free text is a debug facility. Trace context crosses supported boundaries without credentials; sensitive baggage is off by default. Sampling a trace does not drop required audit or metric accounting.

### 6.7 FinOps

A first-party analytics adapter joins a FOCUS-compatible cost dataset with telemetry and explicit shared-resource allocation models. Outputs state methodology, billing period, cost type, coverage, confidence and unallocated remainder. Sampled traces are not per-request cost. Reprojection records its model version. High-cardinality attribution is archived, not exported as metric labels.

| ID | Requirement | Acceptance evidence | Old ID |
|---|---|---|---|
| PLAT-06-01 | Type checking MUST be configuration-independent; a telemetry or target plan MAY fail plan compatibility checking at the plan site but MUST NOT change the accept/reject result of type checking. | Same source type-checked under every plan; incompatible-plan diagnostics at the policy site. | OBS-003, CFG-003 (part) |
| PLAT-06-02 | Completeness MUST be defined and tested per stage. | Stage-loss matrix tests (`conformance/telemetry`, `telemetry_check.py`: S1, A1). | TEL-001 (part) |
| PLAT-06-03 | Full permitted detail MUST be preservable before projection filters. | Archive/metric branch comparison (A1). | TEL-001 |
| PLAT-06-04 | Predicates MUST validate against source schema types. | Wrong-enum/unit/presence tests; the plan corpus at its lines (L1). | TEL-002 |
| PLAT-06-05 | Up-down projections MUST bind membership at lifecycle start or be rejected. | Tier-change lifecycle tests at every prefix of several delivery orders (U1). | TEL-003 |
| PLAT-06-06 | Reload and backfill MUST be versioned and prevent double accounting. | Restart/replay/dedup tests: crashes at every disk operation, SIGKILL, reordered and repeated delivery, epochs (D1-D4, E1). | TEL-004 |
| PLAT-06-07 | Export MUST have bounded backpressure and declared durability. | Network/disk-full/partial-upload tests. (pending DEL-55s: the archive's full modes only were run by SP-09) | TEL-005 |
| PLAT-06-08 | Privacy MUST run before every sink and spool. | Secret/PII egress tests: every byte written, on every backend, scanned for canaries; paired inputs (P1-P4). | TEL-006 |
| PLAT-06-09 | Standard telemetry MUST pass the pinned OpenTelemetry conventions. | Golden OTLP tests. (pending DEL-55s) | OBS-004 |
| PLAT-06-10 | Cost outputs MUST disclose data, methodology and unexplained remainder. | Synthetic billing reconciliation. (pending DEL-55s) | FIN-001 |
| PLAT-06-11 | Operators MUST preview and reload compatible plans without changing capture source. | Hot-plan and reprojection tests (compatible reload and catch-up run by SP-09; preview pending DEL-55s). | FIN-002 |
| PLAT-06-12 | `report`, audit events, span status and metric increments MUST stay distinct effects; only `report` consumes a failure. | Valid multi-signal and duplicate-report cases. | OBS-005 |

---

## 7. Web UI kit (a library)

The web UI kit is a set of optional packages for server-side rendering, server-managed components, single-page navigation and progressive web apps. An application imports exactly the packages it wants (`fortis.web.ssr` alone gives templates; adding `fortis.web.components` gives reactive components; `fortis.web.spa` and `fortis.web.pwa` add client navigation and offline support). Nothing in the language core, the runtime host or the HTTP kit depends on these packages; a Fortis API server never links them. The kit is "batteries included" in the sense that it ships in the distribution and works with the rest of the platform out of the box, and "a library" in the sense that it is ordinary importable code with its own version.

It also has its own release train. The web platform changes every few weeks and the language far less often, so the `fortis.web.*` packages, `fortis.web.platform` included (7.8), are released separately from the compiler and the core library. Each release states the Language versions it supports and is tested against each of them, and a release of the web packages never waits for, or forces, a compiler release. The distribution carries the latest compatible release, so scaffolds work out of the box, and a project pins whichever compatible release it wants in its lockfile.

### 7.1 Packages

| Package | Provides | Requires |
|---|---|---|
| `fortis.web.ssr` | templates with contextual escaping, the standalone `view` head, `SafeHtml` | http |
| `fortis.web.components` | the `component` head (with `state`, `resource`, `action` body words); server protocol | ssr, di |
| `fortis.web.forms` | accessible form widgets, validation binding | components |
| `fortis.web.spa` | client navigation, history, prefetch, asset revisions; the client half compiles to WASM under the `browser` feature set | components |
| `fortis.web.pwa` | manifest, service worker, cache policy, local outbox | spa |

### 7.2 Execution boundaries

`server component`, `client component` and `hybrid component` state where a component runs. Server components reach browser APIs only through typed bridges. Client components cannot resolve secret or database providers; the DI elaboration rejects it. Server-only packages never enter a browser bundle. Browser code is untrusted by the server.

### 7.3 Component model

A component declares typed inputs, an explicit `state` struct, actions, resources and a view. Rendering is a pure function of the state struct: version 1 rerenders the whole component from its state after each action; there is no derived dependency tracking. Actions have typed inputs and returns, normal effects and failures, a validated request context and their own authorisation check.

```fortis
// illustrative
import fortis.web.components { SafeHtml }

server component Counter {
  state { count: Int<0..1000000> }

  try action increment(editable self) Void {
    let next = self.state.count + 1 or fail
    self.state.count = Int<0..1000000>.validate(next) or fail
  }

  view {
    <button on:click=${increment}>Count: ${self.state.count}</button>
  }
}
```

A `view` body is a raw template region (Language Reference 16): the core lexer does not tokenise it, so template syntax is defined here and nowhere else. Templates support contextual escaping, typed attributes, conditionals, exhaustive matches, keyed loops and child components. Raw HTML requires `SafeHtml`. Event handlers name actions or explicit client handlers.

Resources have a closed state model (`loading`, `ready`, `failed`, optional `idle`/`stale`). Superseded requests are cancelled or ignored by generation token. Every state is rendered or delegated to a declared boundary.

### 7.4 Server protocol

Initial HTML is server rendered. Action calls carry a signed, versioned component reference, the action ID, validated input and a concurrency revision. All client state is untrusted; signatures do not replace authorisation, validation or fresh reads. Private fields and injected services are never serialised. Revision and idempotency controls reject stale and duplicate actions. Multi-instance deployment requires compatible keys and stores. DOM patching preserves focus, selection, composition and keyed identity. Hot reload pins compatible generations or resynchronises without discarding input.

### 7.5 SPA and PWA

`fortis.web.spa` handles back/forward, scroll, redirects, downloads, prefetch limits and asset revisions. `fortis.web.pwa` generates the manifest, icons and service worker with explicit cache and update policy; capabilities are feature-detected. Service workers intercept requests and serve caches; they never execute server actions locally.

Offline data has identity, TTL, size, privacy and logout/tenant invalidation. Mutations use a typed local outbox with stable idempotency keys and pending/failure/conflict UI. Conflict resolution is a user-supplied function:

```fortis
// illustrative
try func resolveDraft(local: Draft, remote: Draft) Resolved
```

There is no built-in last-write-wins. Reconnect revalidates authentication, permissions and schemas. Local migrations are versioned.

### 7.6 Parity and budgets

VM/native parity for this kit covers pure functions and server components. Client (WASM) bundles have measured startup, size and memory budgets separate from server performance. Browser behaviour is tested with real browsers (section 11.5) and is outside VM parity.

### 7.7 Accessibility defaults

Official widgets provide semantic labels, keyboard and focus behaviour, error association, localisation and RTL, time and currency formatting and reduced motion. Scaffolds target WCAG 2.2 AA; reports separate automated evidence from manual requirements. Uploads have quotas, safe paths and scanning hooks. Server validation is authoritative.

### 7.8 Typed client applications

A single-page or progressive web app is checked as thoroughly as a server, so that a client is correct by construction rather than by testing alone:

- **The client model.** A client component's messages are an enum. Its `update` is a function from its state and a message to a new state and the commands to run, and its view is a pure function of its state (the Elm architecture). Commands carry declared capabilities and effects; an update performs none itself. So every state transition is a pure function that can be property-tested, specified (Programme PROG-PRD-19) and replayed from a message log.
- **Browser APIs.** Fetch, storage, history, notifications and the rest are bound from the web platform's WebIDL. Each operation carries its capability, effects and failures, and other JavaScript is reachable only through a declared binding.
- **Templates.** Views are checked against a typed model of HTML and ARIA: what each element may contain, attribute names and value types, event handler signatures, required accessible names and roles. Contextual escaping is proved for every interpolation.
- **Routes, styles and messages.** Routes have typed parameters, and a link to no route is a compile error. Class names and design tokens are typed identifiers, and message catalogs are typed: a missing translation, placeholder or plural form fails the build.
- **One validation.** A form field's validation is the refined type of the server's input, run on the client for feedback and again on the server for authority (PLAT-07-05). With an `api` contract (Programme PROG-PRD-15), a client that disagrees with its server does not compile.
- **Beyond types.** Invariants and specifications over updates, model-based exploration of message sequences in the VM and in real browsers, and convergence of the offline outbox under reordering, duplication and conflict.

**Keeping the definitions current.** The HTML, ARIA and WebIDL definitions are generated, not written. They live in `fortis.web.platform`, a package of the Web UI kit that ships in the distribution with its own version. The core library never depends on it (PLAT-07-01), and a new web platform release needs no compiler release.

- **Sources.** Each release is generated from pinned snapshots of machine-readable sources:
  - the curated extracts of every web specification (WebIDL, elements, events and CSS, as w3c/webref publishes them);
  - the WAI-ARIA roles and properties;
  - the HTML content models, as the Nu HTML Checker's schemas encode them;
  - browser support (MDN's browser-compat-data and the web-features Baseline data).

  A small reviewed overlay adds what no source states in machine-readable form: escaping contexts, accessible-name requirements, and event target types.
- **Cadence and compatibility.**
  - A scheduled job regenerates the package from new snapshots and runs the whole Web UI corpus and the ecosystem regression run (Programme PROG-PRD-25).
  - It proposes the release with its `fortis diff`, which classes each change by compatibility: an added element or attribute is compatible; an obsoleted one becomes a deprecation warning that carries the specification's status, and is removed only after the deprecation window.
  - A change an agent proposes is approved by a person (Programme PROG-AGT-04).
- **Baselines.** A project declares the browsers it supports, as a Baseline year or an explicit list. Using a feature outside that baseline is an error unless the code detects it first with a typed `supported(...)` check, so a feature that is not yet everywhere is handled by construction.
- **An independent oracle.** The generated types are checked against their sources (every element, attribute and WebIDL member has a type, and every type has a source). Every page of the template corpus, rendered, is validated by the Nu HTML Checker, a validator Fortis did not write.

| ID | Requirement | Acceptance evidence | Old ID |
|---|---|---|---|
| PLAT-07-01 | Web UI packages MUST be optional imports; no core, host or HTTP package MAY depend on them. | Dependency graph lint; API-only binary inspection. | new (P-02, user direction) |
| PLAT-07-02 | Client/server/hybrid boundaries MUST be type and capability checked. | Server-secret/import rejection tests. | WEB-001 |
| PLAT-07-03 | Server components MUST rerender from an explicit state struct; no derived tracking in v1. | Render golden tests. | new (P-02) |
| PLAT-07-04 | Resource races and stale responses MUST have defined handling. | Reordered/cancelled response tests. | WEB-002 |
| PLAT-07-05 | Every action MUST validate and authorise independently of client state. | Tamper/replay/object-ID tests. | WEB-003 |
| PLAT-07-06 | SPA navigation MUST preserve history, focus and keyed state. | Cross-browser journeys. | WEB-004 |
| PLAT-07-07 | Offline sync MUST declare outbox, identity and a user conflict function. | Offline/logout/schema/conflict tests. | WEB-005 |
| PLAT-07-08 | UI defaults MUST support accessibility and localisation and disclose manual gaps. | Automated/manual evidence. | WEB-006 |
| PLAT-07-09 | Browser modules MUST exclude server dependencies; parity is limited to pure functions and server components. | WASM/native/VM subset parity. | WEB-007 |
| PLAT-07-21 | The `fortis.web.*` packages MUST be released on their own release train, separately from the compiler and the core library: each release states the Language versions it supports and passes the kit's suite against each, and the distribution ships the latest compatible release. | Release records with the supported-version matrix and its suite results; a web release made with no compiler release. | new (adoption) |
| PLAT-07-22 | The supported matrix MUST include the browser target for the `browser` feature set (`wasm32-unknown-unknown` with its generated JavaScript glue), beside the WASI target, with async running single-threaded on the browser's event loop; every client bundle of the kit's suite builds, starts and passes in three browsers on it. | Matrix row; the suite's client bundles in three browsers. | new (adoption) |
| PLAT-07-10 | Client components MUST follow the typed message architecture of 7.8: messages are an enum; `update` maps state and message to state and commands; the view is a pure function of the state; commands carry their capabilities and effects, and an update performs none. | Client-model corpus; an effect inside an update rejected; message-log replay equal in the VM and three browsers. | new (adoption) |
| PLAT-07-11 | Browser APIs MUST be reached through bindings generated from the web platform's WebIDL, each operation carrying its capability, effects and failures; other JavaScript only through a declared binding. | WebIDL coverage report; a call without its capability rejected. | new (adoption) |
| PLAT-07-12 | Templates MUST be checked against a typed model of HTML and ARIA (element content, attribute names and value types, event handler signatures, accessible names and roles), with contextual escaping proved for every interpolation. | Template corpus with one rejected case per class; escaping proof fixtures. | new (adoption) |
| PLAT-07-13 | Routes MUST be typed: path and query parameters have refined types, a link names a route and supplies its parameters, and an internal link to no route is a compile error. | Route corpus; broken-link rejection. | new (adoption) |
| PLAT-07-14 | Styles SHOULD be referenced through typed identifiers (class names and design tokens) declared in the package; an undefined style is an error and an unused one is reported. | Style corpus. | new (adoption) |
| PLAT-07-15 | Message catalogs MUST be typed: every message a view uses exists in every shipped locale with matching placeholders and plural forms, or the build fails. | Catalog corpus with a missing translation and a mismatched placeholder rejected. | new (adoption) |
| PLAT-07-16 | A form field's validation MUST come from one declaration: the refined type of the server's input, run on the client for feedback and on the server for authority (PLAT-07-05). | Shared-validator fixtures; tampered-client tests. | new (adoption) |
| PLAT-07-18 | The HTML, ARIA, WebIDL and CSS definitions MUST be generated into `fortis.web.platform`, a separately versioned package of the kit, from pinned snapshots of machine-readable sources plus a reviewed overlay; each regeneration is released with its compatibility-classed diff, and an obsoleted definition is deprecated before it is removed. | Regeneration log with source digests; diff per release; deprecation-window fixtures. | new (adoption) |
| PLAT-07-19 | A project MUST declare its browser baseline; a feature outside it is an error unless guarded by a typed `supported(...)` check. | Baseline fixtures with guarded and unguarded uses. | new (adoption) |
| PLAT-07-20 | The generated definitions MUST be checked against their sources in both directions, and every rendered page of the template corpus MUST pass the Nu HTML Checker. | Source-coverage report; validator log over the corpus. | new (adoption) |
| PLAT-07-17 | Client correctness SHOULD be checked beyond types: invariants and specifications over updates (Programme PROG-PRD-19), model-based exploration of message sequences in the VM and in real browsers, and convergence of the offline outbox under reordering, duplication and conflict. | Invariant, exploration and convergence corpora. | new (adoption) |

---

## 8. Security kit

The security kit supplies authentication adapters, session and CSRF handling, HTTP hardening, and the second exit from a secret: `declassify` with registered sanitisers. `Secret[T]` itself and `reveal` are core (`fortis.host`), because the core `Environment.get` returns `Secret[String]?`; the kit re-exports them. An application imports it to get reviewed defaults instead of writing its own cryptography or secret handling.

### 8.1 `Secret[T]`

`Secret[T]` is a nominal wrapper with no codec, no `capture` eligibility, no `Equatable`, and a `Displayable` that renders only a fingerprint (`Secret(sha256:ab12…)`), so interpolating a secret is legal and shows nothing. Exactly two operations return the underlying value:

- `reveal(secret, cap: SecretAccess) T` — returns the value; requires the `SecretAccess` capability; carries the `io` effect because it writes an audit record.
- `declassify(secret, sanitiser: Sanitiser[T, U]) U` — applies a registered sanitiser (tokenisation, keyed hashing, truncation) and returns its result; carries the `io` effect because it writes an audit record naming the sanitiser. `Sanitiser[T, U]` is itself a non-copyable `Authority` minted by `Sanitiser.register` from `SecretAccess`, which is what satisfies the authority rule for `declassify` and for any function that wraps it.

`Sanitiser[T, U]` is a nominal type whose only constructor is `Sanitiser.register(cap: SecretAccess, name: StringView, sanitiser: pure fn(T) U)`. Registration requires the same capability as `reveal`, so a sanitiser cannot be created by code that could not have revealed the secret; the registered name appears in every audit record.

There is no information-flow analysis in version 1: a `Bool` computed from a secret is a `Bool`. Side channels (timing, length, error messages) are outside the guarantee. A hashed identifier's cardinality is a schema classification attribute on the observation field, not taint.

Configuration stores secret references; debugging prints fingerprints.

### 8.2 Authentication and sessions

Adapters: cookie sessions, OAuth/OIDC, password credentials, WebAuthn/passkeys, via maintained libraries with pinned, reviewed parameters and a migration policy. Authentication is separate from object and tenant authorisation. Coarse authorisation at each HTTP or action boundary is the `auth:` property of the `http` and `action` aspects, checked in their `before` hooks against the request identity; object and tenant checks are explicit calls in the body. The kit checks that the property or the call is present, not that the rule is right.

Cookie sessions rotate, expire and revoke; state-changing cookie-authenticated actions require CSRF tokens. CORS is explicit.

### 8.3 Hardening

HTTP clients verify TLS, limit redirects, bodies and time, and enforce egress policy. Templates escape by context. XML external entities are disabled; decompression is bounded. All parsers have quotas. Inspection and agent endpoints bind locally by default and need explicit configuration in production.

| ID | Requirement | Acceptance evidence | Old ID |
|---|---|---|---|
| PLAT-08-01 | Security adapters MUST be maintained, reviewed and version-pinned. | Inventory/review/security suite. | SEC-001 |
| PLAT-08-02 | The underlying value of a `Secret[T]` MUST leave only through `reveal` with capability or `declassify` with a registered sanitiser; its display MUST be the fingerprint; it MUST NOT be encodable or capturable, directly or nested. | Display/codec/capture compile-fail incl. nested; audit tests. |
| PLAT-08-03 | Identity MUST NOT substitute for object/tenant authorisation. | Cross-tenant/escalation tests. | SEC-003 |
| PLAT-08-04 | Untrusted inputs MUST have bounded processing. | Fuzz/resource-exhaustion suite. | SEC-004 |
| PLAT-08-05 | Debug and agent interfaces MUST be authenticated and capability-restricted. | Exposure/privilege tests. | SEC-005, NFR-024 |

---

## 9. Packages, scaffolds and supply chain

This section defines how Fortis code is distributed and reused: packages, their identity and locking, the registry, and scaffolds that generate project and feature code. A library author publishes a package; an application depends on it by name and version and builds offline from a lockfile.

### 9.1 Package model

A package is the distributable unit named in `project.yaml` (`name: acme.billing`). Its public surface is the set of modules listed under `exports:`; `export` declarations in other modules are visible only within the package. Identity is registry, namespace, name, version and content digest. Two incompatible versions of one package have distinct nominal types.

```yaml
schema: fortis.project/v1
name: acme.billing
version: 1.4.0
language: "1.0"
compatibility_set: "2026.4"
exports: [billing, billing.invoices, billing.money]
entry: billing.main          # applications only
dependencies:
  fortis.http: "^1"
  acme.ledger: "=2.1.3"
```

A package declares what it contains: library modules, contracts, adapters (with trusted Rust), declarative-head schemas, scaffolds, tests, docs, skills. A lockfile pins every transitive package, feature, native crate, compiler compatibility and provenance record. Vendored, offline builds are required.

### 9.2 Registry and build safety

Registry operations authenticate, checksum, and check archive path/symlink/size safety, signatures, provenance, revocation and licences. A signature proves origin, not semantics.

Install-time and build-time execution of **package-supplied** code is not supported in version 1: a package that contains a Cargo build script or any executable generator of its own is rejected at install. This is distinct from **compiler-owned declarative elaboration**: declaration heads, codecs, DI constructors and scaffold templates are expanded by the toolchain from data (schemas, YAML manifests, templates) that packages supply; packages supply data, never code that runs during install or build. Runtime and FFI crates needed by `fortis` ship vendored and prebuilt per tier-1 target. Sandboxed build scripts are a later deliverable listed in the Delivery Programme.

### 9.3 Scaffolds

Scaffolds are project, feature or component blueprints with a YAML manifest declaring typed parameters, dependencies, provided and required interfaces, generated files, conflicts and verification. `plan` returns a semantic diff; `apply` is bound to the workspace hash and atomic; `verify` runs check, contracts, tests and schemas. Repeated application is idempotent or creates a distinct named instance. Provenance records template version, parameters and generated symbols.

Upgrade in version 1 compares the content hash of each generated file with the hash recorded at generation. Unchanged files are replaced; changed files are left untouched and listed in a conflict report with the new version placed alongside (`file.ft.new`). There is no semantic three-way merge.

Official scaffolds: library, CLI, HTTP API, SSR app, SPA/PWA app, worker, scheduler, event consumer, PostgreSQL, SQLite, cache, authentication, API client (Swift/Kotlin/TS), telemetry/archive, desktop host.

### 9.4 Supported combinations

The scaffold option matrix is published machine-readably in three classes: **full** (every combination integration-tested), **representative** (a named subset tested, others compile-checked), **unsupported** (rejected by `plan`). "Every combination tested" is not claimed.

| ID | Requirement | Acceptance evidence | Old ID |
|---|---|---|---|
| PLAT-09-01 | Packages and toolchains MUST be content-locked and build offline. | Clean vendored rebuild. | PKG-001 |
| PLAT-09-02 | Install and build MUST NOT execute package code; build scripts MUST be rejected. | Malicious package tests. | PKG-002, CMP-006 |
| PLAT-09-03 | Package public surface MUST be the `exports:` module list. | Visibility corpus. | new (G-02) |
| PLAT-09-04 | Scaffolds MUST plan/apply/verify atomically with provenance. | Rollback/stale-workspace tests. | SCF-001 |
| PLAT-09-05 | Upgrade MUST replace only unmodified files and report every diverged file. | Hash-diverged corpus. | SCF-002 |
| PLAT-09-06 | The scaffold matrix MUST be published in full/representative/unsupported classes. | Matrix evidence. | SCF-003 |

---

## 10. Configuration and deployment

Project configuration is typed YAML that selects runtime mechanisms, providers, destinations and deployment values. It never changes what source means. This section defines the layering rules, waivers, versioned public schemas and the contents of a deployment bundle.

### 10.1 Configuration

Configuration is an ordinary class, loaded once in `main` through capabilities and passed as a value; there is no global configuration and no ambient `env()`. Nested sections are ordinary classes too, so every field has a type, a default and a name a reviewer can find.

```fortis
// illustrative
export class ListenConfig derive Default {
  export host: String = "127.0.0.1"
  export port: Int<1..65535> = 8080
}
export class DatabaseConfig {
  export url: Secret[Url]
  export poolSize: Int<1..64> = 10
}
export class HttpConfig derive Default {
  export requestTimeout: Duration = 30s
  export corsOrigins: List[Url] = List.of()
}
export class AppConfig {
  export stage: Stage = Stage.Development
  export debug: Bool = false
  export listen: ListenConfig = ListenConfig.default()
  export database: DatabaseConfig
  export http: HttpConfig = HttpConfig.default()
}
```

Inside `main`, which declares the `Environment` and `FileSystem` capabilities and the arguments as its parameters:

```fortis
// illustrative
let config: AppConfig = Config.load(environment, fileSystem, args) or fail
```

**Layers**, lowest to highest precedence: field defaults; `config/base.yaml` then `config/<stage>.yaml`, where `<stage>` is the environment variable `FORTIS_STAGE` (default `development`), read before anything else; a `.env` file in the project root; the process environment; the command-line arguments passed to `load`. Values merge leaf by leaf (a later layer replaces a field, never a whole section); a null in a file means "unset, use the lower layer"; a key appearing twice in one source is an error.

**Keys** map mechanically: `database.poolSize` is `database.pool_size` in a file, `APP__DATABASE__POOL_SIZE` in the environment and `.env`, `--database.pool-size` on the command line. The loader reads only environment variables under the `APP__` prefix (the application may choose another prefix at `load`), so inherited variables such as `PATH` are ignored; a prefixed variable, a file key or a command-line flag that names no field is a load failure naming the key. A required field with no value is a load failure; a value outside its refinement is a load failure with the compiler's refinement diagnostic. The loader keeps per-field provenance (`config.origin(field)`).

**Security rules the loader enforces:**

- A `Secret[T]` field may be sourced from `.env`, the environment or a secret-provider reference (`ref:vault:prod/db`); a config file may hold only a reference for it, never a literal.
- A non-secret string or `Url` value that carries credentials (userinfo in a URL, or a field or variable whose name, split on case changes and underscores and compared case-insensitively, contains the word `password`, `secret`, `token`, `key`, `credential` or `dsn`, so `apiKey` and `API_KEY` both match) is a load failure unless the field is `Secret[T]`. The name check is a backstop, not a classification: credential-bearing configuration must be declared `Secret`.
- A `Secret[T]` field is never read from the command line; a flag naming one is a load failure, because argv is visible to every process on the host.
- `Environment.get(name)` on the capability returns `Secret[String]?`, never a plain `String`, so a variable read outside the loader is a secret until revealed; `Config.load` unwraps a variable only into a non-secret field whose name passes the check above.
- `.env` is read only when the stage is `development` or `test`; in any other stage its presence is a startup error. A world-readable `.env` is refused on POSIX hosts.
- `Secret[T]` is not `Encodable` and not capturable, and its `Displayable` rendering is a fixed fingerprint (`Secret(sha256:ab12…)`) that reveals nothing; the fingerprint, `reveal(secret, caps.secrets)` and `declassify` (8.1) are the only three ways anything derived from a secret reaches a non-secret value.
- Reading the environment needs the `Environment` capability, which only `main` creates, so no library reads variables behind the application's back.
- Defaults fail closed: `debug: false`, loopback listen address, TLS verification on, empty CORS origins.

Aspect arguments are compile-time constants, so a route's `limits:` is a fixed ceiling on the endpoint; values that vary per stage (ports, timeouts, origins, rate limits) are configuration applied server-wide at `Server.create`, never above the endpoint's ceiling.

### 10.2 Public schemas

Versioned public schemas: APIs (OpenAPI), observations, jobs, component state, DI manifest, agent protocol, package manifests, evidence. Internal IR and bytecode are compiler-version-specific. The JSON Schemas are planned deliverables tracked in the Delivery Programme; they validate shape, and the compiler validates semantics.

### 10.3 Deployment bundle

A bundle contains the native binary, assets, configuration schema, OpenAPI document, migration plan, SBOM, provenance and verification evidence, plus health/readiness contracts, graceful shutdown and resource limits. Containers are optional packaging.

| ID | Requirement | Acceptance evidence | Old ID |
|---|---|---|---|
| PLAT-10-01 | Configuration MUST be strict, typed, deterministic and provenance-tracked. | Duplicate/unknown/layer tests. | CFG-001 |
| PLAT-10-02 | Reload MUST be atomic and remain within compiled contracts. | Concurrent/invalid-plan tests. | CFG-002 |
| PLAT-10-03 | Selectors and waivers MUST NOT weaken guarantees; soundness MUST have no waiver. | Rename/expiry/policy tests. | CFG-003 |
| PLAT-10-04 | Persisted and wire schemas MUST have explicit compatibility rules. | Old/new decoder suite. | SCH-001 |
| PLAT-10-05 | Release bundles MUST carry deployment, schema, security and evidence manifests. | Clean deployment and rollback exercise. | DEP-001 |
| PLAT-10-06 | Configuration MUST be loaded through capabilities into a typed class with the layering, key mapping and secret rules of 10.1; a literal secret in a config file or an unknown key MUST fail the load. | Loader corpus; secret-in-file negative tests. | new |

---

## 11. Developer tooling

Developer tooling is the development VM with hot reload, the REPL, replay, the language daemon, the agent protocol, browser testing and the dashboard: toolchain components, not a kit (Programme PROG-PRD-38); what a program sees of them is the `explore` head (1.2). A developer or an agent uses it to edit, check and run a Fortis program with sub-second feedback; production binaries never include it.

### 11.1 Development VM

The VM executes checked IR with source, contract and provenance metadata. It may use tagged values and reference counts internally but never changes source semantics or accepts invalid code. The standard library is implemented once (Rust adapters plus Fortis source); the VM calls it as intrinsics, so VM/native parity applies to the Fortis-source parts. A missing development adapter is an explicit unsupported-capability error.

### 11.2 Reload policy

A reload compiles the edited source into a new **generation**: a checked, validated program whose declarations keep
**identities** across edits. A function is identified by its qualified name. A lambda inherits the identity of the
previous generation's lambda of the same parent with the same structure (environment and body, its own lambdas
included), or with a unique environment. An ambiguous correspondence is a removal and an addition, never a rematch.
Each change between the generations is classified. A **compatible edit** changes only non-generic function bodies
(the exported summary, meaning signature, effects, allocation summary and obligations, and every closure
environment layout unchanged) or adds declarations. Genericity is declared: a body under type parameters is generic
even when no parameter survives in its IR. Every other change is incompatible: a summary, an environment, a
layout, a generic body, a constant, a removal, a method's implementation, or anything the DI elaborator generated.

Generations are pinned per root invocation (a request, a job run, a REPL evaluation, and every task it launches or
detaches): a frame started on generation *n* calls generation *n* code until it completes, so one operation never
mixes two versions. New root invocations use the newest generation. Closures, function values and unstarted
futures carry their function's identity and deep fingerprint (summary, environment, and the layouts of what they
mention), not their generation. A frame resolves them in its own generation, and one that does not resolve is a
version conflict: the invocation ends, runs none of the stale code, and its scope restarts. A root invocation's
result, and a detached task's, is destroyed by the generation that made it.

A compatible reload restarts nothing for its changes: every scope's state is carried as it is (the caps below may
still evict). An incompatible edit restarts, with an explanation, each **affected scope**:

- **dependency**: the changed declaration is in what the scope's factory and handlers reach (calls, dispatch,
  values, lambdas, constants and their initializers, types mentioned), in either generation;
- **state**: the state may hold a value the new generation cannot use (a type it reaches through fields, payloads,
  implementors, captures, future arguments or failures, with type arguments substituted, changed or was removed);
- **entry**: the scope's factory no longer accepts its recorded arguments, so it stops.

Additions restart nothing, but an old invocation that could meet a new value through the state's interior mutability
(a `Mutex`) is cancelled (**old reader**). A restart signals the scope's tasks, drains them without advancing time,
releases the old state on the generation it was valid in, installs the new generation and rebuilds the state there.
A task that cannot finish without time stalls the drain: the scope stops, and its state is kept, pinning its
generation, until the task finishes. The explanation names the rule, the change, and the path from the scope's state or entry point to the
changed declaration.

Old generations are reclaimed when no task and no stalled scope's kept state references them; carried scope
state, code values and completed tasks pin nothing. A **generation cap** (live generations) and a **memory cap**
(the old generations' IR bytes) evict the oldest generation an eviction can free, by restarting the scopes with
invocations on it. A reload is rejected (**bounded admission**) when its drain stalls past a cap, when kept state
pins generations no eviction can free, or when old readers did not drain. Browser DOM state is outside VM parity.

### 11.3 REPL and replay

The REPL has typed cells; redefining a binding uses versioned reload and cannot change a binding's type silently. Replay modes state their fidelity: input-only, controlled-environment, controlled-scheduler, snapshot-inspection. Clock, random, network, filesystem and context adapters are the replayable boundaries; external processes and databases are not rewound. Recordings pin build, configuration, schema and privacy policy.

### 11.4 Agent protocol

Versioned JSON-RPC over stdio or a local socket; authenticated HTTP and an MCP adapter expose the same services. Read operations: workspace snapshot; symbol search and inspect; type/effect/ownership explain; contract origins; DI plan, live and history; test coverage and findings; telemetry plan preview; generated-source mapping. Write/run operations: scaffold plan/apply/upgrade; atomic semantic edit bound to the workspace hash; format and check; selected tests, fuzz, mutation, benchmarks, browser runs; explore/replay/promote; native build; evidence verification. Every operation states capabilities, snapshot, budget and idempotency. Runs return a job ID and structured progress. Permissions separate explanation, code change, command execution, secrets and deployment; skill prose cannot override them.

CLI exit codes: 0 success; 1 user diagnostics or test failure; 2 invalid invocation or configuration shape; 3 infrastructure or dependency unavailable; 4 internal tool defect; 5 cancelled.

### 11.5 Browser testing and dashboard

Browser tests use real Chromium, Firefox and WebKit behind a pinned driver adapter (Playwright initially) with role/label locators, auto-waiting and isolated contexts; failure bundles include screenshots, DOM and accessibility snapshots, console and network logs, backend traces and clock controls. Browser binaries are installed explicitly and cached; nothing downloads silently.

The dashboard shows routes and OpenAPI, component and resource state, DI provenance, query and migration plans, traces, captures and projections, findings and replay, tasks and contract explanations. It is a client of the same authenticated APIs as agents. Development binds locally; production requires explicit access configuration.

| ID | Requirement | Acceptance evidence | Old ID |
|---|---|---|---|
| PLAT-11-01 | The VM MUST share checked semantics with native; the standard library MUST be implemented once. | Differential and negative corpus. | restates PROG-VM-01 for developer tooling |
| PLAT-11-02 | Compatible edits (unchanged summary and layouts) MUST reload without restarting unrelated scopes. | Active frame/future tests: `conformance/reload` isolation, migration and classification (SP-07). | VM-002 |
| PLAT-11-03 | Incompatible edits MUST restart the affected scope with an explanation. | Layout/effect/provider change tests: `conformance/reload` restart and explanation checks (SP-07). | VM-003 |
| PLAT-11-04 | Generations MUST be pinned per root invocation and later reclaimed. | Mixed-generation and soak tests: `conformance/reload` retention and soak checks (SP-07). | VM-004 |
| PLAT-11-05 | Replay MUST disclose uncontrolled boundaries. | Mixed real/mock fixtures. | VM-005 |
| PLAT-11-06 | Human and agent tools MUST share semantic and evidence services. | Cross-surface consistency tests. | AGT-001 |
| PLAT-11-07 | Agent edits MUST be atomic and snapshot-bound. | Concurrent/stale edit tests. | AGT-002 |
| PLAT-11-08 | Browser tests MUST be isolated, safe and evidence-producing. | Session/external-effect tests. | TST-005 |

---

## 12. Standard contracts

Named contracts are conjunctions of exactly these elements, and nothing else: the modifiers `pure`, `noalloc`, `async`, `try`, `recovers`; `forbids <effect>` over the eight core effects; `requires capability X`; and `excludes capability X` (the function may not take a parameter of that capability type and may not call, directly or through callees, a function that requires it; the check is transitive over summaries). A contract subtracts; anything a kit wants to *do* around a function (check authorisation, map failures, attach a schema, enforce limits) is an aspect, and anything it wants to check about a declaration that is not function-shaped is a head validator. Contracts are declared with the `contract` head and applied by name in the modifier position of a function head (`export PublicApi async try func …`). Carrying a contract's name does not satisfy it; the checker verifies every predicate.

```fortis
// accept
contract FastCalculation {
  pure
  noalloc
}

contract PublicApi {
  excludes capability RawSql
  forbids recovers
}

contract DatabaseOnly {
  excludes capability Network
  excludes capability Process
}
```

| Contract | Expansion |
|---|---|
| `PureCalculation` | `pure` |
| `FastCalculation` | `pure noalloc` |
| `PublicApi` | `excludes capability RawSql`, `forbids recovers`; the `http` aspects `needs PublicApi` and themselves supply authorisation, limits, the status map and the `HttpRequest` observation |
| `BusinessOperation` | `excludes capability RawSql`, `forbids recovers`; the observation schema is attached with `@observe(Schema)`, whose required fields include the business outcome and denominator |
| `BackgroundJob` | contract part: `requires capability TaskContext`, `forbids recovers`; the policy (payload schema, deadline, idempotency/retry/ack, dead letter, observation) is the `@job(...)` aspect, which `needs BackgroundJob` |
| `DatabaseOnly` | `excludes capability Network`, `excludes capability Process` |
| `DurableAudit` | not a contract: `@audit(Schema)` is a Telemetry aspect whose `after` hook performs an acknowledged persisted write and whose `on failure` re-fails; it `needs` no contract but is rejected on `pure` targets by the ordinary effect rule |
| `NativeAdapter` | not a contract: the `trusted` modifier (Language Reference, Effects and contracts) plus the package validator for adapter metadata (ownership, layout, allocation, cancellation, VM parity adapter) |
| `BrowserAction` | `forbids recovers`; the `action` aspect of the Web UI kit supplies validation, per-action authorisation, revision/idempotency, safe response and resource invalidation |
| `OfflineMutation` | contract part: `excludes capability Network`; the outbox, identity/schema validation, replay/idempotency and the user conflict function are the `@offline(...)` aspect, which `needs OfflineMutation` |
| `ReusableLibrary` | not a contract: a package validator (explicit `exports:`, conformance suite present, documentation for every exported declaration, target support and compatibility baseline declared) |

`Money` lives in the core module `fortis.money`: `Decimal` paired with a `Currency` value carried at run time (`Money.of(Currency.GBP, 10d)`); arithmetic between different currencies fails with `money.currency_mismatch`, arithmetic through named functions (`Money.add`) returning `Outcome`, and explicit versioned rate conversion. There is no operator overloading.

| ID | Requirement | Acceptance evidence | Old ID |
|---|---|---|---|
| PLAT-12-01 | A named contract MUST expand only to modifiers, `forbids <effect>`, `requires capability` and `excludes capability`; every operational or declaration-level requirement MUST be an aspect or a named validator, and the table in 12 MUST be the single authoritative expansion of each name. | Contract-expansion golden tests; validator inventory. |
| PLAT-12-02 | A contract name MUST NOT satisfy a contract; every predicate MUST be checked. | Mis-labelled implementation negative tests. | App. D |

---

## Appendix A. Requirement ID rename table

| Old ID | New ID | Note |
|---|---|---|
| ARC-003 | PLAT-01-03 | |
| MOD-001 | LANG-04-02 (PLAT-01-04 restates it) | |
| STD-001, STD-002 | PLAT-01-01, PLAT-01-05 | |
| ASY-004, ASY-005 | PLAT-02-06, PLAT-02-05 | ASY-001, ASY-002, ASY-003 are LANG-12-03/05 (PLAT-02-01 implements them) |
| APP-001 | PLAT-02-04 | |
| DI-001 … DI-007 | PLAT-03-02 … PLAT-03-07 | |
| API-001 … API-005 | PLAT-04-01, -02, -04, -05, -06 | |
| DAT-001 … DAT-004, JOB-001, CAC-001 | PLAT-05-01 … PLAT-05-07 | |
| OBS-003, OBS-004, OBS-005 | PLAT-06-01, PLAT-06-09, PLAT-06-12 | OBS-001, -002, -006 moved to the Language Reference |
| TEL-001 … TEL-006, FIN-001, FIN-002 | PLAT-06-02 … PLAT-06-11 | |
| WEB-001 … WEB-007 | PLAT-07-02 … PLAT-07-09 | |
| SEC-001 … SEC-005, NFR-024 | PLAT-08-01 … PLAT-08-05 | |
| PKG-001 … PKG-003, SCF-001 … SCF-003, CMP-006 | PLAT-01-02, PLAT-09-01 … PLAT-09-06 | |
| CFG-001 … CFG-003, SCH-001, DEP-001 | PLAT-10-01 … PLAT-10-05 | |
| VM-002 … VM-004, AGT-001, AGT-002, TST-005 | PLAT-11-02 … PLAT-11-08 | VM-001 is PROG-VM-01; VM-005 split (PROG-VM-05 core part) |
| NFR-014 | PROG-NFR-14 (PLAT-01-05 restates it) | |
| Appendix D | section 12 | `FastCalculation` is now `pure noalloc`; `nostate` removed |
