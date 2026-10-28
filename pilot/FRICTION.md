# Friction log: the `ledger` documentation pilot (PROG-AGT-08)

Written while building `pilot/ledger` from README.md, `docs/guide/`, `docs/agent/`, `skills/fortis-code/SKILL.md`
and the library declarations (`bin/fortis view --decls library/fortis`). A toolchain defect that a small
program shows has a minimised program in `pilot/repro/`, whose header comment gives the exact commands and output;
the others name the commands that show them (see `pilot/README.md`).

Toolchain state: worktree at commit 5f80b6f (`DEL-49 ...`), `bin/fortis` (reference checker and interpreter) and
the prebuilt `compiler/target/release/fortisc` (production checker, VM, native backend), not rebuilt.

## Counts by kind

| Kind | Entries | Count |
|---|---|---|
| Tool defect | 6, 7, 9, 11, 12, 13, 14, 20, 23, 25, 27, 28, 29, 34, 36, 37, 40, 43, 44, 45, 46, 47, 48, 49, 50, 52, 53 | 27 |
| Documentation gap | 1, 2, 3, 16, 17, 18, 21, 26, 31, 41 | 10 |
| Documentation error | 4, 8, 30, 51 | 4 |
| Language question | 10, 19, 24, 39 | 4 |
| Worked well | 5, 15, 22, 32, 35, 38, 42 | 7 |
| Process note (mine) | 33 | 1 |

Entries 12 and 47 are also documentation errors (a guide example and a documented warning that the tools
contradict); they are counted once, as tool defects.

## The five that mattered most

1. **39**: a package's `main` can never be `recovers`, and `try recovers` is refused everywhere, so a program
   whose entry reaches any function that recovers a failure cannot be a package with an exit status. It forced a
   redesign of the parser (a failure sink), and nothing in the docs warns about it.
2. **27, 28, 37, 48**: what a module may do with a type depends on which *other* types it happens to import
   (copyability, VM method lookup, native `Clone`), and checking one file of a package gives different findings
   from checking the package.
3. **12, 13**: `derive Comparable` (`<`) and hand-written `Displayable` pass both checkers and fail at run time
   on both backends; one failing program is a guide example the README says is run.
4. **29, 34, 36**: the VM refuses whole packages that both checkers accept ("the typed IR is invalid", "not
   built"). Each time, every test of the package failed to run until the construct was rewritten.
5. **3**: the standard library's text API is undocumented and minimal (no line splitting, trimming, searching,
   integer or decimal parsing; `Decimal` cannot be built from text); about 400 lines of the application are
   scanning helpers I had to write after reading the library's declarations.

## Entries

### 1. Passing arguments to `fortis run` (documentation gap)
- **Trying to:** run `ledger balance` and `ledger register assets:cash`.
- **Docs:** README "Building the tools and running a program" table, guide/tour.md "Getting started" table and
  agent/README.md "Tools" table give `fortis run FILE.ft|DIR` with no arguments; the tour's hello world prints
  "0 arguments" and never shows how to pass one.
- **Happened:** only `bin/fortis --help` says `fortis run FILE.ft [args...]`. `bin/fortis run pilot/ledger one two`
  works and `main` sees `["one", "two"]`. Nothing documents how `fortisc vm` takes arguments.
- **Change:** add `[ARGS…]` to the three tables and one example with an argument.

### 2. Where a program's input files live (documentation gap)
- **Trying to:** give the program a sample journal to read through `FileSystem`.
- **Docs:** guide/15-runtime.md "The development adapters" and agent/15-runtime.md: "the file system is rooted
  in a fresh empty directory for each run (or `FORTIS_DEV_ROOT`)". Nothing says where a package keeps data files,
  whether a path is relative to the package, or that `FORTIS_DEV_ROOT` is resolved against the working directory.
- **Happened:** without the variable, `bin/fortis run pilot/ledger` printed `failure fs.not_found in main (path:
  journal.ledger)` and exited 1. With `FORTIS_DEV_ROOT=pilot/ledger/data` (relative to the current directory) it
  worked on the interpreter and on the VM. I put the journal in `pilot/ledger/data/`, a convention I invented.
- **Change:** a sentence and an example in the tour or 04-modules ("data files: put them in DIR, run with
  `FORTIS_DEV_ROOT=DIR`"), and say whether the native binary uses the real file system.

### 3. The standard library's text and collection API is not documented, and is minimal (documentation gap)
- **Trying to:** split a file into lines, trim, split on `:`, find a substring, parse integers and decimal amounts,
  iterate and update a `Map`.
- **Docs:** agent/06-types.md "Facts used when writing code" lists `byteCount`, `scalarCount`, `slice`,
  `scalarAt`, `String.from` and `text.Builder`; the guide's "Text and views" and "Collections" show a few more.
  No page lists what `String`, `StringView`, `Map`, `Set` or `List` offer.
- **Happened:** I needed `bin/fortis view --decls library/fortis` (and `--symbol String`, `Map`, `Scalar`,
  `Currency`, `Money`, `List`, `abs`, `sorted`). What exists: `String` has only `from`, `empty`, `scalarCount`,
  `byteCount`; `slice`/`scalarAt` exist only on `StringView` (a `String` argument converts to a view
  parameter, but `myString.slice(…)` does not exist); `Text.words` is the only splitter; there is no `lines`,
  `trim`, `startsWith`, `contains`, `indexOf`, `toLower`, integer parser, `Decimal` parser, `Map.remove`,
  `Map.contains` or `List.sort` (only `fortis.collections.sorted(ListView[Int])`). `scalarAt` is O(n), so walking a
  text scalar by scalar is quadratic. Every text operation is `try`. `Decimal` cannot be built from text or from an
  `Int`, which is why the ledger holds amounts as integer minor units instead of `Decimal`/`fortis.money`.
- **Workaround:** `src/scan.ft` (lines, trim, prefix/suffix, find, split, integer parsing, padding), the amount
  parser in `src/amount.ft`, a generic merge sort in `src/order.ft`.
- **Change:** a library reference page per standard module generated from the declarations (they already carry
  good contracts, e.g. "fails `text.slice_bounds` unless 0 <= start <= end <= scalarCount; O(n) in bytes"), linked
  from the agent README; and, in the library, `lines`, `trim`, `split`, `find`, `Int.parse`, `Decimal.parse`.

### 4. `fortis.text` and `fortis.collections` are described with contents they do not have (documentation error)
- **Docs:** guide/15-runtime.md "The standard modules": `fortis.text` provides "`Builder`, `Scalar`, grapheme
  counts, normalisation and case mapping (LR 6.6)"; `fortis.collections` "collection algorithms such as `sorted`".
- **Happened:** `bin/fortis view --decls library/fortis`: `text.ft` declares `Builder`, `Scalar`, `graphemes`
  and `Text.words`; no normalisation, no case mapping. `collections.ft` declares one function,
  `sorted(items: ListView[Int])`.
- **Change:** describe what is there, or mark the rest as planned.

### 5. The diagnostics and their corrections (worked well)
- `E-SYN-002` named the keyword I had used as a name (`owned`, `before`, `once`, `from`, `error` as a table
  field); `E-OWN-001` told me exactly to write `alias ownedText` at a call whose result borrows; `E-EFF-001` named
  the function that needed `recovers`; `E-REF-002` named the checked alternative (`use Int.sub`); `E-TYP-160`
  said to annotate the receiving binding. The `fields` (`proved_interval`, `required_range`) made the interval
  checker understandable without the (stub) proofs page. Every finding came back as a record in under a second.

### 6. A generic bound is lost for values read from a local `List[T]` (tool defect)
- **Trying to:** write a generic insertion sort `where T implements Comparable, T implements Copyable`.
- **Docs:** guide/06-types.md "Generics": "A generic body is checked once, against its bounds".
- **Happened:** both checkers: `E-TYP-040 "`>` needs Comparable; T does not implement it"` and
  `E-OWN-002 "`copy b`: T is not Copyable"` for values read from `let mut out: List[T] = …`; the same operations on
  values read from the parameter are accepted.
- **Workaround:** `src/order.ft` sorts a list of indices and reads every element from the parameter.
- **Repro:** `pilot/repro/generic-bound-annotated-local.ft`.

### 7. A `List[Int]` argument does not fix `T` of a `ListView[T]` parameter (tool defect)
- **Docs:** guide/06-types.md "Generics": type arguments "are inferred from the arguments, or from the type the
  call's result is expected to have".
- **Happened:** both checkers: `E-TYP-160 "the type arguments of this call are not fixed by its arguments;
  annotate the receiving binding"` for `let d = copied(source)` with `source: List[Int]`.
- **Workaround:** annotate the binding (`let d: List[Int] = …`) at every call of `sortedBy`.
- **Repro:** `pilot/repro/listview-inference.ft`.

### 8. A failure's message cannot be reached by the program, and is not rendered in records (documentation error)
- **Trying to:** print a failure's message in a diagnostic.
- **Docs:** guide/10-outcomes.md "Declaring failures": the declaration gives "a display template", "rendered when
  the failure is displayed".
- **Happened:** `"${e}"` is `E-TYP-040 "interpolated `e` has type Failure, which is not Displayable"`, and
  `Failure`'s only declared member is `code` (`view --symbol Failure`). In records the template is not rendered
  (entry 40).
- **Workaround:** the parser builds its own messages from `is Error(field: …)` payloads (`src/parser.ft`,
  `dateRefusal` and friends).
- **Change:** say how a program displays a failure (or that it cannot), and render the template in records.

### 9. `validate` into String-based types fails at run time, differently on each backend (tool defect)
- **Trying to:** put an offending word into an error payload (payload strings must be length-bounded), and
  declare `type CommodityCode = …` over a String.
- **Docs:** agent/06-types.md "Facts": refinement forms include `String<scalars: 1..120>`; "`T.validate(x)` …
  return `Outcome[T]`".
- **Happened:** both checkers accept. `bin/fortis run`: `dynamic check failed: validate of a non-integer`, exit 2,
  no record, nothing printed, for `String<scalars: 0..40>.validate(…)`. `fortisc vm`: `observed: dynamic:validate
  of a non-integer`, exit 0, for a newtype over an unrefined `String`. A newtype over a *refined* String works on
  both, which is what the ledger uses (`CommodityCode = String<scalars: 1..10>`).
- **Workaround:** no String payloads in errors; positions and enums only.
- **Repro:** `pilot/repro/string-refinement-validate.ft`.

### 10. How does a `pure` function discharge a failure it does not recognise? (language question)
- **Trying to:** map a failure to a diagnostic in a `pure` parser: `match move f { is DateSyntax => …,
  otherwise(other) => ? }`.
- **Docs:** agent/10-outcomes.md: `otherwise(name)` "is required (open type)" and must be consumed; the examples
  consume it with `report` (not allowed in `pure`) or `fail`.
- **Happened:** `E-OBL-001 "the failure `other` bound by this arm is not consumed"`. The only discharge I found
  in pure code is `let lost: Outcome[T] = Outcome.failure(move other)` then `move lost or recover v`, an obvious
  trick. In the end the parser does not discharge at all (entries 39, 41).
- **Change:** document the intended idiom, or provide `Failures.discharge(owned Failure)` marked `recovers`.

### 11. `fortis test FILE.ft` on a module of a package cannot run its imports (tool defect)
- **Docs:** README table: "run its tests (a file or a package directory)"; agent/README: `fortis test
  FILE.ft|DIR`.
- **Happened:** `bin/fortis test pilot/ledger/src/date.ft` checked cleanly, then every test using an imported
  function failed: `ERROR parse good dates / new year: expected a well-formed run, actual `twoDigits` is not
  declared (line 0)`; `fortisc test` on the file: `pilot/ledger/src/date.ft:74: not built: `twoDigits` is not a
  local, a constant or a function`. Testing the package directory works.
- **Change:** run a module's tests with its package (or refuse with a record saying "test the package").

### 12. `<` on a derived `Comparable` fails at run time, including the guide's own example (tool defect)
- **Docs:** guide/06-types.md "The built-in interfaces and `derive`": example with `enum Size derive Equatable,
  Comparable, Displayable` printing `Medium; small before large: true`; README: "every example is run".
- **Happened:** the example copied verbatim (`pilot/repro/guide-derive-comparable.ft`): both checkers accept;
  `bin/fortis run` prints `dynamic check failed: `<` on non-comparable values`, exit 2, and nothing else (the
  line printed before the comparison is lost); `fortisc vm`: `observed: dynamic:`<` on non-comparable values`,
  exit 0. For a class with `derive Comparable` the VM also has no `compare` method ("unbuilt:line 40: no method
  `compare` on Pair"). `Date` in the ledger (a `trivial class derive Comparable`) failed every ordering test on
  both backends.
- **Workaround:** `Date.isBefore`/`compareTo` and `classRank` written by hand.
- **Repro:** `pilot/repro/guide-derive-comparable.ft`.

### 13. A hand-written `Displayable` is ignored by interpolation (tool defect)
- **Docs:** agent/09-classes.md "Facts": implementing is `implements I` with every member or `implement I for T
  { … }`; guide/06-types.md: `Displayable` gives `${value}`.
- **Happened:** both checkers accept; `"${n}"` prints `<display>` (interpreter, `implements` form),
  `Tagged(tag: y)` (interpreter, `implement … for` form), and `Named(name: x)`/`Tagged(tag: y)` on the VM,
  instead of the text the `display` method appends.
- **Workaround:** `date.iso()` called explicitly.
- **Repro:** `pilot/repro/handwritten-displayable.ft`.

### 14. Test records differ between the backends (tool defect)
- **Docs:** agent/README "Tools": `fortisc test … --format json` gives "the same records" as `fortis test`;
  agent/14-tests.md: replay a failing property "with `fortisc test`".
- **Happened:** the interpreter runs 25 cases per property and writes one `test-result` per case ("case 25"); the
  VM runs 100 and writes one result ("100 cases", with the seed). The same package counts 673 on the interpreter
  and 241 on the VM. The interpreter's `test-summary` has no `rule` field (`{"kind": "test-summary", "passed": 108,
  "failed": 0}`), the VM's has `"rule": "PROG-TST-01"`. The VM's failing-property record prints `replay: fortis
  test pilot/ledger --test … --seed …`, a command whose options `fortis test` does not document.
- **Change:** one record shape and one case count, or document the difference.

### 15. The interval checker proved real arithmetic (worked well, with a gap)
- The day-number conversions in `src/date.ft` (`(153 * monthFromMarch + 2) / 5 + self.day - 1`, `era * 146097 +
  dayOfEra`, the inverse with five divisions) were accepted with plain operators, because the operands are
  refined (`Int<1..9999>`, `Int<1..12>`). So were `a + b == 0` on refined property inputs and `n + 1` passed to an
  `Int<306..3652364>` parameter.
- **Gap:** agent/06-proofs.md and guide/06-proofs.md are stubs ("Do not infer the proof rules from this page"); I
  learnt by trial what is proved (and see entry 20).

### 16. A newtype's representation cannot be read back (documentation gap)
- **Trying to:** count lines with `type LineNo = Int<1..>`.
- **Docs:** guide/06-types.md "Tuples, records, variant maps and newtypes": "a new nominal type with a private
  representation", made by `validate` or a proved literal; nothing on getting the value out.
- **Happened:** `let mut number: LineNo = 1` is `E-TYP-040 "Int<exact: 1..1> is not LineNo"`;
  `Int.add(number, 1)` is `E-TYP-040 "LineNo is not Int"`. No accessor is documented.
- **Workaround:** keep an `Int` counter and `LineNo.validate(count)` per line; to get a `String` back from
  `CommodityCode`, interpolate it (`"${code}"`).
- **Change:** say explicitly that the representation is unreachable (and why), or document the accessor.

### 17. Common words are reserved (documentation gap)
- **Happened:** `E-SYN-002` for `owned`, `before`, `after`, `once`, `from`, and a table-test field `error:`.
  agent/appendix-a-keywords.md lists them all, but the tour and the "write your first program" path never point
  to it, and `before`/`after`/`from`/`record`/`view`/`state`/`model` are names every program wants.
- **Change:** a line in the tour: "these everyday words are keywords: …".

### 18. Literal lists are `List[StringView]`, and refined element types do not widen (documentation gap)
- **Happened:** `sortedStrings(List.of("pear", "apple"))` is `E-TYP-040 "List[StringView] is not
  ListView[String]"`; a property's `List[Int<-50..50>]` is `E-TYP-040 "List[Int<-50..50>] is not ListView[Int]"`.
- **Workaround:** `let fruit: List[String] = List.of(…)`; `items.map(fn(x: Int<-50..50>) Int => x)`.
- **Change:** say in guide/06-types.md that a refinement is a subtype only at the top level, not inside a
  collection, and what type `List.of("…")` has.

### 19. A tuple element is not a place; `for` does not destructure (language question)
- **Trying to:** `for entry in map { editable out.push(copy entry.0) }`.
- **Happened:** `copy entry.0` is `E-SYN-000 "unexpected INT '0' … expected one of: keywordName, lowerIdent"`;
  `for (key, value) in m` is `E-SYN-000`. `match entry { (key, value) => … }` works.
- **Docs:** agent/07-ownership.md "Places: a binding, a field path from a binding or `self`, an `Array` element at
  a literal index"; guide/06-types.md iterates a map with `entry.0` but only interpolates it.
- **Workaround:** `Holdings` and `Commodities` keep their keys in a list beside the map.

### 20. Comparisons narrow in `if` statements but not in `if` expressions (tool defect)
- **Happened:** `let d = n % 10; return if d < 0 { give 0 - d } else { give d }` into `Int<0..9>` is
  `E-REF-001 "returned value has interval [-9, 9]"` on both checkers; the statement form with `let mut digit` is
  accepted. Narrowing an optional in an `if` expression fails the same way (`span = if range == null { … } else {
  "${range.first…}" }` is `E-TYP-040 "member `first` of an optional Period?"`).
- **Repro:** `pilot/repro/if-expression-narrowing.ft`.

### 21. The `Void` value (documentation gap)
- **Trying to:** handle a failing `Void` call with `catch`.
- **Happened:** `recover {}` is a syntax error. The spelling `recover Void` appears only in guide/12-async.md and
  agent/12-async.md, inside a `defer`.
- **Change:** show `catch e { …; recover Void }` in 10-outcomes.

### 22. Destructuring and enum payloads (worked well)
- `let Parser(commodities: c, accounts: a, accountTexts: _, …) = move parser`, `match move verdict {
  Valid(transaction: t) => …, Invalid(line: at, problem: p, detail: d) => … }`, `owned self` receivers and
  class payloads in enums worked exactly as agent/09-classes.md describes, on all backends.

### 23. `if x == null { return }` does not narrow an optional *loan* for `copy` (tool defect)
- **Docs:** guide/06-types.md "Optional values": after `if n == null { return … }` the place "is a `T` for the
  rest of the block".
- **Happened:** for `let last = items.get(0)` both checkers say `E-OWN-002 "`copy last`: alias String from self?
  is not Copyable"` (the message also names `self`, not `items`). `match … { present(x) => copy x … }` works.
- **Repro:** `pilot/repro/narrowed-optional-loan-copy.ft`.

### 24. Fallible text operations and comparators (language question)
- Every text operation (`scalarAt`, `slice`) is `try`, so a comparator that looks at text must either propagate
  (impossible for a `pure fn(T, T) Bool`) or recover, and `recovers` then spreads to every caller up to `main`,
  which may not have it (entry 39). I precomputed sort keys instead (`classRank`, segment lists). A `ListView`
  index or a total `firstScalar` would have avoided it.

### 25. A call-site `try` gets an unhelpful message (tool defect)
- **Happened:** out of habit I wrote `try checkSegment(piece, position) or fail`; the record is `E-SYN-000
  "unexpected LOWER 'checkSegment' at 53:11; expected one of: 'fn', 'recovers'"`. agent/01-overview.md lists
  "postfix `?`", `::`, `new` and others as "Removed, never write" with corrections, but not a call-site `try`
  (guide/10-outcomes.md says "There is no `?`, no `try` at the call site").
- **Change:** a dedicated message ("a call is not marked `try`; write `f() or fail`") like the `;` one.

### 26. Testing a failure's payload is awkward (documentation gap)
- **Trying to:** check `BadAccountName(problem: …, position: …)` in a table test.
- **Happened:** `match outcome { failure(f) => match move f { is BadAccountName(…) => { expect … } … } }` gave
  `E-OWN-001` (`match outcome` must be `match move`), `E-TYP-110 "match arm block in expression position does not
  end in `give`"` and `E-OBL-005 "the linear value in `outcome` is live across `fail`"`.
- **Workaround:** a helper `refusalOf(text) Refusal?` (in `src/account.ft`) that returns the payload as a value.
- **Change:** a "testing a failure's payload" pattern in 14-tests (only `e.code` is shown).

### 27. Copyability depends on which types the using module imports (tool defect)
- **Happened:** `AccountName` (fields `String`, `List[String]`, `AccountClass`) was `E-OWN-002 "AccountName is not
  Copyable (6.11)"` in every module that did not also import the enum `AccountClass`, and so were `List[PostingDraft]`
  and everything holding an account. Importing `AccountClass` (unused) makes the same lines accepted. Both
  checkers agree.
- **Docs:** guide/06-types.md "Trivial and copyable types": classes "whose fields are all copyable" are copyable.
- **Workaround:** `import pilot.ledger.account { AccountClass, … }` with a comment in seven modules.
- **Repro:** `pilot/repro/copy-imported-optional/src/outside.ft`.

### 28. Checking one file of a package disagrees with checking the package (tool defect)
- **Happened:** `bin/fortis check pilot/repro/copy-imported-optional/src/user.ft` reports `E-TYP-090` and two
  `E-OWN-002` that the package check does not; for `outside.ft` it is the other way round; and the single-file
  check accepts an import of a member the module does not export (`import repro.copyopt.sealed { Tagged }` when
  there was no `Tagged`).
- **Docs:** README and agent/README: `fortis check FILE.ft|DIR`, with no warning that they differ.
- **Repro:** `pilot/repro/copy-imported-optional/src/user.ft` and `outside.ft`.

### 29. The VM refuses a move in a returning branch followed by a later consumption (tool defect)
- **Happened:** both checkers and the interpreter accept; `fortisc test pilot/ledger` printed only
  `pilot/ledger: the typed IR is invalid (C1.move pilot.ledger.parser::function placesRefusal:390 `other` is
  used after it was moved): a compiler defect`, so none of the package's tests ran. `fortisc check` reports
  nothing. Rewriting as `if … { consume } else { consume }` is accepted.
- **Repro:** `pilot/repro/move-in-returning-branch.ft`.

### 30. An error constructor does not produce a `Failure` (documentation error)
- **Docs:** guide/10-outcomes.md "Declaring failures": "a constructor, `Name(field: …)`, that produces a
  `Failure`"; agent/10-outcomes.md the same.
- **Happened:** `annotate(UsageError(…), "…")` and `keep(Odd())` (parameter `f: owned Failure`) are `E-TYP-040
  "UsageError is not Failure"` / `"Odd is not Failure"`. `fail Odd()` and `Outcome.failure(Missing(…))` work.
- **Change:** say what type `Name(…)` has and where it converts to `Failure`.

### 31. Library failures cannot be matched by kind (documentation gap)
- **Happened:** `numeric.overflow`, `refinement.out_of_range`, `text.index` and the scanner's own codes reach the
  parser, but there is no declared error to write `is …` for the library's codes. I compare `f.code ==
  "numeric.overflow"` (`unexpectedRefusal` in `src/parser.ft`). The docs give the codes (agent/06-types.md) but
  never how to match them.

### 32. Raw multi-line strings as test fixtures (worked well)
- `const sample = """commodity GBP 2 … """` spanning many lines worked as a module constant on every backend; the
  docs only say `"""raw"""` is a `StringView from static` with no escapes.

### 33. My own slips (process note)
- Before settling into the skill's rules I read four of my own `.ft` files with the Read tool or `head` (scan.ft,
  date.ft, one repro); afterwards I used `bin/fortis view` only. I also wrote several report expectations by hand
  with wrong column counts and a wrong posting count; I checked the program's output line by line against the
  journal's arithmetic before replacing those expectations (`src/reports.ft`).

### 34. The VM refuses a `catch` that returns from inside a `for` loop (tool defect)
- **Happened:** `fortisc test pilot/ledger`: `the typed IR is invalid (C1.move pilot.ledger.validate::function
  balance:88 `e` is used after it was moved): a compiler defect`, again for the whole package; the interpreter
  passes and the checkers accept.
- **Workaround:** the summation moved into its own function (`writtenSum` in `src/validate.ft`).
- **Repro:** `pilot/repro/catch-in-loop.ft`.

### 35. Properties over refined types (worked well)
- `property "…" forall (minor: Int<-100000000000000..100000000000000>, scale: Int<0..8>)` drew in-range inputs;
  when a property failed early on, the VM printed the shrunk input (`minimal failing input … {"d1": 1, …}`) and a
  replay command. 18 properties cover parsing, formatting, arithmetic, dates, sorting and roll-ups.

### 36. `p == null || p.m()` does not build on the VM (tool defect)
- **Happened:** both checkers accept and the interpreter runs it; `fortisc test pilot/ledger`:
  `pilot/ledger:87: not built: no method `holds` on Period?`.
- **Docs:** narrowing is documented "after `x != null`, after `if x == null { return … }`, and in `x != null &&
  e`" (guide/06-types.md); the `||` form is not listed, so either the checkers accept too much or the VM too
  little.
- **Workaround:** `let excluded = period != null && !period.holds(t.date)` (`src/books.ft`).
- **Repro:** `pilot/repro/narrowing-in-or.ft`.

### 37. The VM cannot call a method on a field whose class the module does not import (tool defect)
- **Happened:** `loaded.diagnostics.count()` in a test of `books.ft`: `ERROR balances per account: expected a built
  test, actual unbuilt: line 138: no method `count` on <unknown>`. Importing the holder class changes it to "no
  method `count` on Counter"; importing both builds it. The checkers and the interpreter are fine.
- **Workaround:** extra imports in `src/books.ft`.
- **Repro:** `pilot/repro/unimported-member-type/`.

### 38. The two backends agree on the whole program (worked well)
- The sample run prints 128 lines; the interpreter's output and the VM's `observed:` line are identical, and both
  write the same two failure records.

### 39. A package's `main` cannot recover a failure, ever (language question, with a tool inconsistency)
- **Trying to:** keep the parser `pure`; it turns failures (a bad date, amount, account) into diagnostics, which
  is the `recovers` effect, so `loadJournal` is `recovers` and so is its caller, `main`.
- **Docs:** agent/04-modules.md LANG-04-06: "the exported `main` … with one of the four signatures of LR 4.6"
  (`try`, `async` or `async try`); but guide/06-types.md "Integer arithmetic" and guide/10-outcomes.md use
  `export recovers func main` in accepted examples.
- **Happened:** `try recovers func main` in the package: `E-NAM-001 "main has signature `try recovers func main(
  args : List [ String ] ) Void`, not one of the four forms of 4.6"`. `recovers func main` in the package: the same
  record. The same `recovers func main` in a single file is accepted and runs. Since `recovers` is transitive, no
  package whose entry reaches any recovering function can be built, and none can have both a recovered failure and
  an exit status.
- **Workaround:** a redesign: the parser does not discharge failures; it moves each one, annotated with its
  line, into a sink (`sink: editable List[Failure]`) that `main` reports (entry 41). `main` is `try` again and
  `ledger check` exits 1 on errors.
- **Change:** allow `recovers` with each of the four forms (it says nothing a caller depends on), or reject it for
  single files too and say so in 04-modules; either way document the idiom of 41.
- **Repro:** `pilot/repro/recovers-main/`.

### 40. Failure records name the wrong module and do not render the message (tool defect)
- **Happened:** the record of `ledger.date.range` raised in `Date.of` (date.ft) said `"origin": {"file":
  ".../pilot/ledger/src/books.ft", "function": "of"}`: books.ft has `Balances.of`. In the repro, the first module
  (alphabetically) with a function of that name is named. The `message` is the template, quoted and unfilled:
  `"\"${year}-${month}-${day} is not a day of the calendar\""`; the VM record has the template unquoted and no
  file. agent/records.md shows `"message": ""` for a declared error, a third behaviour.
- **Repro:** `pilot/repro/failure-origin/`.

### 41. No documented idiom for "failures become data" without `recovers` (documentation gap)
- **Trying to:** report every malformed line once and keep going, in pure code, with `main` still able to fail.
- **Happened:** after entry 39 the only design I found is the sink: `refuseWith(line, refusal, move f, editable
  sink)` pushes `annotate(move f, "journal line N")`; `main` calls `reportAll(move sink)` (`consumeEach` with
  `report f`) before it looks at the outcome, so no failure is live on any failing path (E-OBL-005). Tests use
  `parsedForTest`/`loadedForTest`, which settle the sink first. The annotation appears in JSON
  (`"annotations": ["journal line 50"]`) but not in the text rendering (`failure ledger.date.range in of (year:
  2026, month: 13, day: 2): "…"`).
- **Change:** a worked example of this pattern (it is the general shape of every parser and validator), and the
  annotations in the text rendering.

### 42. Capabilities, `try main` and the exit status (worked well)
- Passing `caps.fileSystem` and `caps.console` down made every I/O boundary visible in signatures; `readJournal`
  is the only function that touches the file system. Once `main` could be `try`, `ledger check` on a journal with
  errors printed its message, wrote exactly one record (`ledger.journal_has_errors`, `"payload": {"errors": "5"}`,
  `"reported_by": "main"`) and exited 1, exactly as LR 4.6 is described.

### 43. Native build: an enum payload field of a declared error (tool defect)
- **Happened:** `bin/fortis build`: `E-INT-002 "the Rust generated for pilot.ledger.main::function explainUsage
  does not compile (error[E0308]: mismatched types)"`; the generated Rust reads the payload with
  `__f.field("problem")` and passes it where the enum is expected. Interpreter and VM run it.
- **Repro:** `pilot/repro/native-enum-payload.ft`.

### 44. Native build: optionals narrowed by `&&` in a generic function (tool defect)
- **Happened:** `E-INT-002 "the Rust generated for pilot.ledger.order::function sortedBy does not compile
  (error[E0308]: arguments to this function are incorrect)"`; the Rust passes `&Option<&T>` to the callback. The
  same function without the type parameter builds.
- **Repro:** `pilot/repro/native-narrowed-loans.ft`.

### 45. Native build: a module `const` as an intrinsic's argument (tool defect)
- **Happened:** `E-INT-002 … reports::function balanceReport … mismatched types`; the Rust is
  `width.saturating_add(&const_pilot_ledger_reports_amountWidth())`.
- **Repro:** `pilot/repro/native-const-argument.ft`.

### 46. `fmt --write` prints inferred facts only in files it rewrites anyway (tool defect)
- **Docs:** guide/tour.md "Notation that earns its place" and agent/01-overview.md: "the formatter prints every
  inferred fact back in canonical form", e.g. the `pure noalloc` of a lambda.
- **Happened:** `bin/fortis fmt --write pilot/ledger`: "3 of 13 files reformatted"; in those three, lambdas became
  `pure fn …`/`pure noalloc fn …`; in the other ten (diagnostic.ft, books.ft, reports.ft, parser.ft, validate.ft,
  main.ft) lambdas stayed `fn …` (`fortis view --query lambda` lists them as `fn`), and `fmt --check` accepts all
  thirteen.

### 47. Unused imports are never reported (tool defect)
- **Docs:** agent/04-modules.md and guide/04-modules.md: "an unused import is a warning under the baseline lint
  level and an error under the strict one"; no option to choose a level is documented.
- **Happened:** no record from either checker for two unused imports; in the ledger, the imports added only as
  workarounds (entries 27 and 37, never named in the importing module) were never flagged either.
- **Repro:** `pilot/repro/unused-import.ft`.

### 48. Native build: the copyability defect becomes uncompilable Rust (tool defect)
- **Happened:** first native attempt: `E-INT-002 "the generated Rust does not compile (error[E0277]: the trait
  bound `TransactionDraft: Clone` is not satisfied)"`. `ParsedJournal` (parser.ft, which imported `AccountClass`)
  derived `Clone`; `TransactionDraft` (journal.ft, which did not) did not. Fixed in the ledger by the entry 27
  workaround in `journal.ft`.

### 49. Native build: stopped at the fifth defect (tool defect, not minimised)
- After working around 43, 44 and 45 in a throw-away copy, the build failed in `pilot.ledger.validate::function
  balance` (`error[E0308]: mismatched types`). I stopped there; the native build of the ledger does not succeed.
  The ledger's own sources keep the clean design (the workarounds for 43 to 45 would have degraded diagnostics).
- **Added at the documentation review (DEL-49, round 3):** the throw-away copy was not kept, but no workaround is
  needed to see this failure: rustc reports every error of the generated crate at once, so running the record's
  `rustc` command with `--error-format=short` lists it beside 43 to 45 (`expected Amount, found Option<Amount>`).
  Minimised as `pilot/repro/native-narrowed-local-move.ft`: a local optional narrowed by `!= null` and moved into
  a field of the non-optional type.

### 50. `E-INT-002` records do not include the Rust compiler's message (tool defect)
- **Docs:** agent/04-modules.md: "report it with the record's reproduction".
- **Happened:** the record says only `error[E0308]: mismatched types` and gives a `rustc` command line as the
  reproduction; finding the cause meant reading `build/native-build/…/src/main.rs` by hand.

### 51. Which command fuzzes (documentation error)
- guide/14-tests.md "Fuzzing": "`fortis test --fuzz` turns every property into a … fuzz target"; the table below
  it and agent/14-tests.md give `--fuzz` only to `fortisc test`; `bin/fortis --help` lists no `--fuzz`.

### 52. The interpreter is slow (tool observation)
- `bin/fortis test pilot/ledger`: 130 seconds for 673 results; `fortisc test pilot/ledger`: about 1 second for
  241. A sample run (`fortis run` on the 112-line journal) takes about 16 seconds on the interpreter and
  0.3 seconds on the VM. Agents should test on the VM while iterating; no page says so.

### 53. `recovers` disappears through a lambda (tool defect)
- **Happened:** `pure func viaLambda(x: Int) Int => apply(fn(n: Int) Int => half(n) or recover 0, x)` with
  `apply(f: pure fn(Int) Int, …)` is accepted by both checkers with no `recovers` on the lambda, on `viaLambda` or
  on anything above; the same discard written directly is `E-EFF-001`.
- **Docs:** agent/08-functions.md: "`try`, `async`, `recovers` must be written when the summary has them
  (functions and lambdas)"; guide/10-outcomes.md: "a signature therefore shows who swallowed a failure".
- **Note:** found while writing entry 24; it is also an (unsound) way around entry 39, which I did not use.
- **Repro:** `pilot/repro/recovers-through-lambda.ft`.
