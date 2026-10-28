# Ownership

<!-- covers: LR 7 -->
*Human mode. Agent mode: [Ownership, by rule](../agent/07-ownership.md). Source of truth: LR 7.*

Every value in Fortis has exactly one owner, and every other use of it is either a loan or a transfer that the
line says out loud. Assigning an existing ownership-bearing value, or passing it where the callee takes ownership,
writes `copy`, `alias` or `move`; changing something through someone else's value writes `editable` (LR 1,
LR 7.5). Passing a value to an ordinary parameter needs no marker, because it is a read loan for the call, and a
trivial value, such as an integer, never needs one (LR 7.5, LR 6.11). There are no hidden clones and no hidden reference counts,
so a reader can answer "who owns this, and can this call change it?" from the line alone. The rules are close to
Rust's borrow checking, made lexical and written into the source; this chapter introduces them one at a time.

## The idea in one paragraph

The checker follows, for every binding, its type, whether it is mutable, whether it still owns its value or has
had it moved out, and which loans of it are active. Every expression is checked against that environment and
records the loans it creates, the effects it has and the obligations it leaves behind; written formally,
`Γ ⊢ e : τ ▷ L, E, O` (LR 7.1). It is a finite dataflow analysis over the function's control flow, with no
solver, so its answers are predictable: each rejection names a place, a conflicting loan or move, and both
positions.

## Places and owners

A **place** is a path to storage: a binding (`invoice`), a field path from a binding or from `self`
(`invoice.lines`, `self.status`), or an `Array` element at a literal index (`grid[3]`). Nothing else is a place:
a call result, `a ?? b`, a literal or `list[i]` with a variable `i` are values (LR 7.2). The ownership markers
`copy`, `alias`, `move` and `editable` apply only to places.

A `let` binding owns the value it was initialised with until the value is moved out, and drops it at the end
of its block if it still owns it. A parameter is a binding that can never be reassigned: an `owned` parameter
owns its argument, a plain or `editable` one holds a loan (LR 7.2).

There are no partial moves: you cannot move one field out of a value and keep the rest (`E-OWN-040`, LR 7.2).
Move the whole value, or take it apart with a consuming pattern ([Classes](09-classes.md)).

```fortis
// reject: E-OWN-040
class Order {
  export id: Int
  export lines: List[String]
}

func keep(lines: owned List[String]) Int => lines.count()

export func main(args: List[String]) Void {
  let order = Order(id: 1, lines: List.of("tea"))
  discard keep(move order.lines)      // reject: E-OWN-040
}
```

## Moves

`move p` transfers ownership out of `p`. Afterwards `p` is uninitialised on that path: reading it, loaning it,
moving it again or dropping it is `E-OWN-020`, use after move (LR 7.4). Where two paths join, a place counts as
initialised only if it is initialised on both. A `let mut` binding can be given a new value after a move.

```fortis
// reject: E-OWN-020
func consume(items: owned List[Int]) Int => items.count()

export func main(args: List[String]) Void {
  let items = List.of(1, 2, 3)
  discard consume(move items)
  discard items.count()              // reject: E-OWN-020
}
```

```fortis
// accept
import fortis.host { Host }

func consume(items: owned List[Int]) Int => items.count()

export func main(args: List[String]) Void {
  let caps = Host.capabilities()
  let mut items = List.of(1, 2, 3)
  let n = consume(move items)
  items = List.of(4)
  caps.console.print("consumed ${n}, now ${items.count()}")
}
```

```output
consumed 3, now 1
```

## Markers and where they are needed

The four markers say how a place is used; they are not operations on values (LR 7.5):

| Marker | Meaning | Allowed when |
|---|---|---|
| `copy p` | an independent duplicate | the type is copyable; refused in `noalloc` code if it allocates |
| `alias p` | a read loan | always |
| `editable p` | an exclusive loan | `p` is rooted in a `let mut`, an `editable` parameter or an exclusive loan |
| `move p` | transfer ownership | `p` owns its value and nothing borrows it |

Where a marker is needed depends on the position. In short (LR 7.5):

| Position | An existing ownership-bearing place needs |
|---|---|
| `let x = p`, `x = p`, a field initialiser `name: p`, `return p` | `copy`, `alias` or `move` |
| argument to a plain parameter, receiver of a `self` method | nothing: an implicit read loan for the call |
| argument or receiver that the result borrows `from` | `alias p` (or `editable p`) |
| receiver of an `editable self` method | `editable p.method(…)` |
| receiver of an `owned self` method | `move p.method(…)` |
| argument to an `editable` parameter | `editable p` |
| argument to an `owned` parameter | `move p` or `copy p` |
| a capture in a closure's capture list | a marker (LR 8.3) |
| operands of `==`, `<` and friends, interpolation, `match p`, `for x in p` | nothing: an implicit read loan |

A fresh value (a literal, a constructor call, a function result, arithmetic) never needs a marker, and neither
does a trivial value such as an `Int` (LR 7.5, LR 6.11). Forgetting a marker where one is required is
`E-OWN-001` (LR 7.5):

```fortis
// reject: E-OWN-001
class User {
  export name: String
}

export func main(args: List[String]) Void {
  let user = User(name: "Ada")
  let other = user                   // reject: E-OWN-001
}
```

The checker cannot guess which of the three you meant, and each means something different, so it asks:

```fortis
// accept
import fortis.host { Host }

class User {
  export name: String
}

export func main(args: List[String]) Void {
  let caps = Host.capabilities()
  let user = User(name: "Ada")
  let duplicate = copy user
  let reader = alias user
  caps.console.print("${duplicate.name} ${reader.name}")
  let taken = move duplicate
  caps.console.print(taken.name)
}
```

```output
Ada Ada
Ada
```

`copy` of a value whose type is not copyable, such as a `Shared` handle, is `E-OWN-002` (LR 7.5).

## Loans are lexical

A loan is a reference to a place, either read-only (`alias`) or exclusive (`editable`). Loans last for a
lexical region, never "until last use" (LR 7.3):

- a loan held by a binding (`let r = alias p`, `let r = editable p`) lasts until the end of the block that
  declares `r`;
- a loan taken for a call's argument or receiver ends when the call returns, unless the result carries it;
- a loan taken by a comparison, interpolation or `capture` ends with the statement, and one taken by a `for`
  header lasts for the whole loop;
- a loan taken for an argument of an `async` call is held by the future the call returns;
- a loan returned with `from` is held by whatever binding receives it.

To end a loan early, close a block around it (LR 7.3).

**The aliasing invariant.** At any point an owner has either one active exclusive loan or any number of read
loans, never both. An exclusive loan of any field freezes the whole owner, while read loans of different fields
coexist. While a loan is active the owner may not be moved, dropped or reassigned, and under a read loan not
mutated. A conflicting loan is `E-OWN-010`; using an owner while it is loaned is `E-OWN-011` (LR 7.3).

```fortis
// reject: E-OWN-010
class Invoice {
  export customer: Int
  export lines: List[String]
  export func issue(editable self) Void {
    self.customer = 0
  }
}

export func main(args: List[String]) Void {
  let mut invoice = Invoice(customer: 7, lines: List.of("tea"))
  let customer = alias invoice.customer
  let lines = alias invoice.lines
  editable invoice.issue()                 // reject: E-OWN-010
}
```

The two read loans of different fields are fine together; the exclusive loan for `issue` is not, while they
are alive. Putting the reads in their own block ends them before the call:

```fortis
// accept
import fortis.host { Host }

class Invoice {
  export customer: Int
  export lines: List[String]
  export func issue(editable self) Void {
    self.customer = 0
  }
}

export func main(args: List[String]) Void {
  let caps = Host.capabilities()
  let mut invoice = Invoice(customer: 7, lines: List.of("tea"))
  {
    let lines = alias invoice.lines
    caps.console.print("${lines.count()} line")
  }
  editable invoice.issue()
  caps.console.print("customer now ${invoice.customer}")
}
```

```output
1 line
customer now 0
```

**Loans through loans.** A binding that holds a loan (a plain or `editable` parameter, or `let r = alias p`) is a
path to the owner, and every loan is recorded against the owner (LR 7.3). Reading through an exclusive parent
creates a read child that *freezes* the parent; taking an exclusive child *suspends* the parent entirely until
the child ends. A loan-holding binding can never be reassigned.

```fortis
// reject: E-OWN-011
class Cell {
  export value: Int
}

func inspect(c: Cell) Int => c.value

func use(c: editable Cell) Void {
  discard inspect(c)           // a read child of the exclusive parent, ended when inspect returns
  let r = editable c           // an exclusive child held by r: c is suspended until the block ends
  r.value = 1
  discard c.value              // reject: E-OWN-011
}

export func main(args: List[String]) Void {
}
```

## Parameters, receivers and returns

A parameter's mode says what the callee may do, and the call site repeats it (LR 7.6):

| Parameter | Caller writes | Callee holds |
|---|---|---|
| `x: T` (ownership-bearing) | the place, unmarked | a read loan for the call |
| `x: editable T` | `editable p` | an exclusive loan for the call |
| `x: owned T` | `move p`, `copy p` or a fresh value | ownership |
| `x: T` (trivial) | the value | a copy |

`editable` gives permission to change the referent, not to rebind the parameter: parameters are never
reassignable (LR 7.6). Methods declare their receiver the same way, `self`, `editable self` or `owned self`,
and the call mirrors it: `invoice.render()`, `editable invoice.issue(now)`, `move invoice.archive()`. A reviewer
sees at every call whether the receiver can change or is consumed (LR 7.6).

```fortis
// accept
import fortis.host { Host }

class Counter {
  export value: Int<0..1000>
  export func bump(editable self) Void {
    if self.value < 1000 {
      self.value = self.value + 1
    }
  }
  export func finish(owned self) Int => self.value
}

func show(c: Counter) Int => c.value

export func main(args: List[String]) Void {
  let caps = Host.capabilities()
  let mut counter = Counter(value: 0)
  editable counter.bump()
  editable counter.bump()
  caps.console.print("shown ${show(counter)}")
  caps.console.print("finished at ${move counter.finish()}")
}
```

```output
shown 2
finished at 2
```

**Returning a loan.** A function that returns a loan names where it comes from with `from`, which names exactly
one plain or `editable` parameter (or `self`) (LR 7.6). At the call, the argument the result borrows from is
marked with `alias` (or `editable` for an exclusive parameter), because that loan now outlives the statement: it
is held by the binding that receives the result.

```fortis
// accept
import fortis.host { Host }

class User {
  export name: String
}

pure func displayName(user: User) StringView from user {
  return alias user.name
}

export func main(args: List[String]) Void {
  let caps = Host.capabilities()
  let user = User(name: "Ada Lovelace")
  let name = displayName(alias user)
  caps.console.print("${name} has ${name.scalarCount()} scalars")
}
```

```output
Ada Lovelace has 12 scalars
```

The returned loan has the mode the signature declares: `StringView from user` is a read loan, and
`editable Cell from c` is an exclusive one, allowed only when `c` is itself `editable`, which suspends the
caller's `c` while the result lives (LR 7.6). A loan of a local or of a temporary cannot be returned, since it
would outlive its owner (`E-OWN-051`), and a result may name only one origin (`E-OWN-050`): return an owned value
or split the function (LR 7.6).

```fortis
// reject: E-OWN-051
class User {
  export name: String
  export pure func guest() User => User(name: "guest")
}

pure func displayName(user: User) StringView from user {
  return alias user.name
}

export func main(args: List[String]) Void {
  let name = displayName(User.guest())        // reject: E-OWN-051
}
```

```fortis
// reject: E-OWN-050
class User {
  export name: String
}

pure func longer(a: User, b: User) StringView from a {
  if a.name.scalarCount() > b.name.scalarCount() {
    return alias a.name
  }
  return alias b.name                         // reject: E-OWN-050
}

export func main(args: List[String]) Void {
}
```

To hand back something you own, return it with `return move local`; to hand back a field of a value you only
borrow, `return copy obj.field` or use a `from` signature; a fresh expression needs no marker (LR 7.6).

## Views, cursors and no stored loans

`StringView`, `BytesView`, `ListView[T]` and the cursors of the standard collections are **loan-carrying
types**: values that store a read loan of their origin (LR 7.7). A loan lives in exactly three places: a local
binding, a parameter, or a result that names its origin with `from`. It is never stored in a field. A class may
not declare a lifetime parameter, and no field may have a loan type or a loan-carrying type other than one
`from static`; both are `E-OWN-053` (LR 7.7, X-137).

```fortis
// reject: E-OWN-053
class Line {                        // reject: E-OWN-053
  export number: Int
  export text: StringView
}

export func main(args: List[String]) Void {
}
```

Store the text itself instead, own it with `move` or `copy`, hold it through an explicit `Shared`, or pass it to
each call that needs it (LR 7.7). A parser that would keep a view of its input keeps offsets into it.

```fortis
// accept
import fortis.host { Host }

class Line {
  export number: Int
  export text: String
}

export func main(args: List[String]) Void {
  let caps = Host.capabilities()
  let line = Line(number: 1, text: "first")
  caps.console.print("${line.number}: ${line.text}")
}
```

```output
1: first
```

Leaving stored loans out keeps every loan's lifetime visible in one function, and adding them later would break
no program, whereas removing them after a release would (LR 7.7).

## Shared ownership

When a value really must have several owners, `Shared[T]` is a reference-counted owner, and the cost is written
where it happens (LR 7.8):

| Operation | Gives | Meaning |
|---|---|---|
| `Shared.create(move value)` | `Shared[T]` | takes ownership; one strong handle |
| `s.retain()` | `Shared[T]` | another strong handle; the only way to duplicate one, since `copy` is refused |
| `s.downgrade()` | `Weak[T]` | a handle that does not keep the value alive |
| `w.upgrade()` | `Shared[T]?` | a strong handle while one still exists, `null` afterwards |
| `w.retain()` | `Weak[T]` | another weak handle |

Reading through a `Shared[T]` is a read loan of the value (`s.field`, `s.method()`); a `Shared` gives read access
only, so mutation needs a `Mutex` or an atomic (LR 7.8, LR 12.7). `retain` and `downgrade` have no effects, so a
`pure` function may create and pass handles; `upgrade` is `nondet`, because its answer depends on whether another
task has dropped the last strong handle (LR 7.8). Cycles of `Shared` handles leak and the checker does not detect
them; a back-reference uses `Weak`.

```fortis
// accept
import fortis.host { Host }

class Settings {
  export name: String
}

export func main(args: List[String]) Void {
  let caps = Host.capabilities()
  let first = Shared.create(Settings(name: "production"))
  let second = first.retain()
  let weak = second.downgrade()
  caps.console.print("${first.name} and ${second.name}")
  match weak.upgrade() {
    present(again) => caps.console.print("still alive: ${again.name}")
    null => caps.console.print("gone")
  }
}
```

```output
production and production
still alive: production
```

```fortis
// reject: E-OWN-002
class Settings {
  export name: String
}

export func main(args: List[String]) Void {
  let first = Shared.create(Settings(name: "production"))
  let second = copy first                     // reject: E-OWN-002
}
```

A `Shared` of a capability carries the capability, which is how several values hold one console: see the
example under destruction below and LR 11.6.

## Destruction

A value is destroyed at exactly one of these events (LR 7.9):

| Event | Order |
|---|---|
| block exit | the block's loans end, then its `defer`s run last-first, then its locals are dropped in reverse order of initialisation |
| assignment to a `let mut` owner | the new value is evaluated, then the old value is dropped, then the store |
| temporaries | at the end of the statement, in reverse order of creation |
| `break`, `continue`, `return`, `fail` | each block left, innermost first, in block-exit order |
| a failed constructor function | the locals it initialised, in reverse order |
| a class value | its own `cleanup` method if it declares one, then its fields in declaration order |

A linear value is never destroyed implicitly: the obligation rules make that unreachable (LR 7.9). The order is
the same on every backend (LR 7.9):

```fortis
// accept
import fortis.host { Capabilities, Console, Host }

class Resource {
  export id: Int
  export console: Shared[Console]
  cleanup(editable self) Void {
    self.console.print("release ${self.id}")
  }
}

func work(console: Shared[Console]) Void {
  let first = Resource(id: 1, console: console.retain())
  let second = Resource(id: 2, console: console.retain())
  console.print("working")
}

export func main(args: List[String]) Void {
  let caps = Host.capabilities()
  let Capabilities(console: out, fileSystem: fs, network: net, clock: clock, random: rnd, environment: env, process: proc, supervisor: sup, secretAccess: sec, rawSql: sql) = move caps
  let console = Shared.create(move out)
  work(console)
  console.print("done")
}
```

```output
working
release 2
release 1
done
```

`second` was initialised last, so it is dropped first. `main` takes the console out of the capabilities with a
consuming pattern and puts it behind a `Shared`, because a field may not hold a loan of it (LR 7.7, LR 11.6).

## The Rust-expressible subset

The ownership rules were chosen so that every accepted program has a lowering to safe Rust, listed shape by shape
in LR Appendix C (LR 7.10). A situation the checker can describe but that has no such shape, for example a loan
stored inside a `Shared`, is rejected with `E-OWN-090` and a pointer to the nearest supported pattern; it is
never lowered with `unsafe` (LR 7.10, LR C.2).

```fortis
// reject: E-OWN-090
class User {
  export name: String
}

pure func displayName(user: User) StringView from user => alias user.name

func run(user: User) Int {
  let seen = displayName(alias user)
  let shared = Shared.create(move seen)    // reject: E-OWN-090
  return shared.scalarCount()
}

export func main(args: List[String]) Void {
}
```

Widening the subset is a language change with a recorded decision, never an implementation shortcut, and the
checker never inserts a clone, a reference count or hidden sharing to rescue a rejected program (LR 7.10).
