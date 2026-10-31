# Coffee shop: checked domain model

<!-- covers: LR 6, LR 8, LR 9, LR 10, LR 11.5, LR 14 -->
*Agent mode. Human mode: [Model a domain](../guide/worked-example.md). Source of truth: LR 6, LR 8, LR 9,
LR 10, LR 11.5 and LR 14.*

The complete [v0.1.0 showcase](../../examples/showcase/v0.1.0-coffee.ft) and its
[expected stdout](../../examples/showcase/v0.1.0-coffee.stdout) are pinned. The gate checks both checkers and
runs `main` and tests on all three backends. The guide contains the same complete program, also checked by docset.

## Construction order and rules

| Step | Program shape | Rule and constraint |
|---|---|---|
| Model | `Pence`, `Size`, `Drink.quantity` | LANG-06-01/02/03: nominal newtype, const refinement bound, proved constructor; private unwrap with `as` (LR 6.3, LR 6.9, LR 6.13). |
| Behaviour | `Priced`, `Labelled`, `Pricing` | LANG-09-03/04/07, LANG-11-06: compatible interface methods, mixin method requirements, dynamic eligibility, consistent effect contract (LR 9.5, LR 9.6, LR 11.5). |
| Parse | enum derive and `Drink` conversion | LANG-06-14/15: ASCII case-insensitive enum names; trim separately; split, parse and refine; every fallible `as` handled (LR 6.6, LR 6.13). |
| Collect | loop over `orders.lines()` | LANG-10-01/03: move each success into `Basket`, report each failure; collection-newtype editing private to this module (LR 6.9, LR 10.2, LR 10.6). |
| Summarise | `each`, `fold`, searches, sorting, grouping | LANG-06-16, LANG-08-02/04: inferred captures only for directly loaned literals; exact callback modes and declared effects; copying methods require Copyable (LR 6.7, LR 8.2, LR 8.4, LR 11.3). |
| Fail early | `tryMap`, `tryEach`, `tryFold` | Outcome-returning callbacks, stop on first failure, handle the result (LR 6.7, LR 10.2). |
| Reuse | `Ticket`, `Pastry`, `larger`, `Pending` | Explicit field-wise conversion; implement block; Comparable and Copyable bounds; `where T: linear` and transfer of the owned obligation (LR 6.10, LR 6.12, LR 6.13, LR 9.5, LR 10.7). |
| Containers | `Deque`, `SortedSet`, `Array`, `Map`, `Set` | Import the first two; Array view borrows; hash keys need Hashable and Equatable; no stored loan outlives its owner (LR 6.7, LR 7.7). |
| Test | example, cases, property | LANG-14-04: `expect` only in test code; refine generated quantities by their type (LR 14). |

A declared `convert from String` member accepts literals and `StringView` sources as read loans; an `owned String`
parameter copies those sources (LR 6.13). Property tests run today, with their full release scope still v0.2.

## Verification

<!-- no-citation: tool commands and the showcase gate, not language rules -->

```sh
python3 -I conformance/tools/gate.py --only showcase
bin/fortis check examples/showcase/v0.1.0-coffee.ft --format json
bin/fortis test examples/showcase/v0.1.0-coffee.ft
compiler/target/release/fortisc test examples/showcase/v0.1.0-coffee.ft
compiler/target/release/fortisc native examples/showcase/v0.1.0-coffee.ft --tests
```

Read diagnostics before changing the source: [codes and corrections](codes.md). Do not change the expected
output or remove assertions to make a backend pass.
