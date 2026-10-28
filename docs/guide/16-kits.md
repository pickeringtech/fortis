# Kits and declaration heads

<!-- covers: LR 16 -->
*Human mode. Agent mode: [Reserved declaration heads for kits, by rule](../agent/16-kits.md). Source of truth:
LR 16.*

> Preview: declaration heads and kits (LR 16) arrive in v0.5.

The core language has no HTTP server, no database layer, no web components and no dependency injection. Those
come in **kits**: named, versioned bundles of packages that ship with Fortis, such as the HTTP kit, the Data kit
and the Web UI kit (PROG-PRD-38). A kit is a library. It extends Fortis only through ordinary packages, aspects,
declaration heads, embedding bindings and verifiers; it never changes what a core program means
(PROG-PRD-38, PROG-FND-07). This chapter is about the two ways a kit's syntax reaches your module, aspects and
declaration heads, and about what the core reserves for them (LR 16).

## Aspects need no special syntax

Most of what a kit adds is function-shaped: a route, an authorisation check, a request limit, a schema for a
response. Kits express those as aspects, declared in ordinary modules and applied with `@name(args)` (LR 16,
LR 11.8). An aspect is an ordinary declaration, imported by name like any other, so it needs no grammar of its
own; your own packages can declare aspects in exactly the same way:

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

[Effects and contracts](11-effects.md) explains the parts of an aspect and how it is elaborated around the
function it is applied to.

## Declaration heads

Some things a kit declares are not functions: an application with its lifecycle, a data model, a component
with state and actions, a dependency-injection context. For those a kit uses a **declaration head**: a fixed
keyword that introduces a declaration whose body is validated by a versioned schema and *elaborated*, before type
checking, into ordinary core declarations (LR 16). The core reserves every head word in advance, so that no
identifier in any module can ever collide with a head in any kit (LR 16):

| Head | Kit | Elaborates to |
|---|---|---|
| `application` | Runtime host | a generated `main`, capability acquisition and lifecycle ordering |
| `explore` | Developer tooling | an exploration declaration |
| `context` (with `provide` inside its body) and the `inject` class member | Dependency injection | constructor parameters and a generated scope struct |
| `model` | Data | a nominal data class, schema, codecs, query metadata |
| `view` and `component` (with `state`, `action`, `view` and `resource` inside its body) | Web UI | a render function; a class with an explicit state struct, action functions and a render function |
| `observation` | the core itself ([Observation](13-observation.md)); consumed by Telemetry | a frame type and schema |

The table is LR 16's. `observation` is the one head the core defines and uses itself, which is why you have
already met it.

The *elaboration contract* is what keeps kits honest: a head produces classes, functions, enums, contracts and
observation schemas that obey every rule of the Language Reference, and nothing else (LR 16). Because elaboration
happens before ownership and effect checking, a kit's declarations have no ownership or effect rules of their
own; the generated code is checked like code you wrote by hand (LR 16). And packages cannot add heads, tokens or
grammar rules (LR 16): the set of heads is fixed by the core.

### Head words are reserved

Every head word, the prefixes `server`, `client` and `hybrid`, and the words used inside head bodies (`inject`,
`provide`, `state`, `action`, `resource`) are reserved words, even in a module that imports no kit (LR 16,
LR A). Using one as the name of a binding, a parameter, a field or a function is `E-SYN-002`, as for any keyword
(LR 3.4):

```fortis
// reject: E-SYN-002
export class Page {
  export view: Int            // reject: E-SYN-002
}

export func main(args: List[String]) Void {
}
```

The one relaxation is the same as for every keyword: after a `.`, and as the name of a method or an associated
function, a keyword is an ordinary name (LR 3.4). That is how a library can offer `page.view()` or
`Page.model()`:

```fortis
// accept
import fortis.host { Host }

export class Page {
  export title: String
  export func view(self) String => "<h1>${self.title}</h1>"
  export func model() Page => Page(title: "home")
}

export func main(args: List[String]) Void {
  let caps = Host.capabilities()
  let page = Page.model()
  caps.console.print(page.view())
}
```

```output
<h1>home</h1>
```

Head words are never imported: importing a kit's package makes its heads available, while its aspects are
imported by name like any other declaration (LR 16).

## Raw template regions

A head body is a balanced sequence of ordinary Fortis tokens, with one exception: a **raw template region**
(LR 16). A statement made of `view` (optionally after `export` and one of `server`, `client` or `hybrid`), an
optional name and parameter group, and a `{`, opens a region; the lexer takes the text from that `{` to the `}`
that balances it, counting every brace including those of `${…}` interpolations, and hands the whole text to the
Web UI kit unlexed, as one token (LR 16). The region is recognised by its lexical shape alone, with no knowledge
of the declaration around it, and the kit validates where it may appear. Template syntax is therefore the Web UI
kit's to define, and it never adds a token to the core (LR 16).

The embedded-language blocks being designed in SP-23 generalise this: a template would become an `html` block
with typed holes, one mechanism for every embedded language ([Embedded languages](embedded-languages.md),
PROG-PRD-34).

## The four extension slots

A kit's syntax can enter a module in exactly four places, and nowhere else (LR 16):

1. a head block, optionally prefixed by `server`, `client` or `hybrid`;
2. an `[export] inject [lazy] name: Type` member inside a class body, where an exported slot becomes an exported
   field, so a consumer declared in one module can be constructed by a context in another (X-138);
3. aspects, declared by the kit and applied with `@name(args)`; they need no slot of their own, being ordinary
   declarations;
4. the `cases` table syntax of test declarations ([Tests](14-tests.md)).

A reader who knows these four slots knows every place where a kit can change how a module reads.
