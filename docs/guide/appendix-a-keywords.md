# Appendix A: Keywords

<!-- covers: LR A -->
*Human mode. Agent mode: [Keywords, by rule](../agent/appendix-a-keywords.md). Source of truth: LR A.*

Fortis reserves its keywords: a keyword can never be the name of a variable, a parameter, a field, a type or any
other declaration (LR 3.4). There is one relaxation, and it is what lets the standard library read naturally: a
keyword may be the name of a method or an associated function, and after a `.` it is an ordinary member name
(LR A). This appendix lists the words, says where each is defined, and shows both sides of that rule.

## The reserved words

The words are grouped by what they do; each group is defined in the LR section named beside it (LR A).

| Group | Words | Defined in |
|---|---|---|
| Declarations | `func` `fn` `let` `mut` `const` `type` `alias` `class` `trivial` `enum` `error` `interface` `mixin` `implement` `implements` `with` `derive` `requires` `forbids` `excludes` `contract` `aspect` `needs` `before` `after` `on` `metadata` `lifetime` `from` `where` `export` `import` `as` | LR 4.3, LR 4.4, LR 5.1, LR 6.9, LR 6.10, LR 7.6, LR 7.7, LR 8.1, LR 8.2, LR 9, LR 10.3, LR 11.5, LR 11.8 |
| Modifiers | `pure` `noalloc` `async` `try` `recovers` `editable` `owned` `dynamic` `linear` `trusted` `once` | LR 6.12, LR 7.6, LR 8.1, LR 8.2, LR 9.5, LR 11.2, LR 11.3 |
| Ownership | `copy` `move` | LR 7.5 |
| Control | `if` `else` `while` `for` `in` `loop` `break` `continue` `return` `fail` `give` `recover` `match` `when` `otherwise` `is` `or` `catch` `discard` `defer` `await` `scope` | LR 5.2, LR 5.3, LR 5.4, LR 5.5, LR 9.8, LR 10.2, LR 10.4, LR 10.5, LR 12.1, LR 12.3, LR 12.5 |
| Values | `true` `false` `null` `present` `success` `failure` `self` `Self` | LR 3.5, LR 6.8, LR 9.3, LR 9.5, LR 10.1 |
| Observation | `capture` `record` `report` `observation` `required` `version` `encoding` | LR 10.6, LR 13 |
| Testing | `test` `cases` `property` `forall` `fuzz` `bench` `expect` | LR 14 |
| Kit heads (reserved by the core, defined by kits) | outer heads `application` `context` `model` `component` `view` `explore`; prefixes `server` `client` `hybrid`; words inside head bodies `inject` `provide` `state` `action` `view` `resource` | LR 16 |

Some words are reserved although your own code rarely or never writes them. `lifetime` is reserved, but stored
loans are not in Language 1.0, so no user type declares a lifetime parameter (LR 7.7). `trusted` is accepted only
inside the `fortis` package (LR 11.3). The kit heads are reserved by the core so that no module, in any kit, can
use one as a name, whether or not it imports the kit that defines the head (LR 16).

## Keywords cannot be names

Using a keyword where a name is declared is `E-SYN-002`: a variable, a parameter, a field and a type all count
(LR 3.4). The checker stops at the first such word, so each example below has exactly one.

```fortis
// reject: E-SYN-002
export func main(args: List[String]) Void {
  let match = 3          // reject: E-SYN-002
}
```

A field is a declaration position too, so a field cannot be called `from`, even though a method can (LR 3.4):

```fortis
// reject: E-SYN-002
class Span {
  export from: Int       // reject: E-SYN-002
  export until: Int
}

export func main(args: List[String]) Void {
}
```

The usual repair is a more specific name: `start` for `from`, `pattern` for `match`.

## Keywords as member names

The one relaxation is member position: a method or an associated function may be named with a keyword, and the
word after a `.` is always a member name, never a keyword (LR 3.4, LR A). The lexer never reclassifies a keyword;
the grammar simply accepts a keyword where a member name is expected. This is why the standard library can spell
`String.from(view)`, and why your own class can have a `from` constructor or a `where` method.

```fortis
// accept
import fortis.host { Host }

class Query {
  export limit: Int
  export pure func from(limit: Int) Query => Query(limit: limit)
  export pure func where(self, n: Int) Int => n
}

export func main(args: List[String]) Void {
  let caps = Host.capabilities()
  let q = Query.from(5)
  let text = String.from("view")
  caps.console.print("${q.where(3)} ${q.limit} ${text}")
}
```

```output
3 5 view
```

## Contextual words

A handful of words have a fixed meaning in one place but are not reserved: `launch`, `launchParallel`,
`launchBlocking` and `detach` (methods on a scope), `retain`, `validate`, `checkpoint`, `join`, `annotate`,
`cleanup`, `capability` and `lazy` (LR A). They are ordinary names everywhere else, so a variable may be called
`capability` or `lazy`:

```fortis
// accept
import fortis.host { Host }

export func main(args: List[String]) Void {
  let caps = Host.capabilities()
  let capability = "read"
  let lazy = true
  caps.console.print("${capability} ${lazy}")
}
```

```output
read true
```

Each of these words is a method of a standard type (`launch`, `retain`, `validate`), a function of the prelude
(`checkpoint`, `join`, `annotate`), the name of the special method `cleanup` (LR 9.4), or a word that only means
something inside one clause (`capability` in a contract, `lazy` in an `inject` member; LR 11.5, LR 16).
