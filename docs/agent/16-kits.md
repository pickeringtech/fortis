# Reserved declaration heads for kits (LR 16)

<!-- covers: LR 16 -->
*Agent mode. Human mode: [Kits and declaration heads](../guide/16-kits.md). Source of truth: LR 16.*

> Preview: declaration heads and kits (LR 16) arrive in v0.5.

LR 16 has no rule table; the rule that binds it is LANG-03-04 (LR 3.4): every reserved word, head words
included, is rejected in declaration and binding positions and admitted as a member or method name.

## Facts

| Fact | LR |
|---|---|
| A kit is a named, versioned bundle of packages shipped with the distribution; it extends Fortis only through libraries, aspects, declaration heads, embedding bindings and verifiers. Toolchain components (daemon, LSP, SDK generators) are not kits. | PROG-PRD-38 |
| A kit never changes core semantics and is never a prerequisite of Language 1.0. | PROG-FND-07 |
| Kits extend the language with aspects (LR 11.8) and, for non-function-shaped declarations, declaration heads. | LR 16 |
| A head's body is validated by a versioned schema and elaborated into core declarations before type checking. | LR 16 |
| Elaboration contract: a head produces only classes, functions, enums, contracts and observation schemas that obey every LR rule. | LR 16 |
| Elaborated code is checked like any other code; heads have no ownership or effect rules of their own. | LR 16 |
| Packages cannot add heads, tokens or grammar rules. | LR 16 |
| Head words are reserved words, never imported; importing a kit's package makes its heads available; its aspects are imported by name. | LR 16 |
| Raw template region: a statement `[export] [server\|client\|hybrid] view [name [(params)]] {` opens a region; the lexer takes the text to the balancing `}` (counting all braces, including `${…}`) as one unlexed token handed to the Web UI kit. | LR 16 |

## Reserved heads

| Head | Kit | Elaborates to | LR |
|---|---|---|---|
| `application` | Runtime host | generated `main`, capability acquisition, lifecycle ordering | LR 16 |
| `explore` | Developer tooling | an exploration declaration | LR 16 |
| `context` (body word `provide`), class member `inject` | Dependency injection | constructor parameters, generated scope struct | LR 16 |
| `model` | Data | nominal data class, schema, codecs, query metadata | LR 16 |
| `view` (standalone), `component` (body words `state`, `action`, `view`, `resource`) | Web UI | render function; class with explicit state struct, action functions, render function | LR 16 |
| `observation` | core (LR 13), consumed by Telemetry | frame type and schema | LR 16 |

Reserved also: prefixes `server`, `client`, `hybrid`; body words `inject`, `provide`, `state`, `action`,
`resource` (LR A). Not reserved (contextual): `lazy`, `launch`, `launchParallel`, `launchBlocking`, `detach`,
`retain`, `validate`, `checkpoint`, `join`, `annotate`, `cleanup`, `capability` (LR A).

## Extension slots (exactly four; LR 16)

1. A head block, optionally prefixed `server`, `client` or `hybrid`.
2. `[export] inject [lazy] name: Type` inside a class body; an exported slot is an exported field (X-138).
3. Aspects declared by the kit, applied with `@name(args)` (no slot of their own).
4. The `cases` table syntax of tests (LR 14).

## Diagnostics

| Code | Trigger | Correction | LR |
|---|---|---|---|
| E-SYN-002 | a head word, prefix or head-body word used as a binding, parameter, field, type or free-function name | rename (`page`, `pageView`); a method or associated function may keep the keyword name | LR 3.4, LR 16 |

## Correction patterns

### E-SYN-002: a head word as a field name (LR 3.4, LR 16)

```fortis
// reject: E-SYN-002
export class Page {
  export view: Int            // reject: E-SYN-002
}

test "page" {
  expect true
}
```

```fortis
// accept
export class Page {
  export views: Int
  export func view(self) Int => self.views
}

test "page" {
  let p = Page(views: 3)
  expect p.view() == 3
}
```

### E-SYN-002: a head word as a binding (LR 3.4, LR 16)

```fortis
// reject: E-SYN-002
pure func size() Int {
  let model = 1               // reject: E-SYN-002
  return model
}

test "size" {
  expect size() == 1
}
```

```fortis
// accept
pure func size() Int {
  let dataModel = 1
  return dataModel
}

test "size" {
  expect size() == 1
}
```

### E-SYN-002: a head word as a free function name (LR 3.4, LR 16)

```fortis
// reject: E-SYN-002
pure func component(n: Int) Int => n      // reject: E-SYN-002

test "component" {
  expect component(1) == 1
}
```

```fortis
// accept
export class Widget {
  export id: Int
  export func component(id: Int) Widget => Widget(id: id)
}

test "component" {
  expect Widget.component(1).id == 1
}
```

## Pattern: a user aspect (no slot needed; LR 16, LR 11.8)

```fortis
// accept
import fortis.host { Console, Host }

aspect logged(label: StringView) {
  needs console: Console
  before {
    console.print("enter ${label}")
  }
}

@logged("work")
func work(console: Console, n: Int) Int => n

export func main(args: List[String]) Void {
  let caps = Host.capabilities()
  caps.console.print("result ${work(caps.console, 3)}")
}
```

```output
enter work
result 3
```
