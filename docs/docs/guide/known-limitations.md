# Known limitations of v0.1.0

<!-- covers: LR 0 -->
*Human mode. Agent mode: [Known limitations, by defect](../agent/known-limitations.md). The release note is [v0.1.0](../releases/v0.1.0.md).*

v0.1.0 is a subset of the language (the ladder in the delivery plan). These are the open defects inside that subset that do not block the release: a valid program the checkers reject, a missing operation, two tools that disagree, or a checked program one backend refuses with a clear error and a non-zero exit while another runs it (X-343). A program the checkers accept and then run wrongly is listed only when an earlier release had the same defect: v0.1.0 is no worse, and those are in a section of their own (X-416). The last section lists what v0.1.0 leaves for later by design, as its release note says.


This page, like its agent-mode twin, is generated from the defect register and the release's own list; edit those, not the page.

## Valid programs the checkers reject, and library gaps

<!-- no-citation: each row is a toolchain limitation already recorded against v0.1.0 or before; the language rules are unchanged -->

| Defect | What you meet | Workaround |
|---|---|---|
| D-550 | A user helper cannot call a plain fn mut callback parameter; the callback needs an editable parameter. | Declare the parameter editable fn mut and call it through that parameter. |
| D-51.5 | An unused import is accepted with no warning. | Delete the import. Nothing else changes. |
| D-52.1 | An error constructor is typed as its own kind, so a use that the language accepts where a failure is expected is rejected. | Match the constructor's own type, or construct the failure where its own name is expected. |
| D-54.6 | For a parameter named with a keyword, fortisc vm reports a generic syntax error and fortisc check reports the keyword-specific one. | Rename the parameter. Both tools refuse the program. |
| D-251 | A variant whose name is also a class's is rejected by both checkers, so the program does not reach the VM. List.of(Red) with the list's element type written out runs on the interpreter, the VM and native. | Write the variant with its enum prefix, or do not declare a class of the same name. The list form already runs when the expected type is written. |
| D-285 | A package whose manifest the loader refuses (conformance/packages/bad.policy) makes fortis check, run and test exit 2 with a message on standard error, and fortisc check exit 1 with a names record on project.yaml. | Both tools refuse the package. Read fortisc's record; it names the manifest. |
| D-343 | A property runs 25 generated cases with fortis test and 100 with fortisc test, from different generators, so a property that fails on few inputs can pass on one and fail on the other. | Run properties with fortisc test as well, and turn a failure it finds into an example test. |
| D-344 | Native executor tracing omits the drop event for a temporary made by a ?? fallback. The interpreter and VM trace that drop. Output, cleanup calls and log records agree on all three backends. | Compare traces on the interpreter and VM; native tracing omits the temporary fallback drop. |
| D-363 | A tuple that holds a loan, such as (xs.get(0), 1), cannot yet be reassigned to a let mut binding that holds one, or returned from a function declared to return one: both checkers reject it as a loan where an owned value is required, although the Language Reference allows both. The same with an optional, t = xs.get(1), is accepted. | Keep the loan in a binding of its own (let t = xs.get(0)) and the other values beside it, and reassign or return those separately. |
| D-365 | A defer that changes a list, such as defer { let v = xs.pop() }, is rejected while a binding of the same block holds a loan from that list inside an optional (let t = xs.get(0)), although a deferred block runs after its block's own loans have ended. | Put the loan in an inner block that ends before the defer's block does, or make the change after the block. |
| D-402 | After match move o of a binding that holds a loan (let o = xs.get(0)), the loan o held is not released, so the owner stays frozen for the rest of the block (xs.push is refused as a change under a live loan). New with this release's loan holders. | Put the match and the binding in an inner block, so the loan ends with it. |
| D-508 | A refinement failure reported directly from a test body has origin.function main on the interpreter and the test title on VM/native. This is also the v0.0.2 legacy validate path, whose interpreter validate implementation uses main when no call is on the stack. The migration does not change the discrepancy. | Call a named helper from the test when the failure origin must name a function; compare the failure code and payload, not the test-body function label. |
| D-535 | `Map` and `Set` have no `iterate()` method and no cursor type, though LR 6.7 listed `Map.iterate()`. A `for` over either works on every backend (a `Map` yields `(key, value)` tuples, which LR 6.7 now states). Split from D-58, which keeps the other items. | Use a for loop over a Map or Set; a Map loop binds a key and value tuple. |
| D-543 | `Map.remove` and `Set.remove` cost O(n) in the entries after the one removed, because the insertion order of the rest is kept by moving them up. A hash container whose removal is expected O(1) needs tombstones and compaction on all three backends. | Use removal for small collections, or rebuild the collection in one traversal when removing many entries. |
| D-546 | `SortedSet.add` and `SortedSet.remove` cost O(n) moves in the members after the one added or removed, because `List` has no insert or remove at an index and the set moves each of them with a `pop` and a `push`. Lookups are O(log n). | Use SortedSet for ordered lookup and modest updates; batch changes in a list before constructing the set. |
| D-548 | A generic class with a List[T] field is not Copyable even at a copyable element type. Copyability is judged on the declaration, as LR 6.7 now states; copy Bin[String], Deque[Int] or SortedSet[Int] is rejected. | Copy through toList(), then pass the new list to Deque.fromList or SortedSet.fromList. |
| D-590 | Outside hash-container key validation, both checkers accept a tuple as a Hashable type argument, although LR 6.9 says tuples and anonymous records do not satisfy interfaces. The D-549 key check rejects these structural key types. | Use a named class or enum with an explicit or derived Hashable implementation instead of a tuple in a Hashable bound. |
| D-640 | Rust diagnostic deserialisation and validation accept a missing or wrong class, info severity and malformed correction edits against the published record schema; the Python validator rejects all six cases. | Validate externally supplied diagnostic records with the Python diagnostics.validate function before consuming them. |
| D-669 | Both checkers fail to infer the result argument of Outcome.map when its callback returns the bare Void value: ("21" as Int).map(fn(n) => Void) is a diagnostic without an expected outcome type. An annotated Outcome[Void] binding checks and runs. | Annotate the receiving binding Outcome[Void], or use an empty block lambda fn(n) { }. |
| D-748 | Native calls to generic associated methods omit owner type arguments not inferred from value arguments. | Use a free function: func answer() Int => 7; bind let f = answer and call f() or answer(). The exec_rc3_r6_generic_workaround.ft fixture prints 7 7 on native, the interpreter and the VM. |

## Backend refusals

<!-- no-citation: each row is a recorded loud refusal; the row names which backends run or refuse it -->

| Defect | What you meet | Workaround |
|---|---|---|
| D-753 | Native refuses an ordinary function stored in an anonymous record, even when aliased before calling; the boxed callable field and generated display do not build. Related to D-685. | Use class Holder[F] { work: F } with Holder[fn(String) Int](work: move parse), then let f = alias holder.work. This prints 3 on all three backends. |
| D-751 | Native cannot call through a try fn or async try fn argument, return or record field. The interpreter and VM run these forms. | Use a function declared try, kept in a local binding, Holder[try fn(String) Int] with work: move parse and let f = alias holder.work, or a tuple that is destructured before calling. These forms print 3 on all three backends. |
| D-752 | Both checkers accept holder.work("3"), but every backend refuses the record-field call as no method work. | Bind let f = alias holder.work before calling. The original record then runs on interpreter/VM; native still meets D-751. For all three, use Holder[try fn(String) Int](work: move parse) and the same alias binding. |
| D-54.10 | The VM and native IR validator refuse a catch that returns inside a for loop, reporting a use after move. The interpreter test passes. | Move the catch out of the loop or return a result from a helper. |
| D-55.6 | A narrowed optional is not unwrapped in the native build. copy of a String? after != null, and a field read through a narrowed shared optional, each make fortis build exit 1 (rustc E0308, or no such field). The interpreter and the VM print the value. | Run it with fortis run or fortisc vm. |
| D-55.8 | A dynamic fn in a list or a field does not build natively (rustc E0308 for the list, E0277 for the field). The interpreter and the VM run the programs. | Run it with fortis run or fortisc vm. |
| D-55.16 | A local optional narrowed by != null and moved into a non-optional field is not unwrapped. fortis build exits 1 (rustc E0308). The interpreter and the VM print 7. | Run it with fortis run or fortisc vm. |
| D-157.1 | A closure stored in a class field does not build natively (rustc E0277, the field's display). The interpreter and the VM run the program. Assigning a second lambda to one closure binding is a separate item and is rejected by both checkers. | Run it with fortis run or fortisc vm. A dynamic fn is the heterogeneous form, and a field of that type is D-55.8, which also refuses natively. |
| D-270 | A function declared to return one non-dynamic fn type may return two different lambdas. The interpreter and the VM run both (1 and 2). fortis build exits 1 (rustc E0308). | Run it with fortis run or fortisc vm. Return one lambda, or declare the result dynamic fn when the two bodies really differ. |
| D-342 | A test's expect that compares an optional with a literal, such as expect first(names) == "a", does not build natively (rustc E0614). fortis test and fortisc test pass the test. | Bind the present value first (let got = first(names) ?? "none", then expect got == "a"), or run the test with fortis test or fortisc test. |
| D-360 | A record literal that holds a loan, such as { item: xs.get(0), n: 1 }, does not build natively (rustc E0106), nor does a tuple with a loan passed to a read parameter of tuple type (E0308). The interpreter and the VM run them, and each cleanup runs once. A tuple holding a list's loan, such as (xs.get(0), 1), or an alias loan of a local, such as (alias own, make("p")), builds. | Run it with fortis run or fortisc vm. Or keep the loan in its own binding and pass it on its own. |
| D-361 | A binding that holds a loan on one path and a value of its own on another does not build natively (rustc E0308): an if that gives xs.get(0) on one branch and move d on the other, or assigning a new class value, which the binding then owns, to a let mut binding that held a loan. The interpreter and the VM run it, and the binding destroys only a value it owns. | Run it with fortis run or fortisc vm. Or give the binding one kind of value: copy the loan, or keep the owned value in a binding of its own. |
| D-583 | A moved generic function value passes both checkers and prints 1 on VM/native, but the interpreter refuses it with dynamic check failed: store is not declared. Passing the function directly as a callback works. | Pass the generic function directly to the callback parameter instead of moving a function-value expression. |
| D-605 | Native refuses direct present injection of a concrete value into an optional dynamic interface parameter. | Bind the dynamic interface value, then its optional, before passing it to the function. |
| D-668 | For a package inside the toolchain checkout, running bin/fortis from another working directory loses the checker facts for as conversions. A valid newtype conversion is then refused by the interpreter with a dynamic check failure; the VM runs the package. | Run packages inside the toolchain checkout with that checkout as the working directory. |

## Defects carried from earlier releases

<!-- no-citation: each row is a defect an earlier release already had, recorded for the next release under X-416 -->

The checkers accept these programs, but a backend can run them wrongly or violate the Language Reference. An earlier release had every one of them; v0.1.0 is no worse, and each is fixed in the release its record names. Avoid the shape, as the second column says.

| Defect | What you meet | What to do |
|---|---|---|
| D-750 | A try function returning an inner Outcome flattens it on the interpreter and VM: a nested failure escapes instead of remaining the success value. Native prints 7; the other backends report demo.bad. Pre-existing since v0.0.2, carried under X-416. | Return {inner: move o} in a record, then bind let box = nested() or fail and let {inner: inner} = move box before handling inner. This prints 7 on all three backends. |
| D-754 | Both checkers accept an unbounded type parameter where a concrete type is expected, so a generic function can return or bind its argument as any type. Pre-existing since v0.0.2, carried under X-416. | Use a type parameter only where the same type parameter is expected: return T as T, store it in a field of type T, pass it to a parameter of type T. The checkers do not yet reject T used as a concrete type, so review generic bodies for this. |
| D-755 | A synchronous class method satisfies an async interface method, and a call through a bound runs its body before await. Pre-existing since v0.0.2, carried under X-416. | Declare the implementing method async whenever the interface method is async; the program then prints before, body, 3. |
| D-756 | A lambda that is async only through a contract is typed async fn by the checkers but runs as a synchronous function, so its body runs before await. Pre-existing since v0.0.2, carried under X-416. | Write async on the lambda itself, or declare a named async function, instead of relying on a contract to make a lambda async. |

## What v0.1.0 leaves for later

<!-- no-citation: each row is a limit of this release that its release note lists; the Defect cell names the record of a gap left open on purpose, and is empty for a limit by design -->

| Defect | What you meet | What to do |
|---|---|---|
|  | The older forms are accepted alongside the new ones, so a reader may meet both until the code is migrated. | Run fortis fmt --write on the code. It rewrites each older form where the program means the same. |
| D-404 | fortis fmt never rewrites Int.shiftLeft(a, b) or fail to a << b or fail, although the two are the same operation; Int.add, sub, mul, div and rem are rewritten. | Write the operator form by hand, or leave the named call. The two mean the same. |
|  | fortis fmt does not rewrite a logging call, add @ignorable or derive Default, or rewrite a main that reads its Capabilities record other than field by field (a capability inside an interpolation, or the record moved whole). | Make those changes by hand. Each is the author's choice. |
| D-406 | fortis fmt keeps a pure or noalloc written on a lambda, although the function type the lambda is passed to already states it; it prints no inferred one. | Remove the modifier by hand if you want the shorter form. It changes nothing the checker decides. |
|  | Checked operators cover Int only. The sized integer types (Int32, UInt8 and the rest) have no checked, wrapping or saturating operations yet; they are planned for v0.4. | Give the operands a refined type, or test them first, so that the operation is proved. Or compute in Int. |
|  | fortis fmt prints a module's inferred facts back only when the module checks on its own, so a module of a package that needs the rest of its package to check is not given them. | Nothing changes in what the program means; the checker infers the facts either way. Write a fact by hand where you want it shown. |
