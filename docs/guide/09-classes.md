# Classes, interfaces, mixins and enums

<!-- covers: LR 9 -->
*Human mode. Agent mode: [Classes, interfaces, mixins and enums, by rule](../agent/09-classes.md). Source of
truth: LR 9.*

Fortis has no inheritance. A **class** is a named record with methods; an **interface** is a set of method
signatures a type can promise to implement; a **mixin** contributes ready-made methods to classes that ask for
it; an **enum** is a closed set of variants, each with its own payload. `match` takes any of them apart. Each of
these is nominal and visible in the source: a reader never has to search a hierarchy to find which code runs
(LR 9.1).

## Classes

A class declares fields and methods. Fields and methods are private to the module unless marked `export`, and a
field may have a constant default (LR 9.1, LR 4.4):

```fortis
// illustrative
[export] [trivial] class Name[TypeParams] [derive I1, I2] [implements I3, I4] [with M1, M2] {
  [export] field: Type
  [export] field: Type = constantDefault
  [export] … func method([editable | owned] self, params) Ret { … }
  [export] … func associated(params) Ret { … }
  cleanup(editable self) { … }
}
```

There is no inheritance, no virtual dispatch between classes and no lookup of members at run time (LR 9.1).
Fields, methods, associated functions and constants of a class share one namespace, so a name may be declared
only once in it (`E-TYP-070`, LR 9.1). That includes a field and a method with the same name:

```fortis
// reject: E-TYP-070
class Account {
  export owner: String
  balance: Int
  export pure func balance(self) Int => self.balance      // reject: E-TYP-070
}

func main() {
}
```

## Construction

Every class has a **generated constructor**, `Name(field: value, …)`, which takes every field as a named argument;
fields with defaults may be left out (LR 9.2). The constructor takes ownership of what it is given, so a field
argument that is an existing ownership-bearing place needs `copy` or `move`, exactly as in any other owned
position (LR 9.2, LR 7.5). There is never a half-built object: either every field has a value or there is no
object.

```fortis
// accept
import fortis.host { Console }

class Point derive Equatable {
  export x: Int
  export y: Int
  export label: String = "point"
}

func main(console: Console) {
  let name = "home" as String
  let a = Point(x: 1, y: 2)
  let b = Point(x: 1, y: 2, label: copy name)
  console.print("${a.label} ${b.label} ${name}")
  console.print("${a == Point(x: 1, y: 2)}")
}
```

```output
point home home
true
```

The generated constructor is callable only where every field is visible (LR 9.2). A class with a private field
can therefore be built only inside its module, which is how a class protects its invariants: other modules must
go through the functions it exports. Here the `accounts` module of a library keeps `balance` private, and a test
in a second module, `checks`, opens an account through `Account.open`:

```yaml
# file: demo.accounts/project.yaml
schema: fortis.project/v1
name: demo.accounts
version: 1.0.0
exports: [accounts]
dependencies:
  fortis: "1.0"
```

```fortis
// accept
// file: demo.accounts/src/accounts.ft
export class Account {
  export owner: String
  balance: Int

  export pure func open(owner: owned String) Account => Account(owner: move owner, balance: 0)
  export pure func current(self) Int => self.balance
}
```

```fortis
// accept
// file: demo.accounts/src/checks.ft
import demo.accounts.accounts { Account }

test "a new account is empty" {
  let a = Account.open("ada")
  expect a.current() == 0
}
```

Calling the generated constructor from any other module is `E-NAM-004` (LR 4.4, LR 9.2), here from an application
that depends on the library:

```yaml
# file: demo.misuse/project.yaml
schema: fortis.project/v1
name: demo.misuse
version: 1.0.0
entry: app
exports: []
dependencies:
  fortis: "1.0"
  demo.accounts: "1.0.0"
```

```fortis
// reject: E-NAM-004
// file: demo.misuse/src/app.ft
import demo.accounts.accounts { Account }

func main() {
  let a = Account(owner: "ada", balance: 100)          // reject: E-NAM-004
}
```

### Associated functions

A function declared inside a class without a `self` parameter is an **associated function**, called as
`Name.function(…)` (LR 9.2). Constructors that must validate or may fail are associated functions; by convention
they are named `create`, `of` or `from`, since Fortis has no `new` (LR 1.1). Because associated functions are the
only way to reach private fields, they are where a class enforces its invariants against every caller (LR 9.2).

```fortis
// accept (exit status 1)
import fortis.host { Console }

export error EmptyName {
  code: "people.empty_name"
}

class Person {
  export name: String
  years: Int<0..150>

  export pure try func create(name: owned String, age: Int<0..150>) Person {
    if name.scalarCount() == 0 {
      fail EmptyName()
    }
    return Person(name: move name, years: age)
  }

  export pure func age(self) Int<0..150> => self.years
}

try func main(console: Console) {
  let ada = Person.create("Ada" as String, 36) or fail
  console.print("${ada.name} is ${ada.age()}")
  let nobody = Person.create("" as String, 0) or fail
  console.print("not reached")
}
```

```output
Ada is 36
```

The second call fails, `or fail` passes the failure on, and `main` ends with exit status 1 and one failure record
on standard error (LR 4.6).

### Spreading one value into another

A constructor call may start with a **spread**, `...copy p` or `...move p`, which fills every field of the new
value from the field of the same name and type in `p` (LR 9.2). The remaining fields are given explicitly after
it, and defaults fill the rest. This is how a value is projected into a narrower or wider type without naming each
field.

```fortis
// accept
import fortis.host { Console }

class Customer {
  export name: String
  export email: String
  export notes: String
}

class Summary {
  export name: String
  export email: String
  export invoices: Int
}

func main(console: Console) {
  let customer = Customer(name: "Ada", email: "ada@example.org", notes: "prefers email")
  let summary = Summary(...copy customer, invoices: 3)
  console.print("${summary.name} <${summary.email}>: ${summary.invoices} invoices")
  console.print("still have ${customer.notes}")
}
```

```output
Ada <ada@example.org>: 3 invoices
still have prefers email
```

`...copy p` copies the fields it takes and leaves `p` alone; `...move p` consumes the whole of `p`, dropping the
fields the target does not have (LR 9.2). A field given both by the spread and explicitly is rejected, and a field
with no source, no explicit value and no default is `E-TYP-050` (LR 9.2). Dropping a linear field this way is
never allowed: `...move p` from a value holding an `Outcome` the target has no room for is `E-OBL-007` (LR 9.2).
A spread reads the source's fields, so they must all be visible where it is written, and `...move p` is not
permitted when `p`'s class declares a `cleanup` (LR 9.2).

## Methods and receivers

A method is a function declared in a class whose first parameter is `self` (LR 9.3). The receiver says what the
method may do with the value. A call that consumes it says so with `move`; a call that changes it is written plainly
and is accepted only on a place that may change (a `let mut` binding, an `editable` parameter or capture, an
exclusive loan, or the method's own `editable self`), so the place's declaration already says it (LR 9.3, LR 7.5,
LR 7.6):

| Receiver | The method | Call site |
|---|---|---|
| `self` | reads the value | `value.method(args)` |
| `editable self` | may change it | `place.method(args)`, on a place that may change |
| `owned self` | consumes it | `move place.method(args)` |

Older code writes `editable place.method(args)`; it is still accepted, and `fortis fmt` drops the marker.

```fortis
// accept
import fortis.host { Console }

class Counter {
  export name: String
  count: Int<0..1000>

  export pure func start(name: owned String) Counter => Counter(name: move name, count: 0)

  export pure func value(self) Int<0..1000> => self.count

  export pure func bump(editable self) {
    if self.count < 1000 {
      self.count = self.count + 1
    }
  }

  export func finish(owned self) String => "${self.name}: ${self.count}"
}

func main(console: Console) {
  let mut hits = Counter.start("hits" as String)
  hits.bump()
  hits.bump()
  console.print("value ${hits.value()}")
  let summary = move hits.finish()
  console.print(summary)
}
```

```output
value 2
hits: 2
```

Inside a method, fields are always reached through `self` (LR 4.5). A `pure` method may write through
`editable self`, because that write is its declared footprint, but it may not perform I/O or write anything else
(LR 9.3, LR 11.3). A class may also declare associated constants, `const name = …`, read as `Name.name` (LR 9.3).

## Cleanup

A class may declare `cleanup(editable self)`, which runs when a value of the class is destroyed, before its
fields are dropped (LR 9.4). The fields are then dropped in declaration order (LR 7.9). Cleanup can only release:
it cannot be `try` or `async`, and a finalisation that can fail (a flush, a commit) is an ordinary method that the
caller invokes explicitly (LR 9.4).

```fortis
// accept
import fortis.host { Console }

class Connection {
  export id: Int
  export console: Shared[Console]
  cleanup(editable self) {
    self.console.print("closing ${self.id}")
  }
}

class Pool {
  export first: Connection
  export second: Connection
}

func open(console: Shared[Console]) {
  let pool = Pool(first: Connection(id: 1, console: console.retain()), second: Connection(id: 2, console: console.retain()))
  console.print("pool ready")
}

func main(c: Console) {
  let console = Shared.create(move c)
  open(console.retain())
  console.print("done")
}
```

```output
pool ready
closing 1
closing 2
done
```

The connections hold the console through `Shared`, because a field never stores a loan (LR 7.7, LR 7.8).
`cleanup` cannot be called directly; it runs only at destruction (`E-TYP-040`, LR 9.4):

```fortis
// reject: E-TYP-040
class Cell {
  export value: Int
  cleanup(editable self) {
  }
}

func main() {
  let mut c = Cell(value: 1)
  c.cleanup()                              // reject: E-TYP-040
}
```

## Interfaces

An interface declares method signatures (with their modifiers and receiver modes), associated functions and
associated constants, and no bodies (LR 9.5). A type implements it only by saying so, with `implements`, with
`derive`, or with an `implement Interface for Type { … }` block; having methods with the right names is not
enough (LR 9.5).

```fortis
// accept
import fortis.host { Console }

interface Shape {
  pure func area(self) Int
  pure func name(self) StringView from static
}

class Square implements Shape {
  export side: Int<0..1000>
  export pure func area(self) Int => self.side * self.side
  export pure func name(self) StringView from static => "square"
}

class Rect implements Shape {
  export width: Int<0..1000>
  export height: Int<0..1000>
  export pure func area(self) Int => self.width * self.height
  export pure func name(self) StringView from static => "rectangle"
}

pure func larger[T](a: T, b: T) Int where T implements Shape {
  if a.area() >= b.area() {
    return a.area()
  }
  return b.area()
}

func main(console: Console) {
  console.print("${larger(Square(side: 3), Square(side: 4))}")
  console.print("${Rect(width: 2, height: 5).name()}")
}
```

```output
16
rectangle
```

A generic function like `larger` is checked once against its bound and compiled once per type it is used with,
so dispatch is static rather than through a dynamic interface (LR 6.10).

An implementation must provide every member with a compatible signature, and it may not strengthen a precondition
or weaken a guarantee: it cannot drop `pure` or `noalloc`, add `try`, `async` or `recovers`, narrow a parameter's
refinement, widen a return refinement, or drop a contract's clause (LR 9.5, LR 11.2). Every such mismatch, a
missing member, an associated constant of a type other than the interface declares, and an unmet mixin requirement
is `E-TYP-080` (LR 9.5). Callers through the interface see the interface's contract only:

```fortis
// reject: E-TYP-080
interface Priced {
  pure func price(self) Int
}

class Pear implements Priced {                  // reject: E-TYP-080
  export func price(self) Int => 4
}

func main() {
}
```

The `implement` block is the form used for an implementation declared apart from the type's own head (LR 9.5).
In Language 1.0 its target must be a non-generic class or enum the program declares, however the block names it
(an import alias names the declaration it resolves to), and the block is then exactly the target listing the
interface in `implements`, with the block's methods, which are reached through the interface (LR 9.5, X-180):

```fortis
// accept
import fortis.host { Console }

interface Describe {
  pure func describe(self) String
}

class Temperature {
  export celsius: Int
}

implement Describe for Temperature {
  pure func describe(self) String => "${self.celsius} C"
}

pure func show[T](x: T) String where T implements Describe => x.describe()

func main(console: Console) {
  console.print(show(Temperature(celsius: 21)))
}
```

```output
21 C
```

A block for a core type (`implement Labelled for Int`), a newtype, an optional, a generic class or enum
(`implement Labelled for Pair[Int]`) or any other type is `E-TYP-171`: core types and newtypes have no run-time
identity of their own in Language 1.0's backends, and a block for one instantiation of a generic type would need
the backends to tell its instantiations apart; a generic implementation
(`implement Displayable for Pair[T] where T implements Displayable`) is therefore not in Language 1.0 (LR 9.5,
X-180). A block method the target already has, its own or a mixin's, or that another block for the target adds
in any module of the package, is a member declared twice (`E-TYP-070`); a block for an interface the target
already implements or derives is the overlap `E-TYP-100` alone. Two blocks for one interface and one target are
an overlap whatever their modules; the target is the declaration with its type arguments, so `Box[Int]` and
`Box[String]` are two targets, and a block in one module of a package is seen by every module of it (LR 9.5,
D-132).

```fortis
// reject: E-TYP-171
interface Describe {
  pure func describe(self) String
}

implement Describe for Int {                 // reject: E-TYP-171 — a core type is no target
  pure func describe(self) String => "an int"
}

func main() {
}
```

### Requirements, bundles and markers

An interface may **require** others: a type implementing `Hashable` must also implement `Equatable` (LR 9.5). A
requirement is matched by identity and type arguments: the interface the requiring declaration's own module
names, with the arguments the requirement states, so `Svc requires Tagged[String]` is not met by an
implementation of `Tagged[Int]`, nor by another module's `Tagged` (LR 4.4, LR 9.5). A `dynamic J` can be used as `dynamic I` when `J` requires `I` and both are dynamic-eligible
(LR 9.5). `Equatable` itself is not eligible because `equals` takes a second `Self`; see
[Dynamic dispatch](#dynamic-dispatch). An interface with requirements and no members of its
own is a **bundle**, satisfied by every type that implements
everything it requires, without being named: `Codec` is `Encodable` plus `Decodable` (LR 9.5). An interface with
neither members nor requirements is a **marker**, such as `Authority`, and is only ever implemented by explicit
declaration (LR 9.5). A cycle of requirements is `E-TYP-060` (LR 9.5):

```fortis
// reject: E-TYP-060
interface Left requires Right {             // reject: E-TYP-060
}

interface Right requires Left {              // reject: E-TYP-060
}

func main() {
}
```

An interface may carry a `where` clause constraining its type parameters before its body, and it is checked at
every implementation and use: `class C implements Source[Int]` with `interface Source[T] where T implements
Tag` is rejected when `Int` does not implement `Tag` (`E-TYP-080`), and so is the type `dynamic Source[Int]`
wherever it is written (`E-TYP-090`, the code an unmet bound has at a call; LR 6.10, LR 9.5).

Deriving an interface and also writing the method the derivation would generate is `E-MIX-001` (LR 9.5). There
are no default method bodies in interfaces: shared behaviour on top of an interface is a mixin that requires
the interface's methods it uses (LR 9.5, LR 9.6). A mixin requires methods only, never an interface:
`requires Displayable` is not a sentence of the grammar (`E-SYN-000`, LR 9.6, X-526).

### Coherence

A package may implement an interface for a type only if it defines the interface or the type, and one type has at
most one implementation of an interface. Both violations are `E-TYP-100` (LR 9.5):

```fortis
// reject: E-TYP-100
interface Named {
  pure func name(self) String
}

class Fig implements Named {
  export pure func name(self) String => "fig"
}

implement Named for Fig {                    // reject: E-TYP-100
  pure func name(self) String => "fig"
}

func main() {
}
```

### Dynamic dispatch

When one place must hold values of different types that share an interface, write `dynamic Interface` (LR 9.5).
The value is boxed when it is created, which allocates, and calls through it are dispatched at run time. It is
always written with an ownership form: `dynamic Shape` (owned), `alias dynamic Shape` (a loan) or
`Shared[dynamic Shape]` (LR 9.5).

```fortis
// accept
import fortis.host { Console }

interface Shape {
  pure func area(self) Int
  pure func name(self) StringView from static
}

class Square implements Shape {
  export side: Int<0..1000>
  export pure func area(self) Int => self.side * self.side
  export pure func name(self) StringView from static => "square"
}

class Rect implements Shape {
  export width: Int<0..1000>
  export height: Int<0..1000>
  export pure func area(self) Int => self.width * self.height
  export pure func name(self) StringView from static => "rectangle"
}

func main(console: Console) {
  let mut shapes: List[dynamic Shape] = List.default()
  shapes.push(Square(side: 2))
  shapes.push(Rect(width: 2, height: 5))
  let mut total = 0
  for shape in shapes {
    console.print("${shape.name()}: ${shape.area()}")
    total = Int.saturatingAdd(total, shape.area())
  }
  console.print("total ${total}")
}
```

```output
square: 4
rectangle: 10
total 14
```

An interface is not itself a type a value has: a field, variant payload, parameter or result declared with a
bare interface name is rejected at its declaration (`E-TYP-040`), naming `dynamic I`, `Shared[dynamic I]` and a
type parameter bounded by `I` (LR 9.5). And only some interfaces are dynamic-eligible: one is eligible when it
has no associated types, no generic methods and no method taking or returning `Self` beyond its receiver — a
`Self` parameter, lent or owned, names a second value whose type a `dynamic` cannot know. `dynamic I` of any
other interface is rejected wherever the type is written, a declared type, a lambda's or a `let` annotation
(`E-TYP-172`); an associated function or constant does not count, since a `dynamic` value does not reach it
(LR 9.5, LANG-09-07).

```fortis
// reject: E-TYP-172
func main() {
  let nothing: dynamic Equatable? = null    // reject: E-TYP-172 — `equals` takes a Self, so Equatable is not dynamic-eligible
}
```

A value converts to `dynamic I` only from a type that implements `I` with the written type arguments, from a
type parameter whose bound grants `I` (directly or through an interface that requires it), or from a
`dynamic J` where `J` is `I` or requires it; any other conversion is `E-TYP-040`. An `async` method called
through `dynamic` allocates its future, and such a call is rejected in `noalloc` code (LR 9.5).

## Mixins

A mixin contributes method bodies to the classes that apply it with `with`. It declares what it needs as
`requires` method signatures, never fields, and never an interface — a mixin requires methods only
(`requires Displayable` is not a sentence of the grammar, `E-SYN-000`, LR 9.6, X-526) — and it cannot add
fields, constructors or a cleanup (LR 9.6). A method
the class declares itself wins over a mixin method of the same name (LR 9.6).

```fortis
// accept
import fortis.host { Console }

mixin Loud {
  requires pure func name(self) String
  export pure func shout(self) String => "${self.name()}!"
  export pure func whisper(self) String => "(${self.name()})"
}

class Dog with Loud {
  export pure func name(self) String => "woof"
  export pure func whisper(self) String => "..."
}

func main(console: Console) {
  let dog = Dog()
  console.print("${dog.shout()} ${dog.whisper()}")
}
```

```output
woof! ...
```

A requirement is a signature: the class's method of that name is compared with it as an implementation is with
an interface member — receiver, modifiers and claims, parameters, result — or the application is `E-TYP-080`,
and a method the class has by derivation or through an interface it lists meets a requirement of that name. The
method that meets a requirement is visible to the mixin's bodies whether or not the class exports it, since the
requirement names it; that exception grants access only to the mixin declaring the requirement, not to another
applied mixin or to an ordinary caller outside the class's module (LR 9.6, X-478). A mixin's method may in turn
satisfy a member of an interface the class lists (LR 9.6).

A mixin body is checked at each application site, as a method of the applying class, and sees only the class's
exported members and the methods meeting its requirements, so a mixin never reads a private field (`E-NAM-004`);
a type error in a mixin body is reported at the body for each class that applies it. This includes an
application from another module of the package: the names written in the body resolve in the mixin's declaring
module, including that module's private functions, constants and types, `self` denotes the applying class, and
the diagnostic names the mixin's source file and line and identifies the applying class and its module
(LR 9.6, X-476). Two applied mixins that provide the same method, with no method of that name in the
class to settle it, are `E-MIX-001` (LR 9.6):

```fortis
// reject: E-MIX-001
mixin English {
  export pure func hello(self) StringView from static => "hello"
}

mixin French {
  export pure func hello(self) StringView from static => "bonjour"
}

class Greeter with English, French {        // reject: E-MIX-001
  export id: Int
}

func main() {
}
```

## Enums

An enum is a closed set of variants, each optionally carrying named payload fields (LR 9.7). A variant is
constructed as `Enum.Variant` or `Enum.Variant(field: value)`, and the `Enum.` prefix may be left out wherever the
expected type is already that enum: in a `match` arm pattern, a `return`, an argument, a field initialiser, an
annotated `let`, or the right side of a comparison such as `severity == Error` (LR 9.7). If the bare name is also a
class, an error or another type in scope, it is ambiguous, and the checker asks for `Severity.Error` (`E-TYP-040`),
so a name never quietly means something else. Enums may have methods and implement interfaces, and an enum whose payloads are all
trivial is itself trivial (LR 9.7).

```fortis
// accept
import fortis.host { Console }

enum Coin derive Equatable {
  Penny
  Pound
  Other(pence: Int<0..10000>)

  export pure func pence(self) Int<0..10000> {
    return match self {
      Penny => 1
      Pound => 100
      Other(pence: p) => p
    }
  }
}

pure func larger(a: Coin, b: Coin) Coin {
  if a.pence() >= b.pence() {
    return a
  }
  return b
}

pure func rounded(c: Coin) Coin {
  if c.pence() >= 100 {
    return Pound
  }
  return Penny
}

func main(console: Console) {
  let c = larger(Pound, Other(pence: 250))
  console.print("${c.pence()}")
  console.print("${rounded(c) == Pound}")
}
```

```output
250
true
```

`return Pound` in `rounded` needs no prefix, because the function returns a `Coin`. Enums are closed: a source
enum is never silently extended, and every `match` over one must handle every variant (LR 9.7).

## Patterns and `match`

`match` evaluates its subject once, tries the arms from top to bottom and runs the first that matches; there is no
fall-through (LR 9.8). As an expression, every arm gives a value of one type, with `give` at the end of a block
arm (LR 9.8).

| Pattern | Matches |
|---|---|
| `name` | anything, and binds it |
| `_` | anything |
| a literal | an equal integer, string or Boolean |
| `a..b`, `a..<b`, `a..`, `..<b` | an integer scrutinee whose value lies in the interval |
| `Variant`, `Variant(field: name)`, `Variant(name)` | an enum variant, binding its payload |
| `present(name)`, `null` | an optional |
| `success(name)`, `failure(name)` | an outcome ([Outcomes and failures](10-outcomes.md)) |
| `is ErrorName(…)` | a failure of a declared kind |
| `Name(field: p, …)` | a class, consuming it (`match move` only) |
| `(p1, p2)`, `{ field: p }` | a tuple, an anonymous record |
| `otherwise` | the rest, for open types only |

These are the patterns of LR 9.8; patterns nest, and an arm may add a guard with `when` (LR 9.8). A guarded arm
does not count toward exhaustiveness.

```fortis
// accept
import fortis.host { Console }

enum Light {
  Red
  Amber
  Green(seconds: Int<0..600>)
}

pure func show(light: Light) String {
  return match light {
    Red => "stop"
    Amber => "wait"
    Green(seconds: s) when s > 60 => "go (long)"
    Green(seconds: s) => "go for ${s}s"
  }
}

pure func size(n: Int) StringView from static {
  return match n {
    0 => "none"
    1 => "one"
    otherwise => "many"
  }
}

pure func orZero(n: Int?) Int {
  return match n {
    present(v) => v
    null => 0
  }
}

func main(console: Console) {
  console.print("${show(Green(seconds: 90))}, ${show(Green(seconds: 20))}, ${show(Red)}")
  console.print("${size(0)} ${size(7)} ${orZero(null)} ${orZero(5)}")
}
```

```output
go (long), go for 20s, stop
none many 0 5
```

### Exhaustiveness

A `match` over a closed type, an enum or an optional, must name every case, and each missing variant is reported
by name (`E-MAT-001`, LR 9.8). The record points at the `match`:

```fortis
// reject: E-MAT-001
enum Light {
  Red
  Amber
  Green
}

pure func code(l: Light) Int {
  return match l {                       // reject: E-MAT-001
    Red => 1
    Amber => 2
  }
}

func main() {
}
```

A catch-all (`_`, a bare name or `otherwise`) as the last arm over a closed enum is `E-MAT-002`: if it were
allowed, adding a variant later would silently fall into it, instead of surfacing every `match` that must now
handle it (LR 9.8). `otherwise` is for open types: integers, strings and `Failure` (LR 9.8).

```fortis
// reject: E-MAT-002
enum Light {
  Red
  Amber
  Green
}

pure func code(l: Light) Int {
  return match l {
    Red => 1
    _ => 2                               // reject: E-MAT-002
  }
}

func main() {
}
```

An arm may list alternatives, `Red | Amber => true`, when they share a result: the arm matches when any alternative does. The alternatives bind no names, are never `otherwise`, and the arm takes no `when` guard (`E-MAT-003`); every variant still has to be named somewhere, so a new variant is still reported (LR 9.8). Binding-with-subpattern (`x @ pattern`) is not in Fortis 1.0.

### Range patterns

A **range pattern** matches an integer scrutinee — `Int` or a sized integer type, refined or not — whose value
lies in its interval: `a..b` includes `b`, `a..<b` excludes it, and the one-sided forms `a..` and `..<b` run to
the limits of the scrutinee's own type (LR 9.8, X-517). Each bound is a constant expression that folds as a
refinement's bound folds, an integer literal or a `const` (LR 6.3). An empty or inverted interval (`5..1`,
`5..<5`), a bound that does not fold, or one outside the scrutinee type's own range is `E-MAT-005`. A range
pattern over a scrutinee of any other type is `E-TYP-040`: a refinement alias or a newtype (`type Score =
Int<0..100>`) is not an integer scrutinee — match on the refined integer type itself, or on a binding the
conversion produced (LR 9.8). A range pattern binds no names, nests in payloads and alternatives like any
pattern, and in a destructuring `let` it is refutable as any pattern is (`E-MAT-004`, LR 5.1).

```fortis
// accept
import fortis.host { Console }

const passMark: Int = 40

pure func grade(mark: Int<0..100>) StringView from static {
  return match mark {
    70..100 => "distinction"
    passMark..<70 => "pass"
    0..<passMark => "fail"           // the three intervals cover 0..100, so no `otherwise` is needed
  }
}

func main(console: Console) {
  console.print("${grade(85)} ${grade(50)} ${grade(10)}")
}
```

```output
distinction pass fail
```

Over a refined integer scrutinee, exhaustiveness counts intervals (LR 9.8, X-518): the unguarded arms whose
intervals together cover the scrutinee's whole interval make the match exhaustive without `otherwise` (which
remains permitted as the catch-all), and the intervals left uncovered are named in the record (`E-MAT-001`);
guarded arms and literal arms do not count toward the coverage. An unrefined integer scrutinee keeps its
catch-all requirement. Inside an arm the scrutinee is narrowed to the arm's interval, as a `when` guard
narrows, and a later unguarded arm sees the intervals the earlier unguarded arms covered removed from its
scrutinee's known interval (LR 6.3, LR 9.8).

```fortis
// reject: E-MAT-005
pure func band(n: Int) Int {
  return match n {
    5..1 => 1                              // reject: E-MAT-005 — the interval is inverted
    otherwise => 0
  }
}

func main() {
}
```

### Ownership in patterns

`match p` borrows `p` for the match, and the bindings in its arms are read loans. `match move p` consumes `p`,
and the bindings own their values (LR 9.8). Taking a class apart with a class pattern consumes it, so it is
written with `match move` or `let Pattern = move p`; every field is named, and `_` drops a field that is not
linear (LR 9.8). A class pattern needs every field of the class to be visible where it is written, and a class
that declares a `cleanup` cannot be taken apart this way (LR 9.8). This is the one way to take a linear field out of a class, since there are no partial moves
(LR 9.8, LR 7.2). A `let` pattern has no other case to fall back on, so it must match every value: a variant, a
literal, `null` or `present(…)` anywhere inside it is `E-MAT-004`, and `match` is the form that handles the other
cases (LR 5.1).

```fortis
// accept
import fortis.host { Console }

class Parcel {
  export label: String
  export grams: Int
}

pure func heavy(p: owned Parcel) Bool {
  return match move p {
    Parcel(label: _, grams: g) => g > 1000
  }
}

func main(console: Console) {
  let parcel = Parcel(label: "books", grams: 2500)
  let Parcel(label: label, grams: grams) = move parcel
  console.print("${label}: ${grams} g")
  console.print("${heavy(Parcel(label: "feather", grams: 1))}")
}
```

```output
books: 2500 g
false
```

A range pattern establishes its facts before the guard runs (LR 9.8; X-713). If the guard writes the scrutinee's
place, or loans it or its owner editable, the arm body loses those facts. A guard that only reads keeps them.
A separately bound integer copy keeps its own narrowing when the original owner changes.
