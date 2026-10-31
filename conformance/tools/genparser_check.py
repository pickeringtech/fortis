"""DEL-02 runner: the generated parser against the recogniser on every fixture (agreement on
accept/reject, and on the tree itself where both accept: the generated parser derives exactly the recogniser's
tree, so a checker can run on either), the ambiguity corpus (exactly one parse: no undocumented fallback, recogniser agrees),
and the pinned timing run (warm process, median of 20 parses per fixture, 50 ms budget;
fixtures listed by hash). Timing is recorded but only judged when FORTIS_PARSER_TIMING=1,
for the quiet-machine v1.0 release check (X-149, X-678). Writes conformance/generated/check.json."""
import hashlib
import json
import os
import platform
import statistics
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
sys.path.insert(0, HERE)
sys.path.insert(0, os.path.join(ROOT, "conformance", "generated"))
import lexer      # noqa: E402
import parser     # noqa: E402
import fortis_parser  # noqa: E402

AMB = os.path.join(ROOT, "conformance", "ambiguity")


def gen_parse(toks, start):
    p = fortis_parser.FortisParser(toks)
    try:
        p.tree = p.parse(start)
        return True, p, ""
    except fortis_parser.ParseError as e:
        return False, p, str(e)


def shape(t):
    """A tree as nested tuples of rule names and (kind, text, line, col) tokens."""
    if isinstance(t, lexer.Token):
        return (t.kind, t.text, t.line, t.col)
    return (t[0], tuple(shape(c) for c in t[1]))


def main(argv):
    enforce_timing = os.environ.get("FORTIS_PARSER_TIMING") == "1"
    rec = parser.load_grammar(os.path.join(ROOT, "reference", "grammar", "fortis.ebnf"))
    syntax = json.load(open(os.path.join(ROOT, "conformance", "results.json"), encoding="utf-8"))["results"]
    out = {"agreement": [], "ambiguity": [], "timing": [], "timing_enforced": enforce_timing,
           "timing_budget_ms": 50, "platform": platform.platform(), "python": platform.python_version()}
    disagreements = tree_diffs = trees = 0
    for r in syntax:
        src = open(os.path.join(ROOT, r["path"]), encoding="utf-8").read()
        start = "block" if "statements" in (r.get("detail") or "") else "module"
        text = src if start == "module" else "{ " + src + "\n}\n"
        try:
            toks = lexer.lex(text)
        except lexer.LexError:
            continue
        try:
            rec.recognise(toks, start)
            ref_ok = True
        except parser.ParseFailure:
            ref_ok = False
        gen_ok, p, why = gen_parse(toks, start)
        agree = ref_ok == gen_ok
        same_tree = None
        if ref_ok and gen_ok and start == "module":
            trees += 1
            same_tree = shape(rec.parse(lexer.lex(text), start)) == shape(p.tree)
            tree_diffs += not same_tree
            if not same_tree:
                print(f"TREE DIFFERS {r['id']}")
        disagreements += not agree
        out["agreement"].append({"id": r["id"], "sha256": hashlib.sha256(src.encode()).hexdigest(), "recogniser": ref_ok, "generated": gen_ok, "fallbacks": p.fallbacks, "ordered": p.ordered, "agree": agree, "same_tree": same_tree, "why": why[:100]})
        if not agree:
            print(f"DISAGREE {r['id']}: recogniser {ref_ok}, generated {gen_ok} {why[:90]}")
    print(f"agreement: {len(out['agreement'])} fixtures, {disagreements} disagreements; trees: {trees} compared, {tree_diffs} different")
    # ambiguity corpus
    amb_fail = 0
    for f in sorted(os.listdir(AMB)) if os.path.isdir(AMB) else []:
        if not f.endswith(".ft"):
            continue
        src = open(os.path.join(AMB, f), encoding="utf-8").read()
        toks = lexer.lex(src)
        try:
            rec.recognise(toks, "module")
            ref_ok = True
        except parser.ParseFailure as e:
            ref_ok = False
        gen_ok, p, why = gen_parse(toks, "module")
        ok = ref_ok and gen_ok and p.fallbacks == 0
        amb_fail += not ok
        out["ambiguity"].append({"file": f, "sha256": hashlib.sha256(src.encode()).hexdigest(), "recogniser": ref_ok, "generated": gen_ok, "fallbacks": p.fallbacks, "ordered": p.ordered, "ok": ok})
        print(f"{'PASS' if ok else 'FAIL'} ambiguity/{f}: one parse (undocumented fallbacks {p.fallbacks}, documented ordered choices {p.ordered}){'' if gen_ok else ' ' + why[:80]}")
    print(f"ambiguity: {len(out['ambiguity'])} programs, {amb_fail} FAIL")
    # timing: warm process, median of 20
    slow = 0
    worst = 0.0
    for r in out["agreement"]:
        if not r["generated"]:
            continue
        path = next(x["path"] for x in syntax if x["id"] == r["id"])
        src = open(os.path.join(ROOT, path), encoding="utf-8").read()
        start = "module" if not src.startswith("{ ") else "block"
        sr = next(x for x in syntax if x["id"] == r["id"])
        start = "block" if "statements" in (sr.get("detail") or "") else "module"
        text = src if start == "module" else "{ " + src + "\n}\n"
        toks = lexer.lex(text)
        gen_parse(toks, start)
        samples = []
        for _ in range(20):
            t0 = time.perf_counter()
            gen_parse(toks, start)
            samples.append((time.perf_counter() - t0) * 1000)
        med = statistics.median(samples)
        worst = max(worst, med)
        slow += med >= 50
        out["timing"].append({"id": r["id"], "sha256": r["sha256"], "tokens": len(toks), "median_ms": round(med, 2), "p95_ms": round(sorted(samples)[18], 2)})
    print(f"timing: {len(out['timing'])} fixtures, slowest median {worst:.1f} ms, {slow} at or over 50 ms (conditions: warm process, median of 20, {out['platform']}, Python {out['python']})")
    print("timing budget: enforced" if enforce_timing else "timing budget: recorded only; FORTIS_PARSER_TIMING=1 judges it for v1.0 (X-149)")
    json.dump(out, open(os.path.join(ROOT, "conformance", "generated", "check.json"), "w", encoding="utf-8"), indent=1)
    return 1 if (disagreements or tree_diffs or amb_fail or (enforce_timing and slow)) else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
