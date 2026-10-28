"""DEL-20 runner: aspect elaboration goldens (LANG-11-09). For every `conformance/aspects/<fixture>.golden`
the checker's elaboration record of `conformance/programs/<fixture>.ft` (hooks in application order, after
hooks reversed, failure handlers innermost first) must equal the golden text."""
import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
sys.path.insert(0, os.path.join(ROOT, "conformance", "tools"))
sys.path.insert(0, ROOT)
import lexer      # noqa: E402
import names      # noqa: E402
import parser     # noqa: E402
from checker import check  # noqa: E402


def main(argv):
    rec = parser.load_grammar(os.path.join(ROOT, "reference", "grammar", "fortis.ebnf"))
    lib = names.Library(rec)
    gdir = os.path.join(ROOT, "conformance", "aspects")
    failures = 0
    goldens = sorted(f for f in os.listdir(gdir) if f.endswith(".golden"))
    for g in goldens:
        fid = g[:-len(".golden")]
        src = open(os.path.join(ROOT, "conformance", "programs", fid + ".ft"), encoding="utf-8").read()
        records = check.check_source(lib, src, fid + ".ft")
        got = "\n".join(check.LAST_ELABORATIONS) + "\n"
        want = open(os.path.join(gdir, g), encoding="utf-8").read()
        ok = got == want and not records
        failures += not ok
        print(f"{'PASS' if ok else 'FAIL'} ASP-{fid}")
        if not ok:
            print("  want:", want.strip())
            print("  got: ", got.strip(), [r["code"] for r in records])
    print(f"{len(goldens)} aspect goldens, {failures} FAIL")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
