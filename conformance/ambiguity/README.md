# Ambiguity corpus (DEL-02)

Programs that exercise the places where the Fortis grammar, read naively, could be parsed in more
than one way. Each site has at least three programs. The generated parser must parse every program
with exactly one parse: no undocumented fallback; the decisions it takes are those of the lexer
(policy P1) or the documented parser policies P2 (greedy option/repetition) and P3 (ordered choice),
listed in `conformance/generated/lookahead.md` and Language Reference 3.2 (X-94).

| Site | Naive ambiguity | Decision |
|---|---|---|
| S1 refinement vs comparison | `Int<0..9>` in type position vs `a < b` | lexer: `<` directly after a type name in type position is REF_OPEN (P1 family) |
| S2 braces | `{` opens a block, a record literal or a record type | P3 at the statement level: a statement starting with `{` is a block; in expression position `{ name:` is a record literal |
| S3 keyword as member | `x.from`, `x.to()`, `fortis.http.server` | lexer: a keyword after `.` is a memberName token (P1 family) |
| S4 newline runs | blank lines and comment-only lines between members | lexer: one NL per run (P1) |
| S5 marker before a call | `move p.method()` vs `move f()` vs `move p` | P3 ordered: place then method call, then closure call, then bare place |
| S6 fn type vs lambda | `fn(Int) Int` as a type vs `fn(x: Int) Int => …` as a value | position decides: types appear only after `:` / `)` / `[`, lambdas only in expression position; P2 greedy body |
| S7 type-rooted primaries | `Name(args)` constructor, `Name.member(args)`, `Enum.Variant`, `Int<0..9>.validate(x)` | P2 greedy option: the optional member and call are taken when present |
| S8 range vs expression in `for` | `for i in a..<b` vs `for x in xs` vs `for x in move xs` | P3 ordered: `move place`, then range, then expression |
