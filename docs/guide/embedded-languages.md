# Embedded languages

<!-- covers: LR 16 -->
*Human mode. Agent mode: [Embedded languages](../agent/embedded-languages.md). Source of truth: LR 16.*

> Preview: declaration heads and kits (LR 16) arrive in v0.5.

Many programs hold text in another language: a SQL query, an HTML template, a regular expression, a JSON
document. This page says what Language 1.0 offers for that, and what it does not.

## What Language 1.0 has

The only embedded text the core language defines is the **raw template region** of the Web UI kit (LR 16).
The region is opened lexically, with no knowledge of the declaration around it: a statement whose tokens are
`view`, optionally preceded by `export` and by one of `server`, `client` or `hybrid`, followed by nothing, a
name, or a name and a parenthesised group, and then `{`. The lexer takes the text from that `{` to the `}` that
balances it, counting every brace including those of `${…}`, and hands it to the Web UI kit as one token, unlexed
(LR 16). Template syntax is therefore the kit's to define, and a template never adds a token to the core.

Recognising a region and permitting it are two steps. The lexer recognises one wherever such a statement occurs;
the kit then validates where it may appear, like the rest of a head body: inside a `component` body, or as a
standalone `view` head of its own (LR 16). Where the `view` is a declaration, the grammar requires its group to
be a parameter list; inside a head body the kit validates it.
[Kits and declaration heads](16-kits.md) explains heads and the four extension slots.

Everywhere else, text in another language is an ordinary Fortis string, a `StringView` or `String` (LR 3.5,
LR 3.6). Interpolation in a string is plain text interpolation: it never escapes for HTML, SQL or any other
context (LR 3.6), so a program that builds a query or a page from strings must escape or bind its values itself,
through a library that does so.

## What a later edition plans

Fenced blocks of another language, written inside a module with a language tag and typed holes, checked by an
embedding binding and lowered to a typed value (PROG-PRD-29 to PROG-PRD-32), are **not part of Language 1.0**;
they move to a later edition (X-145). Until then, none of that syntax exists, and a program that tries it is a
syntax error (LR 3.2). The word `embed` is set aside for that edition (X-145).
