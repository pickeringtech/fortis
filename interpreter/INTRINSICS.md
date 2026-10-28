# Kernel intrinsics

Since DEL-26 the intrinsics of the core library are one registry, `interpreter/intrinsics.py`
(`METHODS`, `ASSOC`, `FUNCS`), and each one's contract is the `// intrinsic:` comment above its signature in
`library/fortis/*.ft`. Derived operations (`List.reversed`, `map`, `filter`, `countWhere`, `concat`, `first`,
`isEmpty`, `ListView.isEmpty`, `ListView.reversed`, `String.empty`, `fortis.numeric.abs/min/max`,
`fortis.collections.sorted`) are Fortis bodies the interpreter runs. `conformance/tools/library_check.py`
verifies that every signature has a body or a registered intrinsic with a contract, that the signatures equal
the DEL-06 ones, and that the modules check with no diagnostics.

Outside the four core modules, the host capabilities (`Console.print`, `Clock.sleep`) are still dispatched in
`eval.py`; DEL-28 moves them behind development adapters.
