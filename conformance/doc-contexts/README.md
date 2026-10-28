# Contexts for documentation fragments (DEL-43, PROG-TST-08)

A documentation example that is a run of statements leaves out the declarations it uses, so it cannot be checked
alone. Each file here is the context of one such example of the G1 subset, named by its fixture ID
(`conformance/corpus/<ID>.ft`). `conformance/tools/docs_check.py` pastes the example, all of its lines, in place of
the line `// {{example}}` and checks the result: an accept example must check without errors, and a reject example
must give its declared code at each marked line, with the lines mapped back to the example.

A `// base: <ID>` line first prepends that fixture's declarations (the worked module of section 2, `LR-2-01`), so a
context adds only what the example needs beyond it. A context declares only what the example names and leaves
undeclared; it never changes what the example does.
