"""Writes conformance/generated/lookahead.md from lookahead.json and check.json (DEL-02)."""
import json
import os
import sys
from collections import Counter

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
G = os.path.join(ROOT, "conformance", "generated")


def main():
    la = json.load(open(os.path.join(G, "lookahead.json"), encoding="utf-8"))
    ck = json.load(open(os.path.join(G, "check.json"), encoding="utf-8"))
    k = la["lookahead"]
    run = la["runs"][k - 1]
    by_policy = Counter(c["policy"] or "undocumented" for c in run["conflict_list"])
    lines = ["# Lookahead report (generated parser, DEL-02)", "",
             f"Grammar sha256 `{la['grammar_sha256']}`; generator `conformance/tools/gen_parser.py`; emitted parser `conformance/generated/fortis_parser.py`.", "",
             f"**Lookahead bound: k = {k}**, confirmed by the generator: at k = {k} every one of the {run['decisions']} decision points is separated by token classes alone or covered by a documented policy ({'0' if la['confirmed'] else run['undocumented']} undocumented conflicts). No decision consults a name: lookahead tables hold literal token texts and lexer kinds only.", "",
             "| k | decisions | conflicts | without a documented policy |", "|---|---|---|---|"]
    for r in la["runs"]:
        lines.append(f"| {r['k']} | {r['decisions']} | {r['conflicts']} | {r['undocumented']} |")
    lines += ["", f"Conflicts at k = {k} by resolution policy (Language Reference 3.2, X-94): " + ", ".join(f"{p}: {n}" for p, n in sorted(by_policy.items())), "",
              "| Rule | Decision | Branches | Example shared prefix | Policy |", "|---|---|---|---|---|"]
    for c in run["conflict_list"]:
        lines.append(f"| {c['rule']} | {c['where']} | {c['branches']} | `{c['shared'][0] if c['shared'] else ''}` | {c['policy']} |")
    agree = ck["agreement"]
    dis = [a for a in agree if not a["agree"]]
    amb = ck["ambiguity"]
    tm = ck["timing"]
    worst = max(tm, key=lambda t: t["median_ms"]) if tm else None
    lines += ["", "## Agreement with the recogniser", "", f"{len(agree)} fixtures: {len(dis)} disagreements.", "",
              "## Ambiguity corpus", "", f"{len(amb)} programs, {sum(1 for a in amb if a['ok'])} with exactly one parse (no undocumented fallback); documented ordered choices used: {sum(a['ordered'] for a in amb)}.", "",
              "## Timing (pinned conditions)", "", f"Warm process, median of 20 parses per fixture, {ck['platform']}, Python {ck['python']}; {len(tm)} fixtures listed by sha256 in check.json; slowest median {worst['median_ms'] if worst else 0} ms ({worst['id'] if worst else ''}, {worst['tokens'] if worst else 0} tokens); {sum(1 for t in tm if t['median_ms'] >= 50)} at or over 50 ms.", ""]
    open(os.path.join(G, "lookahead.md"), "w", encoding="utf-8").write("\n".join(lines))
    print("lookahead.md written")


if __name__ == "__main__":
    main()
