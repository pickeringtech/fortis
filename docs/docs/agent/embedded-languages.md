# Embedded languages (LR 16)

<!-- covers: LR 16 -->
*Agent mode. Human mode: [Embedded languages](../guide/embedded-languages.md). Source of truth: LR 16.*

> Preview: declaration heads and kits (LR 16) arrive in v0.5.

| Fact | Source |
|---|---|
| The core's only embedded text is the raw template region: a statement `[export] [server\|client\|hybrid] view [name [(…)]] {` opens it, recognised lexically with no knowledge of the enclosing declaration; the lexer takes the text to the balancing `}` (every brace counted, `${…}` included) and hands it to the Web UI kit unlexed, as one token. | LR 16 |
| Recognition is the lexer's; permission is the kit's: it validates where a region appears, inside a `component` body or as a standalone `view` head. Where the `view` is a declaration, its group must be a parameter list. Template syntax is the kit's; a template adds no core token. | LR 16 |
| Elsewhere, another language's text is a string (`StringView` or `String`). Interpolation never escapes for HTML, SQL or any context: bind or escape values through a library. | LR 3.5, LR 3.6 |
| Fenced embedded-language blocks with typed holes, `embed … from`, embedding bindings and their verifiers are not in Language 1.0 (a later edition, X-145). Do not write them: they are not syntax (LR 3.2). | X-145 |
