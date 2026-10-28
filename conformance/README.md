# Fortis G0 conformance corpus

This directory is the first slice of gate G0 of the Delivery Programme: the executable corpus
derived from the specification's examples, and the syntax layer that runs it.

## Layout

| Path | Content |
|---|---|
| `tools/ebnf.py` | Loads `reference/grammar/fortis.ebnf`; reports undefined and unreachable rules. |
| `tools/lexer.py` | The Fortis lexer (Language Reference 3): identifier categories, keywords with the member-position relaxation, literals, refinement delimiters, newline termination and continuation, body versus expression braces, raw `view` template regions. |
| `tools/parser.py` | A recogniser that interprets the EBNF directly over the token stream. It computes the set of end positions for every rule at every position (memoised), so every alternative is explored and an ambiguous grammar cannot hide behind ordered choice. |
| `tools/extract.py` | Extracts every ```` ```fortis ```` block from the three specification documents into `corpus/` with `manifest.json`. Fails if a block is unmarked. |
| `tools/run.py` | Runs every fixture through lexer and recogniser and judges it against its marker. |
| `corpus/` | Generated. One `.ft` per example, named `<doc>-<section>-<ordinal>` (`LR` Language Reference, `PS` Platform Specification, `OV` overview). Do not edit; edit the document. |
| `programs/` | Hand-written programs: the three independent programs the deciding review cycle traced (CLI, two-module `derive Codec`, async transaction). |
| `results.json` | Written by `run.py --json`; the evidence record for the syntax layer. |

## Running

```sh
python3 -I conformance/tools/ebnf.py reference/grammar/fortis.ebnf
python3 -I conformance/tools/extract.py
python3 -I conformance/tools/run.py --json
```

No dependencies beyond Python 3.10.

## Markers and judgements

A block's first line states what the suite must establish (Language Reference 0):

| Marker | Syntax layer obligation |
|---|---|
| `// accept`, `// execute: …` | must lex and parse, as a module or as a function body (statement sequence) |
| `// reject: E-SYN-…` | must fail to lex or parse |
| `// reject: <other code>` | must parse; the rejection is semantic and belongs to a later layer, so the result is labelled parse-only |
| `// illustrative` | parsed for information only, as a module, a statement sequence, a list of types or a list of function heads; templates with `…` or bracketed placeholders are not parsed |

A line inside a block marked `// reject: E-…` records the diagnostic expected at that line; the
manifest carries these as `marked_lines` for the semantic layer.

## What this layer does and does not establish

It establishes that the grammar is well formed and that every example in the specification is a
sentence of it under the lexer rules of Language Reference 3, and that the syntax rejections are
rejected. It does not type check, resolve names, check ownership, obligations or effects, and it
does not execute anything. Those are the remaining G0 layers (semantic scenarios and the
definitional interpreter skeleton), together with the generated parser and the ambiguity corpus
with bounded lookahead that the Programme requires for G0.
