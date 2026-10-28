"""The static checker of the definitional reference (Language Reference 6–13), built layer by layer:
types and declarations (DEL-10), literals (DEL-11), the typed CFG and expression typing (DEL-12),
refinements and the interval domain (DEL-13), then ownership, outcomes and effects. Diagnostics are
records (conformance/tools/diagnostics.py). It shares the lexer, parser, tree and name resolver with
the conformance tools and the interpreter."""
