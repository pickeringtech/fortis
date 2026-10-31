"""Run each pinned release showcase, including its tests, on all three backends.

Ordinary examples are inspected by ir/view/fmt checks; this release fixture also needs behavioural evidence.
Reuse docset's two-checker, output and test judgements, without its quick-tier backend reduction or known list.
A .stdout file pins each program's output. The v0.1 guide must contain exactly the same complete program.
"""
from __future__ import annotations

import json
from pathlib import Path
import sys

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import docset_check as docset  # noqa: E402

ROOT = HERE.parent.parent


def main():
    programs = sorted((ROOT / "examples/showcase").glob("*.ft"))
    problems, rows = [], []
    if not programs:
        problems.append("no pinned showcase programs")
    Path(docset.WORK, "bin").mkdir(parents=True, exist_ok=True)
    for program in programs:
        source = program.read_text(encoding="utf-8")
        output = program.with_suffix(".stdout")
        if not output.is_file():
            problems.append(f"{program.name}: missing .stdout")
            continue
        expected = output.read_text(encoding="utf-8")
        relative = program.relative_to(ROOT).as_posix()
        if not source.splitlines() or source.splitlines()[0] != "// accept":
            problems.append(f"{relative}: first line must be // accept")
        if not docset.has_main(source) or not docset.has_tests(source):
            problems.append(f"{relative}: a showcase needs main and tests")
        if program.name == "v0.1.0-coffee.ft":
            page = ROOT / "docs/guide/worked-example.md"
            blocks = docset.fences(page.read_text(encoding="utf-8"))
            matches = [b for b in blocks if b["lang"] == "fortis" and "\n".join(b["lines"]) + "\n" == source]
            if len(matches) != 1:
                problems.append("coffee guide: must contain the complete pinned program exactly once")
            elif not matches[0].get("next") or matches[0]["next"]["lang"] != "output" or "\n".join(matches[0]["next"]["lines"]) + "\n" != expected:
                problems.append("coffee guide: shown output differs from the pinned .stdout")
        row = docset.judge({"id": relative, "page": relative, "line": 1, "verdict": "accept",
                            "target": str(program), "display": program.name, "src": source,
                            "expect": ("output", expected.removesuffix("\n")), "exit": 0})
        rows.append(row)
        print(f"{'PASS' if row['ok'] else 'FAIL'} {relative}")
        for check in row["checks"]:
            if not check["ok"]:
                print(f"  {check['tool']}: {check['check']}: {check['detail']}")
    for problem in problems:
        print(f"FAIL {problem}")
    if "--json" in sys.argv:
        (ROOT / "build/showcase.json").write_text(json.dumps({"problems": problems, "fixtures": rows}, indent=1) + "\n")
    ok = not problems and all(r["ok"] for r in rows)
    print(f"showcase: {'PASS' if ok else 'FAIL'} ({len(rows)} programs; both checkers; interpreter, VM, native; tests)")
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
