# Keyword inventory (LR A)

<!-- covers: LR A -->
*Agent mode. Human mode: [Keywords](../guide/appendix-a-keywords.md). Source of truth: LR A, LR 3.4.*

## Rules

| Rule | Statement | LR |
|---|---|---|
| LANG-03-04 | Every word below is reserved in every declaration and binding position (variable, parameter, field, type, declaration name), except the name of a method or associated function; after `.` every word is a member name (`memberName`). The lexer never reclassifies a keyword. | LR 3.4, LR A |

## Inventory

| Group | Words | LR |
|---|---|---|
| Declarations | `func` `fn` `let` `mut` `const` `type` `alias` `class` `trivial` `enum` `error` `interface` `mixin` `implement` `implements` `with` `derive` `requires` `forbids` `excludes` `contract` `aspect` `needs` `before` `after` `on` `metadata` `lifetime` `from` `where` `export` `import` `as` | LR A |
| Modifiers | `pure` `noalloc` `async` `try` `recovers` `editable` `owned` `dynamic` `linear` `trusted` `once` | LR A |
| Ownership | `copy` `move` | LR A |
| Control | `if` `else` `while` `for` `in` `loop` `break` `continue` `return` `fail` `give` `recover` `match` `when` `otherwise` `is` `or` `catch` `discard` `defer` `await` `scope` | LR A |
| Values | `true` `false` `null` `present` `success` `failure` `self` `Self` | LR A |
| Observation | `capture` `record` `report` `observation` `required` `version` `encoding` | LR A |
| Testing | `test` `cases` `property` `forall` `fuzz` `bench` `expect` | LR A |
| Kit heads (reserved by the core) | `application` `context` `model` `component` `view` `explore`; prefixes `server` `client` `hybrid`; in head bodies `inject` `provide` `state` `action` `view` `resource` | LR A, LR 16 |
| Contextual, not reserved | `launch` `launchParallel` `launchBlocking` `detach` `retain` `validate` `checkpoint` `join` `annotate` `cleanup` `capability` `lazy` | LR A |

## Diagnostics

| Code | Trigger | Correction | LR |
|---|---|---|---|
| E-SYN-002 | a keyword in a declaration or binding position (`let match`, a field `from`, a parameter `type`) | rename (`pattern`, `start`, `kind`); a method may keep the keyword name | LR 3.4 |

Tool behaviour: the checker stops parsing a module at its first `E-SYN-002`, so fix each one and re-check; the
rule itself is LR 3.4.

## Correction patterns

### E-SYN-002: keyword as a binding (LR 3.4)

```fortis
// reject: E-SYN-002
pure func span(start: Int, end: Int) Int {
  let match = end            // reject: E-SYN-002
  return match
}

test "span" {
  expect span(1, 4) == 4
}
```

```fortis
// accept
pure func span(start: Int, end: Int) Int {
  let last = end
  return last
}

test "span" {
  expect span(1, 4) == 4
}
```

### E-SYN-002: keyword as a field; keep it as a method (LR 3.4, LR A)

```fortis
// reject: E-SYN-002
class Span {
  export from: Int           // reject: E-SYN-002
}

test "span" {
  expect Span(from: 1).from == 1
}
```

```fortis
// accept
class Span {
  export start: Int
  export pure func from(start: Int) Span => Span(start: start)
  export pure func where(self, n: Int) Int => n
}

test "span" {
  let s = Span.from(1)
  expect s.start == 1
  expect s.where(4) == 4
}
```
