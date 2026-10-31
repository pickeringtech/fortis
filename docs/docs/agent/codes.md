# Diagnostic codes

<!-- covers: LR 0 -->
*Agent mode. Source of truth: the LR section cited in each row; classes and mandatory fields:
[records.md](records.md).*

Every diagnostic code the Language Reference names, grouped by class. **Rule** is the rule ID the toolchain's
record carries for the code (most often; where several rules share one code, the record names the one the
finding breaks; where the record carries a rule that is not the one the LR states for the code, the row says so,
D-57). **Pairs** is the agent page holding a reject/accept correction pair for the code, or the rules
for it. Look a record up by its `code`, read the cited section, apply the correction, re-check.

## Syntax (E-SYN)

| Code | Rule | LR | Trigger | Correction | Pairs |
|---|---|---|---|---|---|
| E-SYN-000 | LANG-03-02, LANG-09-04 | LR 3.2, LR 5.6, LR 9.6, LR C.2 | not a sentence of the grammar (also `f() or fail ?? 0`; a loan type naming two origins; a mixin that `requires` an interface) | fix the syntax; split the expression; `let v = f() or fail` on its own line; make the mixin require the interface's methods it calls (`requires pure func title(self) String`) | [03](03-lexical.md), [05](05-statements.md) |
| E-SYN-001 | LANG-03-01 | LR 3.2 | a `;` outside strings, comments, raw template regions | delete it (the record carries the edit) | [03](03-lexical.md), [B](appendix-b-decisions.md) |
| E-SYN-002 | LANG-03-04 | LR 3.4 | a keyword in a declaration or binding position | rename; a method may keep a keyword name | [A](appendix-a-keywords.md) |
| E-SYN-004 | LANG-05-06 | LR 5.6 | outcome operators, `??` and `&&`/`\|\|` mixed without parentheses | parenthesise | [05](05-statements.md) |

## Names and modules (E-NAM)

| Code | Rule | LR | Trigger | Correction | Pairs |
|---|---|---|---|---|---|
| E-NAM-001 | LANG-04-05 (as recorded; for an assignment the rule is LANG-05-01, D-57) | LR 4.5, LR 5.1, LR 10.3, LR 13 | an undeclared name (assignment to it, a use of it; `log` other than as a log statement, where nothing named `log` is declared; a `${name}` in an error's `message` that is not a payload field); also used for other unresolved names and module errors | declare it, import it, or fix the name; a log is `log.info(m)`, `log.warn(m)` or `log.error(m)` as a statement; a message names only payload fields | [05](05-statements.md), [04](04-modules.md) |
| E-NAM-002 | LANG-05-01 | LR 5.1 | assignment to an immutable binding or a parameter | `let mut`; copy a parameter into a `let mut` local | [05](05-statements.md) |
| E-NAM-003 | LANG-04-05 | LR 4.5 | a `let` shadows a visible binding | a new name | [05](05-statements.md), [04](04-modules.md) |
| E-NAM-004 | none stated (records carry the shadowing rule in error, D-57) | LR 4.4, LR 6.9, LR 9.6 | a newtype unwrap outside its declaring module, or a declaration or member not visible here (not exported, or a constructor with a non-visible field; a private member of the applying class read in a mixin body) | unwrap a newtype only inside its declaring module; export a declaration or use an associated function; in a mixin, require a method the class provides | [04](04-modules.md), [09](09-classes.md) |
| E-NAM-005 | LANG-04-06 | LR 4.6 | a parameter of the entry `main` the runtime cannot supply: a type other than `List[String]` or a capability imported from `fortis.host`, or a mode, default or `...` | give `main` only `name: List[String]` and `name: Console` (and the other capability types), and take anything else from them | [04](04-modules.md) |
| E-NAM-006 | LANG-04-06 | LR 4.6 | two parameters of the entry `main` with the same type | keep one parameter per type; lend it to the functions that need it | [04](04-modules.md) |
| E-NAM-010 | LANG-04-03 | LR 4.3 | dependency names that make the package/module split ambiguous | rename a package or module | [04](04-modules.md) |
| E-NAM-020 | PROG-ARC-04 | LR 4.1 | lockfile does not pin a dependency, pins one no manifest reaches, or is unreadable | `fortis lock` | [04](04-modules.md) |
| E-NAM-021 | PROG-ARC-04 | LR 4.1 | a package, compiler, standard library or Rust toolchain digest differs from the lockfile | restore the pinned input, or re-lock deliberately | [04](04-modules.md) |
| E-NAM-030 | PLAT-01-03 | LR 4.1 | an import of a standard module the build target's feature set lacks (`check --target`, `build --target`) | remove the import, or choose a target with the feature set | [04](04-modules.md) |
| E-NAM-031 | PLAT-01-01 | LR 4.1 | a standard-package dependency outside the published compatibility set | depend on the set's version | [04](04-modules.md) |
| E-NAM-032 | PROG-CMP-06 | LR 4.1 | a native build of a package that ships a Cargo build script | remove the build script | [04](04-modules.md) |
| E-NAM-034 | PROG-DEL-04 | LR 16 | a declaration head a kit defines (`export model Customer { … }`, `policy`, `page`) whose kit this toolchain does not install; `fields.candidates` names the kit | install the kit and import it, or remove the declaration (a plain `class` where no kit is wanted) | [16](16-kits.md) |

## Type (E-TYP, E-MIX-001; LR 6.10)

| Code | Rule | LR | Trigger | Correction | Pairs |
|---|---|---|---|---|---|
| E-TYP-010 | LANG-05-03 | LR 5.2, LR 6.8 | a non-`Bool` condition (`if n`, `if maybe`) | compare explicitly (`!= 0`, `!= null`) | [05](05-statements.md) |
| E-TYP-020 | LANG-06-05 (a float base); LANG-06-02 (any other form) | LR 6.3 | not a refinement of this base: a float refinement; a base 6.3 does not name (`Bool<0..1>`, `Map[K, V]<length: …>`, a newtype, a type parameter); a measure the base lacks (`Int<scalars: …>`, `String<length: …>`); a bound not resolved at compile time (an undeclared name, a `const` of another type); an empty range (`Int<5..1>`); a range outside the base (`UInt8<0..300>`) | refine an integer, text, bytes or a list in the form 6.3 lists, with bounds that are integer literals or `const`s of the base type, within the base; or validate by hand | [06](06-types.md) |
| E-TYP-030 | none stated (records carry the shadowing rule in error, D-57) | LR 4.4, LR 6.9 | an exported signature names a non-exported type, or a tuple, record or variant map (aspect parameters and the `fortis` package's own signatures excepted) | export the type; use a nominal class in exported signatures | [04](04-modules.md), [06](06-types.md) |
| E-TYP-040 | LANG-06-01, LANG-08-04, LANG-11-06 | LR 3.5, LR 4.5, LR 6.1, LR 6.9, LR 8.1, LR 8.4, LR 9.4, LR 9.7, LR 10.1, LR 11.5 | a type mismatch (an unspecialised or constrained function value, a method value including a forwarded newtype method, or an uncalled payload variant without a complete expected function type; `Void()` although Void has no constructor; a non-async callable where `async fn` or `async try fn` is expected; a callable parameter mode differing from the expected function type; an `Outcome[T]` where `T` is expected; an interpolated `String` bound to `StringView`; a non-`Displayable` interpolation; a direct `cleanup` call; a bare variant whose name is also a class, error or type in scope, or that nothing expects the enum of; a newtype where another newtype or its representation is expected, or a value of another type in `Name(x)`; a type argument that differs in its refinement, `List[Int<0..5>]` where `List[Int]` is expected, since type arguments are invariant; a contract's `capability` clause naming a type that is not an Authority type) | call an unspecialised or constrained function directly, or wrap it in a lambda with typed parameters; wrap a method call with the required captures; annotate an enum constructor value with its complete function type; replace `Void()` in a callback with a block lambda `fn(n) { }` or a function returning nothing; declare the function `async`, or wrap its call in an async lambda; match each callable parameter mode to its expected type; consume the outcome first; fix the type; never call `cleanup`; qualify the variant (`Severity.Error`) or annotate the binding; build the newtype that is expected, from a value of its representation type; build a container of the expected element type; name an Authority type in a `capability` clause; for an editable, consuming or `try` method through `?.`, test for null and call in that branch with the receiver mode and outcome handling | [10](10-outcomes.md), [06](06-types.md), [08](08-functions.md), [09](09-classes.md), [11](11-effects.md) |
| E-TYP-050 | LANG-05-09 (a call or constructor); LANG-14-01 (a `cases` row) | LR 5.8, LR 9.2, LR 14 | a parameter without a default (a refined one included), or a constructor field, left unbound; a `cases` row that lacks a field the body reads | pass it; give every row the field | [05](05-statements.md), [09](09-classes.md), [14](14-tests.md) |
| E-TYP-060 | LANG-09-03 | LR 9.5 | an interface requirement cycle | break the cycle | [09](09-classes.md) |
| E-TYP-070 | LANG-09-01 | LR 9.1 | a name declared twice in a class (fields, methods, constants share one namespace) | rename one | [09](09-classes.md) |
| E-TYP-080 | LANG-09-03 | LR 9.5, LR 9.6 | incomplete implementation: missing member or required interface (matched by identity and type arguments), incompatible signature, an associated constant of another type than the interface declares, an interface `where` bound the implementation's arguments do not meet, a mixin requirement the class lacks or whose signature its method does not match | add the member, implementation or method; implement the required interface the requirement names with its arguments; give the constant the declared type | [09](09-classes.md) |
| E-TYP-090 | none stated (records carry the trivial-set rule in error, D-57) | LR 6.7, LR 6.10, LR 6.13, LR 13 | a `Set` element or `Map` key without Hashable and Equatable, an unmet generic interface bound, or a derivation whose rule is not met (a field without the interface; `Encodable` with a private field; a canonical codec call on a non-codec type; a non-`Encodable` schema field; `derive convert from S` with a missing, private or non-copyable field or a fallible or consuming field conversion; enum text derivation with payloads, type parameters, another source or names clashing under ASCII case folding) | implement or derive the required interfaces on the key or field type, add the generic `where K implements Hashable` bound, export the fields, or write `encode`/`decode` or an explicit conversion body by hand | [06](06-types.md), [13](13-observation.md) |
| E-TYP-100 | LANG-06-10 | LR 6.10, LR 9.5 | coherence: an implementation outside the packages defining the interface or the type, or an overlap | implement in the defining package; remove the overlap | [09](09-classes.md) |
| E-TYP-110 | LANG-05-05 | LR 5.4 | a path reaches the end of a non-`Void` function; a block arm or `if`-expression branch without `give` (reported at the function) | `return`/`fail` on that path; `give` in the branch | [05](05-statements.md) |
| E-TYP-120 | LANG-05-05, LANG-14-04 | LR 5.4, LR 14 | `give` outside a match arm or `if` expression, `recover` outside `catch`, `break`/`continue` outside a loop; `expect` outside a `test` or `property` body | move it into the construct it ends; move the `expect` into a test, or test the condition with `if` and fail or return | [05](05-statements.md), [14](14-tests.md) |
| E-TYP-130 | LANG-05-09 | LR 5.8 | positional after named, unknown parameter name, parameter bound twice, too many arguments | fix the argument list | [05](05-statements.md) |
| E-TYP-140 | LANG-05-08 | LR 5.5 | an unused non-`Void` expression statement (a call of an `@ignorable` function excepted) | `discard e`, or use the value; or declare the function `@ignorable` when most callers drop its result | [05](05-statements.md) |
| E-TYP-150 | LANG-11-09 | LR 11.8, LR 5.5 | an aspect application that does not match: unknown aspect, wrong arguments, unmet or ambiguous `needs`, `...rest` outside a parameter list; `@ignorable` on a function whose result is linear (`try`, `async`, a linear type) | fix the application or the target's parameters; drop `@ignorable` from a function whose result must be consumed | [11](11-effects.md) |
| E-TYP-160 | LANG-06-11 | LR 6.10 | a type argument fixed neither by the arguments nor by the expected type (`let names = List.default()`; `label(null, …)`, since `null` fixes nothing) | annotate the receiving binding; pass a value of a typed optional | [06](06-types.md) |
| E-TYP-170 | LANG-06-12 | LR 6.10 | an associated call on a type parameter (`T.default()`, `T.fromText(text)`, `T.create()`), in a body, a field default, a constant or a parameter default (X-180) | take a value of `T`, or a function that makes one (`make: fn() T`), as a parameter; call the associated function on a concrete type | [06](06-types.md) |
| E-TYP-172 | LANG-09-07 | LR 9.5 | `dynamic I` of an interface that is not dynamic-eligible: a generic method, or a method taking or returning `Self` beyond its receiver (`dynamic Equatable`) | take the value as a type parameter bounded by `I`, or give the interface a member without `Self` | [09](09-classes.md) |
| E-TYP-171 | LANG-06-13 | LR 9.5 | an `implement` block whose target is not a non-generic class or enum the program declares: a core type (`implement Labelled for Int`), a newtype, an optional, a generic class or enum (X-180) | wrap the value in a class the program declares and implement the interface for that, or list the interface in the type's own `implements` | [09](09-classes.md) |
| E-TYP-180 | LANG-06-14 | LR 6.13 | `x as T` with no conversion for the pair: no row of the built-in table (`Bool` to `Int`, a `Float` into an integer or `Decimal`, which needs a rounding choice) and no path through a third type | convert through the named function the message cites (`Decimal.fromFloat(x, scale, mode)`), or in the steps the table has (`s as Int as Decimal`) | [06](06-types.md) |
| E-TYP-181 | LANG-06-14 | LR 6.13 | `x as T` where `x` already has type `T` (`Int` to `Int64`, a refinement to its base or a wider refinement) | drop the `as`: the value is already a `T` | [06](06-types.md) |
| E-TYP-182 | LANG-06-15 | LR 6.13 | two conversions declared for one pair: a `convert to T` on the source and a `convert from S` on the target naming each other, or two members for one pair — rejected where declared and at an `as` that would have to choose | delete one of the members: a pair converts by one declaration, in the body of its source or its target | [06](06-types.md) |
| E-TYP-183 | LANG-06-15 | LR 6.13 | a `convert` member for a pair the built-in table already decides: a row of it, two names of one range (`Int` and `Int64`), or the identity (`convert to` the declaring type itself) | delete the member: the pair converts without a declaration, or, for the identity, does not convert | [06](06-types.md) |
| E-TYP-184 | LANG-06-15 | LR 6.13 | a `convert` member on a generic class or enum, or one whose other type is not a nominal type (a refinement: `convert to Int<0..10>`) | wrap the value in a non-generic class the program declares and declare the member there; a refinement target stays the table's `validate` row | [06](06-types.md) |
| E-MIX-001 | LANG-09-04 | LR 6.10, LR 9.5, LR 9.6 | two mixins provide one method; a class derives an interface it also implements by hand | declare the method in the class; drop the `derive` or the hand-written method | [09](09-classes.md) |

## Ownership (E-OWN)

| Code | Rule | LR | Trigger | Correction | Pairs |
|---|---|---|---|---|---|
| E-OWN-001 | LANG-07-01 | LR 7.5, LR 7.6 | an existing ownership-bearing place assigned, returned (other than a local, which `return` moves), initialised into a field or passed `owned` without a marker; a loan of a copyable, non-trivial value (`xs.get(0)`, a binding holding one) where an owned value is expected (D-341); an `editable self` receiver or `fn mut` call, marked or not, on a root that cannot change (a `let`, a read parameter or loan, a read `self`), whatever its type; a marker written on a call of a function, which is not a place (`alias first(alias xs)`; D-382) | `copy p`, `alias p` or `move p` (the record carries an edit); for a loan, `let v = xs.get(0)` then `copy v`, or `match … { present(w) => copy w … }`; for a receiver, declare it `let mut` or take it `editable` (X-345: then it needs no marker); for a marker on a call, write the call plain and put the markers on its arguments | [07](07-ownership.md), [records](records.md) |
| E-OWN-002 | LANG-07-08 | LR 7.5, LR 7.6, LR 7.8  | `copy` of a non-copyable place (`Shared`, `Weak`, a resource); a loan of a value that is not Copyable where an owned value is expected, which has no owned copy (D-341) | `retain()` for `Shared`/`Weak`; keep the loan (a read parameter, a `from` result) or take the value out of its owner (`pop`, `move`) | [07](07-ownership.md) |
| E-OWN-010 | LANG-07-05 | LR 7.3, LR 8.3 | a conflicting loan: an exclusive loan while other loans are active; two `editable` captures of one owner | end the earlier loan by closing a block | [07](07-ownership.md), [08](08-functions.md) |
| E-OWN-011 | LANG-07-05 | LR 7.3, LR 8.3 | the owner used while loaned (suspended by an exclusive child, frozen by a read child) | use it after the loan's block ends | [07](07-ownership.md), [08](08-functions.md) |
| E-OWN-012 | LANG-07-04 | LR 5.7, LR 7.3 | a later argument moves, assigns or exclusively loans a place an earlier argument reserved | evaluate the conflicting argument into a binding first | [05](05-statements.md) |
| E-OWN-020 | LANG-07-04 | LR 7.4, LR 7.5 | use after move | `copy` before the move, or re-initialise a `let mut` | [07](07-ownership.md) |
| E-OWN-030 | LANG-06-07 | LR 6.11 | a `trivial class` with a non-trivial field | drop `trivial`, or make the field trivial | [06](06-types.md) |
| E-OWN-040 | LANG-07-01 | LR 7.2, LR C.2, LR 7.5  | a partial move | move the whole value; destructure it with `let Name(…) = move p` | [07](07-ownership.md), [C](appendix-c-lowering.md) |
| E-OWN-050 | LANG-07-06 | LR 7.6 | a result borrowing from two parameters | return an owned value, or split the function | [07](07-ownership.md) |
| E-OWN-051 | LANG-07-06 | LR 7.6 | returning a loan of a local or a temporary; a value of the function's own returned where the result is declared `from` a parameter, which must lend: `move d`, a copy, a fresh value, a tuple or record literal, a constructor, a lambda, a call whose result is owned, a binding that owns a part of what it holds beside a loan (a `??` fallback of its own, a tuple built around one, a branch's own value; D-369, D-388, D-389), or a `??`, `if` or `match` any of whose results is one of these, at any depth (D-373, D-377, D-381); a loan kept past the statement from a possibly owned temporary receiver (a call, conversion, interpolation, or any owned branch of `??`, `if`, `match` or recovery; D-392, D-562); a closure result declared `from` a `move` or `copy` capture (D-379) | return an owned value (`move`); for a `from` result, bind the loan and return an owned value from a function whose result is owned (`let v = xs.get(0)` then `return copy v ?? move d`), or lend on every path; bind a possible temporary receiver first (`let fallback = " temp " as String`, then `(xs.get(0) ?? alias fallback).trim()`); capture `alias` what a closure lends | [07](07-ownership.md) |
| E-OWN-052 | LANG-07-04 | LR 7.3, LR 12.1 | a `defer` body using a binding of its block that holds a loan | use the owner, or move the deferred work | [12](12-async.md) |
| E-OWN-053 | LANG-07-06 | LR 7.7, LR C.2 | a lifetime parameter, or a field or storage type argument of a loan or loan-carrying type (not `from static`), or an unproved static origin | own the value, copy it, hold a `Shared`, or pass it per call. Prove a `from static` source; an annotation cannot change a loan’s origin. | [07](07-ownership.md), [C](appendix-c-lowering.md) |

## Unsupported shape (E-OWN-090; LR 7.10)

| Code | Rule | LR | Trigger | Correction | Pairs |
|---|---|---|---|---|---|
| E-OWN-090 | LANG-07-09 | LR 7.10, LR C.2 | a shape with no lowering: a loan stored in `Shared`/`Mutex`, a `fn mut` escaping into `Shared` | own the value; `Mutex` or atomic state with a `fn` closure | [C](appendix-c-lowering.md) |

## Obligation (E-OBL, E-ERR-001; LR 5.4)

| Code | Rule | LR | Trigger | Correction | Pairs |
|---|---|---|---|---|---|
| E-OBL-001 | LANG-10-01 | LR 5.3, LR 5.5, LR 6.12, LR 10.1, LR 10.7, LR 11.8, LR 12.3, LR 12.4 | a linear value dropped, overwritten or discarded (an `Outcome`, `Failure`, `Future`, `Task`, `Must`; a never-polled future holding nothing linear and no exclusive loan may be discarded, LR 12.3); `clear()` on a list of a linear element type (LR 10.7); an unconsumed linear `owned` parameter on a failing `before` path; an unconsumed consuming-loop item, an exit leaving its linear remainder, or a temporary linear receiver only borrowed by a call or loop; consuming or discharging an existing obligation anywhere in an optional call argument (LR 6.8) | `or fail`, `or recover v`, `catch`, `match`, `report`, `await`, launch, or pass it on; `consumeEach` or `pop` instead of `clear`; consume each loop item before `continue` or body end, finish the traversal before leaving, and bind a temporary owner before borrowing it; for an optional argument that handles an existing obligation, test for null with `match` or `if let`, call in the present branch and consume the obligation on both branches | [10](10-outcomes.md), [05](05-statements.md) |
| E-OBL-002 | LANG-10-01 | LR 10.4, LR 10.1  | a `catch` block path that does not end in `recover`, `return`, `fail` or a `Never` call | end every path | [10](10-outcomes.md) |
| E-OBL-003 | LANG-10-02 | LR 6.12, LR 10.7 | a user generic type instantiated at a linear argument without its corresponding `T: linear`, or a generic function holding one of its own type parameters by value (an `owned` parameter, its result or a callback result, directly or in a structure whose linearity depends on it) instantiated with a linear type for that parameter without declaring `T: linear` (X-519); linear results of `tryMap`, or linear payloads of `Outcome.map` and `Outcome.collect` (X-621); a linear user consuming cursor without a consuming terminal operation (X-622) | declare `where T: linear` and consume the value, or take the `T` by loan only; use a consuming loop to handle each linear item instead of a dropping functional method | [06](06-types.md), [10](10-outcomes.md) |
| E-OBL-005 | LANG-06-09 | LR 6.12, LR 12.4 | a linear value live across a cancellation point (`await`), or at an `or fail`, that does not implement `Abandonable`; a hidden linear loop remainder at an unwind or cancellation point | consume it before the `await`, or pass it into the awaited call; finish a linear consuming loop before an unwind or cancellation point | [12](12-async.md) |
| E-OBL-006 | LANG-10-02 | LR 8.3, LR 10.7 | a `fn` or `fn mut` closure capturing a linear value by `move` | declare it `fn once` | [08](08-functions.md) |
| E-OBL-007 | LANG-10-02 | LR 9.2, LR 10.7 | `...move p` drops a linear field the target lacks | give the target the field, or consume it first | [09](09-classes.md) |
| E-OBL-008 | LANG-11-09 | LR 11.8 | an `after` hook that may fail when `R` is linear | do not fail in `after`, or return a non-linear type | [11](11-effects.md) |
| E-ERR-001 | LANG-10-01 | LR 5.4, LR 10.2 | `fail` outside a `try` function | declare the function `try` | [05](05-statements.md), [10](10-outcomes.md) |

## Effect (E-EFF)

| Code | Rule | LR | Trigger | Correction | Pairs |
|---|---|---|---|---|---|
| E-EFF-001 | LANG-11-01 | LR 11.1, LR 11.2 | the summary exceeds the claims: an effect in `pure`, an allocation in `noalloc`, `suspend` without `async`, `fail` without `try`, `recovers` without `recovers` (the record names the claim's rule) | remove the effect, or write the modifier the summary needs; for a `noalloc` caller of an operation taking `pure fn`, replace the callback traversal with a `for` loop (LR 11.3, X-610) | [11](11-effects.md), [C](appendix-c-lowering.md) |
| E-EFF-010 | LANG-11-07 | LR 11.6, LR 4.6 | `io` through a capability with no `Authority` parameter; `Host.capabilities()` outside `main`, twice, or in a `main` that takes capability parameters | take the capability as a parameter; give `main` the capabilities as parameters (`func main(console: Console)`) instead of calling `Host.capabilities()` | [11](11-effects.md) |
| E-EFF-020 | LANG-11-01 | LR 11.1 | the `block` effect in an `async` function or anything it calls | move the work to `launchBlocking` | [12](12-async.md), [11](11-effects.md) |
| E-EFF-030 | LANG-11-03 | LR 11.4 | the telemetry plan attaches an allocating record to a `noalloc` function (at the policy site) | change the plan, not the function | [11](11-effects.md) |
| E-EFF-040 | LANG-11-05 | LR 11.3 | warning from `fortis check --api-baseline OLD`: an exported function lost a written `pure`/`noalloc` or gained `async`/`try`/`recovers` | restore the claim, or make the API change deliberately | [11](11-effects.md) |
| E-EFF-050 | LANG-11-06 | LR 11.5 | a contradictory contract: `pure` with `requires capability`; `async`, `try` or `recovers` beside `forbids` of the effect it permits; one authority both required and excluded; in one contract, or between a function's written modifiers and a contract it carries, or two contracts it carries | drop the modifier, the `forbids` or one of the capability clauses; drop the capability from a pure contract, or `pure` from a function that needs the capability | [11](11-effects.md) |

## Refinement unproved (E-REF)

| Code | Rule | LR | Trigger | Correction | Pairs |
|---|---|---|---|---|---|
| E-REF-001 | LANG-06-02 | LR 3.5, LR 6.3, LR 6.9, LR 14 | a value (or literal) whose interval lies outside the target's refinement, or is not proved within it: an argument, an initialiser, a constant's value, the argument of a newtype's `Name(x)`, a `cases` row literal where the body passes it, a refined type argument (`List[Int]` where `List[Int<0..5>]` is expected) | `x as T or fail`, or a wider declared type; for a container, validate element by element into the refined type | [06](06-types.md), [proofs](06-proofs.md) |
| E-REF-002 | LANG-06-02 | LR 6.4, LR 6.3  | an integer operator the interval rules cannot prove (overflow, division by zero, shift range, `MIN / -1`) outside a checked region; on a sized integer type (`Int32`, `UInt8`, …) or `>>`, which have no checked operation, under `or fail` too | on `Int`, the checked operator, `a + b or fail`; on any integer type, a refined type or a test that proves it | [06](06-types.md), [proofs](06-proofs.md) |
| E-REF-003 | LANG-06-02 | LR 6.3 | an `editable` loan or mutation of a length-refined collection | mutate an unrefined collection, then convert with `as` | [06](06-types.md) |

## Patterns (E-MAT)

| Code | Rule | LR | Trigger | Correction | Pairs |
|---|---|---|---|---|---|
| E-MAT-001 | LANG-09-05 | LR 9.8 | a `match` over a closed enum missing variants (each named) | add the arms | [09](09-classes.md) |
| E-MAT-002 | LANG-09-05 | LR 9.8 | `otherwise`, or a last `_` or binding arm, over a closed enum or optional | list the variants | [09](09-classes.md) |
| E-MAT-003 | LANG-09-05 | LR 9.8 | an or-pattern alternative that binds a name or is `otherwise`, or an or-pattern arm with a `when` guard | give the binding or guarded case an arm of its own; `_` for an unused payload | [09](09-classes.md) |
| E-MAT-004 | LANG-05-01 | LR 5.1, LR 9.8 | a `let` pattern that can fail to match: a variant, a literal, `null`, `present(…)`, a range pattern, at any depth (a `let` destructures a class, tuple or record) | use `match` to handle the other cases | [05](05-statements.md) |
| E-MAT-005 | LANG-09-05 | LR 9.8 | a range pattern that is not well formed: an empty or inverted interval, a bound that does not fold at compile time, or a bound outside the scrutinee integer type's own range (X-517) | write a non-empty interval with literal or `const` bounds inside the type's range | [09](09-classes.md) |

## Closures (E-FUN)

| Code | Rule | LR | Trigger | Correction | Pairs |
|---|---|---|---|---|---|
| E-FUN-010 | LANG-08-02 | LR 8.2 | a lambda body needing a stronger kind than declared (`fn` that mutates a capture) | declare `fn mut` or `fn once` | [08](08-functions.md) |
| E-FUN-011 | LANG-08-03 | LR 8.3 | a name used in a lambda body but not in its capture list | add it to the capture list with a marker | [08](08-functions.md) |

## Defer and async (E-DEF, E-ASY)

| Code | Rule | LR | Trigger | Correction | Pairs |
|---|---|---|---|---|---|
| E-DEF-001 | LANG-12-01 | LR 12.1 | `return`, `fail`, `give`, `break`, `continue` or `await` inside `defer` | move it out; consume failures with `or recover`/`catch`/`report` | [12](12-async.md) |
| E-DEF-002 | LANG-12-01 | LR 12.1 | a place the `defer` names is moved out before an exit | keep it owned until every exit (lend instead of moving), or do the deferred work before the move | [12](12-async.md) |
| E-ASY-001 | LANG-12-04 | LR 12.3, LR 12.5 | a `Task` leaves its scope block (`return`, `give`, assignment outside) | `await` it inside the scope, or `detach` it to a `Supervisor` | [12](12-async.md) |
| E-ASY-002 | LANG-12-06 | LR 12.3, LR 12.7 | a `MutexGuard` held across `await` | end the guard's block before the `await` | [12](12-async.md) |
| E-ASY-003 | LANG-12-05 | LR 12.7, LR C.2 | a pool launch whose future or closure holds a loan or a non-`Shareable` value | pass owned `Shareable` data | [12](12-async.md), [C](appendix-c-lowering.md) |

## Observation (E-OBS)

| Code | Rule | LR | Trigger | Correction | Pairs |
|---|---|---|---|---|---|
| E-OBS-001 | LANG-13-02 | LR 13 | a required frame field unassigned on an applicable exit | `capture` it before that exit; changing the schema's `required:` is a change to the schema, never a correction | [13](13-observation.md) |
| E-OBS-010 | LANG-13-02 | LR 13 | a second `capture` of one field on one path | capture once | [13](13-observation.md) |
| E-OBS-011 | LANG-13-02 | LR 13 | a `capture` inside a loop | use `record` for repeated measurements | [13](13-observation.md) |
| E-OBS-012 | LANG-13-01 | LR 13 | a `capture`/`record` expression with an effect, or naming no field of the attached schema | a pure, infallible expression; a declared field | [13](13-observation.md) |

## Resource limits and internal defects (E-LIM, E-INT)

These have no automatic correction.

| Code | Rule | LR | Trigger | What to do | Pairs |
|---|---|---|---|---|---|
| E-LIM-001 | PROG-SEC-01 | LR 6.10 | a native build's instantiation budget (32) exhausted: a generic function instantiating itself at ever larger types | recurse through a non-generic helper, or a `dynamic` value | [06](06-types.md) |
| E-LIM-002 | PROG-SEC-01 | LR 3.1 | a source file over 16 MiB or 2,000,000 tokens, or nested more than 256 deep | split the file; flatten the nesting | [03](03-lexical.md) |
| E-LIM-003 | PROG-SEC-01 | LR 4.1 | a manifest over 1 MiB or 10,000 lines | shrink the manifest | [04](04-modules.md) |
| E-LIM-004 | PROG-SEC-01 | LR 4.1 | a lockfile over 8 MiB or 100,000 lines | shrink the dependency set | [04](04-modules.md) |
| E-LIM-005 | PROG-SEC-01 | LR 4.1 | JSON the toolchain reads (caches, saved fuzz inputs) over 256 MiB or nested more than 128 deep | remove the input | [04](04-modules.md) |
| E-INT-001 | PROG-CMP-05 | LR 4.1 | a checker defect; `reproduction` names a minimised input and the command | report it with the reproduction; never work around it silently | [04](04-modules.md) |
| E-INT-002 | PROG-CMP-05 | LR 4.1 | a native build whose generated Rust does not compile, at the Fortis function it came from | report it with the function it names | [04](04-modules.md) |
| E-INT-003 | PROG-CMP-05 | LR 4.1 | a native build cannot run a tool it needs (cargo, rustc, the linker driver) | install the named tool | [04](04-modules.md) |

## Kit codes

The kits name their own codes in the Platform Specification; the records have the same shape.

| Prefix | Class | Codes | Source |
|---|---|---|---|
| E-DI | Dependency injection (mandatory `consumer`, `slot`, `candidates`) | E-DI-001 … E-DI-006, E-DI-008 … E-DI-011 | Platform 3 |
| E-TEL | Telemetry (mandatory `plan_line`, `projection`, `field`) | E-TEL-001 … E-TEL-007 | Platform 6.4 |
