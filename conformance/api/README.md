# API-change fixtures (LANG-11-05, DEL-43 criterion 3)

Each directory holds two versions of one module, `old.ft` and `new.ft`. `fortisc check --api-baseline old.ft
new.ft` must report exactly the expected `E-EFF-040` warnings (a weakened exported signature), and none for a
strengthening. `conformance/tools/api_check.py` runs them.

| Fixture | Change | Expected |
|---|---|---|
| `removes-pure` | an exported function drops its written `pure` (and a method its contract's `noalloc`) | two `E-EFF-040` |
| `adds-try` | an exported function becomes `try` | one `E-EFF-040` |
| `strengthens` | an exported function gains `pure`, another loses `try` | none |
