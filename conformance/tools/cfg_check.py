"""DEL-12 golden CFGs: builds the CFG of every function in conformance/cfg/*.ft, compares its dump
with conformance/cfg/golden/<file>.<func>.cfg (--update rewrites them) and checks that the dump
round-trips through the text parser. Exit 1 on any difference or round-trip failure."""
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
sys.path.insert(0, HERE)
sys.path.insert(0, ROOT)
import lexer      # noqa: E402
import parser     # noqa: E402
import tree       # noqa: E402
from checker import cfg as cfgmod  # noqa: E402

D = os.path.join(ROOT, "conformance", "cfg")
G = os.path.join(D, "golden")


def main(argv):
    rec = parser.load_grammar(os.path.join(ROOT, "reference", "grammar", "fortis.ebnf"))
    os.makedirs(G, exist_ok=True)
    failures, n = 0, 0
    for f in sorted(os.listdir(D)):
        if not f.endswith(".ft"):
            continue
        src = open(os.path.join(D, f), encoding="utf-8").read()
        t = rec.parse(lexer.lex(src), "module")
        for d in tree.declarations(t):
            if d["kind"] != "func":
                continue
            g = cfgmod.build(d["node"], d["name"])
            if g is None:
                continue
            n += 1
            text = cfgmod.dump(g)
            gp = os.path.join(G, f"{f[:-3]}.{d['name']}.cfg")
            rt = cfgmod.dump(cfgmod.parse(text))
            if rt != text:
                print(f"FAIL {f}:{d['name']} does not round-trip through the text form")
                failures += 1
                continue
            if "--update" in argv or not os.path.exists(gp):
                open(gp, "w", encoding="utf-8").write(text)
                print(f"WROTE {os.path.relpath(gp, ROOT)}")
                continue
            golden = open(gp, encoding="utf-8").read()
            if golden != text:
                print(f"FAIL {f}:{d['name']} differs from golden {os.path.relpath(gp, ROOT)}")
                failures += 1
            else:
                print(f"PASS {f}:{d['name']} ({len(g.blocks)} blocks) matches golden and round-trips")
    print(f"\n{n} CFGs, {failures} FAIL")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
