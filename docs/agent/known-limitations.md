# Known limitations of v0.0.1

<!-- covers: LR 0 -->
*Agent mode. Human mode: [Known limitations of v0.0.1](../guide/known-limitations.md). Release note: [v0.0.1](../releases/v0.0.1.md).*

Open, non-blocking defects in v0.0.1's scope, plus loud refusals (X-343): a checked program, a non-zero exit, another backend correct. Do not treat a row as a licence to change the program's meaning. A soundness bug or a wrong result on every backend is absent here on purpose.

Regenerate both pages with `python3 -I docs/delivery/tools/limitations.py`.

## Valid programs the checkers reject, and library gaps

<!-- no-citation: each row is a toolchain limitation already recorded against v0.0.1; the language rules are unchanged -->

| Defect | What you meet | Workaround |
|---|---|---|
| D-51.5 | An unused import is accepted with no warning. | Delete the import. Nothing else changes. |
| D-52.1 | An error constructor is typed as its own kind, so a use that the language accepts where a failure is expected is rejected. | Match the constructor's own type, or construct the failure where its own name is expected. |
| D-52.4 | Returning a call that passes a loan-holding closure is rejected even when the result is a plain integer. | None recorded. |
| D-52.6 | A class you declare with the same name as a prelude type, such as Task, is checked as the prelude's linear type. | Name the class something the prelude does not declare. |
| D-52.9 | A comparison or a null test narrows in an if statement and does not narrow in an if expression. | Write the test as an if statement, then use the narrowed name in the branch. |
| D-52.10 | if x == null { return } does not narrow an optional loan for a later copy. | Test the optional with != null in the branch that copies it. |
| D-54.6 | For a parameter named with a keyword, fortisc vm reports a generic syntax error and fortisc check reports the keyword-specific one. | Rename the parameter. Both tools refuse the program. |
| D-55.11 | A native build of a package names the package directory as the record's file, rather than the module. | The message still names the fault. Open the module yourself; the directory in the record is the package. |
| D-58.1 | Map has no iterate operation. A for loop over a map yields key and value tuples, which the language reference does not state. | Write for over the map when a key and a value together are enough. |
| D-58.7 | Map has no remove and no contains. | None recorded. Build a new map without the key, or scan it. |
| D-58.9 | Every text accessor can fail, so a comparator written as a pure function of two texts must recover. | Recover inside the comparator, or do not mark it pure. |
| D-158 | Both checkers reject a class pattern nested inside another pattern. | Match the outer pattern, then match the inner value in the arm. |
| D-251 | A variant whose name is also a class's is rejected by both checkers, so the program does not reach the VM. List.of(Red) with the list's element type written out runs on the interpreter, the VM and native. | Write the variant with its enum prefix, or do not declare a class of the same name. The list form already runs when the expected type is written. |
| D-252 | Both checkers reject an optional-chained method call such as lights.get(0)?.code(). The call does not reach the VM. | Match the optional and call the method on the present value. |
| D-285 | A package whose manifest the loader refuses (conformance/packages/bad.policy) makes fortis check, run and test exit 2 with a message on standard error, and fortisc check exit 1 with a names record on project.yaml. | Both tools refuse the package. Read fortisc's record; it names the manifest. |

## One backend refuses the program

<!-- no-citation: each row is a loud refusal under X-343; another backend runs the same program -->

| Defect | What you meet | Workaround |
|---|---|---|
| D-54.10 | A catch that returns inside a for is refused: fortisc test and fortis build exit 1, reporting that the typed IR uses a moved binding. The interpreter's test passes. A move in a returning branch, the other half of this entry, builds and its VM test passes. | Run that test with fortis test. Keep the catch out of the loop, or bind the failure's value before the loop continues. |
| D-55.6 | A narrowed optional is not unwrapped in the native build. copy of a String? after != null, and a field read through a narrowed shared optional, each make fortis build exit 1 (rustc E0308, or no such field). The interpreter and the VM print the value. | Run it with fortis run or fortisc vm. |
| D-55.8 | A dynamic fn in a list or a field does not build natively (rustc E0308 for the list, E0277 for the field). The interpreter and the VM run the programs. | Run it with fortis run or fortisc vm. |
| D-55.16 | A local optional narrowed by != null and moved into a non-optional field is not unwrapped. fortis build exits 1 (rustc E0308). The interpreter and the VM print 7. | Run it with fortis run or fortisc vm. |
| D-156 | A function-type parameter that owns its argument, fn(owned T), is lowered as a borrowed parameter. fortis build exits 1 (rustc E0308). The interpreter and the VM run the call. | Run it with fortis run or fortisc vm. |
| D-157.1 | A closure stored in a class field does not build natively (rustc E0277, the field's display). The interpreter and the VM run the program. Assigning a second lambda to one closure binding is a separate item and is rejected by both checkers. | Run it with fortis run or fortisc vm. A dynamic fn is the heterogeneous form, and a field of that type is D-55.8, which also refuses natively. |
| D-270 | A function declared to return one non-dynamic fn type may return two different lambdas. The interpreter and the VM run both (3 and 4). fortis build exits 1 (rustc E0308). | Run it with fortis run or fortisc vm. Return one lambda, or declare the result dynamic fn when the two bodies really differ. |
