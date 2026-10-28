"""The Fortis definitional interpreter (DEL-08 skeleton: the kernel, dynamic checks only).

Written for clarity, not speed. It evaluates the parse tree produced by conformance/tools/parser.py
over the resolver's view of a module, with dynamic type, move and bounds checks standing in for
the static judgements that later chunks add. Where a later static layer plugs in is marked with
`# plug-in:` comments in eval.py (types, loans, obligations, effects)."""
