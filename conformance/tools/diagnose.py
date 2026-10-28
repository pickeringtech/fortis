"""`diagnose.py FILE...`: lex and parse Fortis files and emit their syntax diagnostics as records.
Text on a terminal, JSON lines on a pipe, `--format text|json` overrides (Programme 7.4)."""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import diagnostics  # noqa: E402
import parser       # noqa: E402
import run          # noqa: E402

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))


def diagnose(rec, path):
    run.CURRENT_FILE[0] = os.path.relpath(path, ROOT) if os.path.isabs(path) else path
    del run.RECORDS[:]
    src = open(path, encoding="utf-8").read()
    run.judge(rec, src)
    return list(run.RECORDS)


def main(argv):
    fmt = None
    files = []
    for a in argv:
        if a.startswith("--format="):
            fmt = a.split("=", 1)[1]
        else:
            files.append(a)
    rec = parser.load_grammar(os.path.join(ROOT, "reference", "grammar", "fortis.ebnf"))
    records = []
    for f in files:
        records += diagnose(rec, f)
    diagnostics.emit(records, sys.stdout, fmt)
    return 1 if records else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
