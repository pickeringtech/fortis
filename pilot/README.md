# The `ledger` pilot

A command-line bookkeeping tool written in Fortis from the documentation alone, for the documentation pilot of
the delivery plan (PROG-AGT-08). The application is in `ledger/`, the friction log in `FRICTION.md`, and in
`repro/` a minimised program for each toolchain defect a small program shows (20 of them; the one for entry 49
was added after the pilot, at the documentation review). The other tool findings are shown by the commands their
friction entries give (messages, records, test output and command lines: entries 1, 14, 25, 41, 46, 50), by the
library's declarations (3, 24, 31), or only by the whole application (11); the defect register names
each case's reproduction.

## What it does

`ledger` reads a double-entry journal from a text file through the `FileSystem` capability, parses it, checks
it, reports every malformed or invalid line once with its line number, and keeps going: the reports are built
from the transactions that passed every check.

```text
; comments start with `;`, `#` or `//`
commodity GBP 2                         a commodity and its number of decimal places (0 to 8)
account assets:bank:current             an account postings may use; the first segment is assets,
                                        liabilities, equity, income or expenses
2026-01-15 * Salary #work               date, optional `*` (cleared) or `!` (pending), description, #tags
  assets:bank:current   2,840.00 GBP    indented postings: account, amount, commodity
  income:salary                         one posting may leave its amount out; it is inferred
```

Checks: dates are calendar days (`YYYY-MM-DD`), amounts have at most their commodity's decimal places,
commodities and accounts are declared, every transaction has at least two postings and at most one left-out
amount, every commodity of a transaction sums to zero, and dates are in order (a warning; the transaction is
kept). Every problem is one diagnostic on the line it is about (`line 75: error amount.too_precise: "3.505" has 3
decimal places but the commodity has 2`); a line never gets two.

Reports: balances per account; balances per account tree (every parent holds the sum below it); a trial balance
(debits and credits per commodity, which must agree); an income statement for a period; a register of the
postings of one account and the accounts below it, with a running balance. Amounts are exact: integers of each
commodity's smallest unit, with checked arithmetic, so an overflow is a reported problem and never a wrong total.

### Modules (`ledger/src/`)

| Module | Contents |
|---|---|
| `scan` | lines, trimming, splitting, searching, integer parsing, padding (the library has no text utilities) |
| `date` | `Date` (a trivial class of refined fields), parsing and calendar checks, day numbers, ordering |
| `amount` | `CommodityCode` (a newtype over a refined `String`), amount parsing and formatting, `Commodities`, `Holdings` |
| `account` | `AccountName` (a class with private fields, built only by `parse`), `AccountClass`, report order |
| `order` | a stable generic merge sort (`sortedBy`) |
| `diagnostic` | `Problem`, `Severity`, `Diagnostic`, `Diagnostics` (at most one per line), `settle` |
| `journal` | `PostingDraft`, `TransactionDraft`, `Posting`, `Transaction`, `Journal`, `Period`, `Flag` |
| `parser` | the stateful `Parser` class: one line at a time, failures turned into diagnostics and handed to a sink |
| `validate` | balancing and inference (`Verdict` enum), declared accounts, date order |
| `books` | `Balances` per account and rolled up the account tree |
| `reports` | the five reports, each a pure function returning its lines |
| `cli` | the command line (`Command` enum, `UsageError`) |
| `main` | the entry point: capabilities, reading the file, printing, reporting failures |

Declared errors: `NotANumber`, `EmptyNumber`, `DateSyntax`, `DateRange`, `AmountSyntax`, `TooPrecise`,
`BadCommodityCode`, `BadAccountName`, `UsageError`, `JournalHasErrors`. The parser and the validator are `pure`;
only `main` and its helpers touch the console and the file system.

One design decision came from the toolchain rather than the problem: a package's `main` may not have the
`recovers` effect (FRICTION 39), so the parser never discards a failure. It turns each into a diagnostic and
moves the failure itself, annotated with its journal line, into a sink (`List[Failure]`) that `main` reports; so
every parse failure also appears once as a failure record on standard error.

## Running it

From the repository root. The development file system starts empty on every run, so the journal's directory is
given with `FORTIS_DEV_ROOT` (FRICTION 2):

```sh
FORTIS_DEV_ROOT=pilot/ledger/data bin/fortis run pilot/ledger                  # every report
FORTIS_DEV_ROOT=pilot/ledger/data bin/fortis run pilot/ledger balance
FORTIS_DEV_ROOT=pilot/ledger/data bin/fortis run pilot/ledger tree
FORTIS_DEV_ROOT=pilot/ledger/data bin/fortis run pilot/ledger trial
FORTIS_DEV_ROOT=pilot/ledger/data bin/fortis run pilot/ledger income 2026-01-01 2026-01-31
FORTIS_DEV_ROOT=pilot/ledger/data bin/fortis run pilot/ledger register assets:bank
FORTIS_DEV_ROOT=pilot/ledger/data bin/fortis run pilot/ledger check            # exit status 1 on errors
FORTIS_DEV_ROOT=pilot/ledger/data bin/fortis run pilot/ledger --file other.ledger balance
FORTIS_DEV_ROOT=pilot/ledger/data compiler/target/release/fortisc vm pilot/ledger   # the VM (no arguments)
```

The sample journal, `ledger/data/journal.ledger` (112 lines, three months of household books), contains five
errors and one warning on purpose. The start of `bin/fortis run pilot/ledger` (exit status 0):

```text
ledger: journal.ledger: 16 transactions, 39 postings, 13 accounts, 2026-01-01 to 2026-03-20

Problems (errors: 5, warnings: 1)
  line 50: error date.invalid: 2026-13-02 is not a day of the calendar
  line 75: error amount.too_precise: "3.505" has 3 decimal places but the commodity has 2
  line 83: error transaction.unbalanced: the postings sum to 1.00 GBP, not zero
  line 87: warning date.out_of_order: 2026-02-14 is earlier than 2026-02-15 on line 78
  line 100: error account.undeclared: account expenses:transport is not declared
  line 112: error directive.unknown: "budget" is not a directive or a date

Balances by account
  assets:bank:current             6,422.45 GBP
  assets:bank:savings             5,004.17 GBP
  assets:cash                        97.00 GBP
                                    -4,800 JPY
  ...
```

followed by the account tree, the trial balance (`debits equal credits`), the income statement for the whole
journal and the register of `assets`: 128 lines in all, identical on the interpreter and on the VM. Standard error
gets the two failure records behind lines 50 and 75.

## Tests

Tests sit at the end of each module: 77 tests (25 of them table tests) and 18 properties. Properties cover
integer printing and parsing, trimming, amount formatting and parsing, holdings arithmetic, day numbers and date
order, sorting, report-order of accounts, roll-ups, tree totals, inferred amounts and text fitting. The reports
are checked line by line against a journal whose figures were verified by hand.

| Backend | Command | Summary line |
|---|---|---|
| definitional interpreter | `timeout 30m bin/fortis test pilot/ledger` | `673 passed, 0 failed` (130 seconds) |
| development VM | `timeout 10m compiler/target/release/fortisc test pilot/ledger` | `pilot/ledger: 241 passed, 0 failed` (about 1 second) |

The counts differ because the interpreter runs 25 cases per property and reports each, while the VM runs 100
and reports one result per property (FRICTION 14). Both checkers report nothing: `bin/fortis check pilot/ledger`
prints `pilot/ledger: no findings (13 modules)` and `compiler/target/release/fortisc check pilot/ledger` prints
nothing; `bin/fortis fmt --check pilot/ledger` lists no file.

### Native build (bonus): fails

`bin/fortis build pilot/ledger --out …` stops with `E-INT-002` ("the generated Rust does not compile"). The first
failure (`TransactionDraft: Clone` not satisfied) was a consequence of FRICTION 27 and is worked around in the
sources. The next three (an enum payload field, optionals narrowed in a generic function, a `const` passed by
reference) are minimised in `repro/native-*.ft` (FRICTION 43 to 45); after working around them in a throw-away
copy, a fifth appeared in `validate.balance` (FRICTION 49), where I stopped. The interpreter and the VM run the
same program correctly.

## Line counts

Non-blank lines, counted from `bin/fortis view pilot/ledger/src --json` (the `line` records whose text is not
blank); "tests" are the lines from each module's first `test` or `property` declaration to its end.

| Module | Source | Tests |
|---|---:|---:|
| account | 226 | 116 |
| amount | 332 | 149 |
| books | 120 | 65 |
| cli | 202 | 81 |
| date | 178 | 92 |
| diagnostic | 145 | 57 |
| journal | 125 | 12 |
| main | 131 | 0 |
| order | 93 | 55 |
| parser | 412 | 105 |
| reports | 283 | 128 |
| scan | 259 | 134 |
| validate | 189 | 85 |
| **total** | **2,695** | **1,079** |

3,774 non-blank lines in all. About 190 of the "source" lines are test fixtures and helpers that sit before the
first test of their module (the sample journals in `parser`, `validate`, `books` and `reports`, `parsedForTest`,
`loadedForTest`, `settle`, `describeCommand`), so the application proper is about 2,500 lines and the tests about
1,270. The repros in `repro/` add 531 lines and are not counted.

## Summary of the friction log

53 entries (`FRICTION.md`): 27 toolchain defects (19 with a repro in `repro/`), 10 documentation gaps, 4
documentation errors, 4 language questions, 7 things that worked well, and one note on my own process.

- **The language and its tools disagree with themselves.** Both checkers accept programs that fail at run time
  on both backends (`<` on a derived `Comparable`, including a guide example the README says is run; a
  hand-written `Displayable`; `validate` into String types), and programs the VM then refuses as a whole
  ("typed IR is invalid", "not built"). What a module may do with a type depends on which other types it imports.
  Checking or testing one file of a package gives different results from the package.
- **One rule changed the design.** A package's `main` cannot be `recovers` (a single file's can), and
  `try recovers` is refused everywhere, so turning failures into diagnostics in a pure parser required a failure
  sink that `main` reports. Separately, `recovers` vanishes when the discard happens inside a lambda passed to a
  `pure fn` parameter.
- **The library reference is missing.** The text and collection API had to be read from the library's
  declarations; it has no line splitting, trimming, searching, number parsing or `Decimal` parsing, and two
  modules are described with contents they do not have.
- **Smaller gaps:** where data files live, how to pass arguments, the `Void` value, reserved everyday words,
  newtype representations, testing failure payloads, matching library failures by kind.
- **What worked well:** the diagnostic records and their corrections; the interval checker proving real
  arithmetic on refined types; properties over refined inputs with shrinking; destructuring, enum payloads and
  `owned self`; capabilities making every I/O boundary visible; `try main` giving one record and exit status 1;
  identical output from the interpreter and the VM.
