"""Static checker runner (backend `checker`): names, declarations and visibility over every fixture
that parsed as a module, writing conformance/checker/results.json. Judgement: an accept or execute
fixture must produce no checker code; a reject fixture whose codes include checker codes must
produce each at its marked line and nothing else from the set; a reject fixture for another
layer's codes must produce none. Fragments and illustrative blocks are reported (INFO), not judged."""
import hashlib
import json
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
sys.path.insert(0, HERE)
sys.path.insert(0, ROOT)
import lexer      # noqa: E402
import names      # noqa: E402
import parser     # noqa: E402
from checker import check  # noqa: E402

SYNTAX_RESULTS = os.path.join(ROOT, "conformance", "results.json")
OUT = os.path.join(ROOT, "conformance", "checker", "results.json")


def main(argv):
    rec = parser.load_grammar(os.path.join(ROOT, "reference", "grammar", "fortis.ebnf"))
    lib = names.Library(rec)
    commit = subprocess.run(["git", "rev-parse", "--short", "HEAD"], capture_output=True, text=True, cwd=ROOT).stdout.strip()
    syntax = {r["id"]: r for r in json.load(open(SYNTAX_RESULTS, encoding="utf-8"))["results"]}
    only = [a for a in argv if not a.startswith("--")]
    results, failures, counts = [], 0, {"PASS": 0, "FAIL": 0, "INFO": 0, "SKIP": 0}
    for fid, sr in syntax.items():
        if only and fid not in only:
            continue
        path = sr["path"]
        src = open(os.path.join(ROOT, path), encoding="utf-8").read()
        v = sr["verdict"]
        if sr["status"] not in ("PASS", "INFO") or sr["detail"].startswith("template") or sr["detail"].startswith("parses as a list") \
                or any(c.startswith("E-SYN") for c in sr.get("codes", [])):
            counts["SKIP"] += 1
            continue
        fragment = "statements" in (sr.get("detail") or "")
        if fragment or v == "illustrative":
            counts["INFO"] += 1
            results.append(result(fid, path, sr, src, commit, "INFO", "fragment" if fragment else "illustrative", "not judged", []))
            continue
        try:
            records = check.check_source(lib, src, path)
        except Exception as e:  # noqa: BLE001
            records = [{"code": "E-INT-000", "message": f"checker failure: {e!r}", "line": 1, "column": 1}]
        judged = check.CODES | set(check.PARTIAL)
        mine = [r for r in records if r["code"] in judged or r["code"].startswith("E-INT")]
        codes = sorted({r["code"] for r in mine})
        expected_codes = [c for c in sr.get("codes", []) if c in judged]
        marked = [(m["line"], m["code"]) for m in sr.get("marked_lines", []) if m["code"] in judged]
        if v in ("accept", "execute") or (v == "reject" and not expected_codes):
            ok = not mine
            expected = "no checker code"
            observed = expected if ok else "reject:" + ",".join(codes)
        elif not mine and all(c in check.PARTIAL for c in expected_codes):
            counts["INFO"] += 1
            results.append(result(fid, path, sr, src, commit, "INFO", "reject:" + ",".join(expected_codes), "not judged at this layer: " + "; ".join(check.PARTIAL[c] for c in expected_codes), records))
            continue
        else:
            hit = all(any(r["code"] == c and r.get("line") == ln for r in mine) for ln, c in marked)
            ok = hit and all(c in codes for c in expected_codes) and all(c in expected_codes for c in codes)
            expected = "reject:" + ",".join(expected_codes) + ("" if not marked else " at " + ",".join(f"{ln}:{c}" for ln, c in marked))
            observed = ("reject:" + ",".join(f"{r.get('line')}:{r['code']}" for r in mine)) if mine else "none"
            if ok:
                observed = expected
        status = "PASS" if ok else "FAIL"
        counts[status] += 1
        failures += status == "FAIL"
        if status == "FAIL" and "--quiet" not in argv:
            print(f"FAIL {fid:<28} expected {expected}; observed {observed}")
            for r in records[:8]:
                print(f"       {r.get('line')}:{r.get('column', '?')} {r['code']} {r['message']}")
        results.append(result(fid, path, sr, src, commit, status, expected, observed, records))
    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    json.dump({"results": results}, open(OUT, "w", encoding="utf-8"), indent=2)
    print(f"\nchecker: {counts}")
    return 1 if failures else 0


def result(fid, path, sr, src, commit, status, expected, observed, records):
    return {"id": fid, "path": path, "verdict": sr["verdict"], "codes": sr.get("codes", []), "stage": sr.get("stage"), "packages": sr.get("packages", []),
            "status": status, "backend": "checker", "commit": commit, "sha256": hashlib.sha256(src.encode("utf-8")).hexdigest(),
            "command": "python3 -I conformance/tools/checker_check.py", "expected": expected, "observed": observed, "diagnostics": records}


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
