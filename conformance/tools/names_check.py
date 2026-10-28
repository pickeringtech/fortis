"""DEL-06 runner: resolve names in every fixture (corpus + programs) and write
conformance/names/results.json (backend `names`). Judgement: an accept fixture must produce no
E-NAM record; a reject fixture whose code is E-NAM-xxx must produce it; any other fixture must
produce no E-NAM record (its rejection is another layer's). Illustrative blocks are reported only.
Fragments are resolved as statement sequences when they are not modules."""
import hashlib
import json
import os
import subprocess
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import lexer      # noqa: E402
import names      # noqa: E402
import parser     # noqa: E402

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
MANIFEST = os.path.join(ROOT, "conformance", "corpus", "manifest.json")
SYNTAX_RESULTS = os.path.join(ROOT, "conformance", "results.json")


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
        form = sr.get("observed") if sr.get("status") in ("PASS", "INFO") else None
        if sr["status"] not in ("PASS", "INFO") or sr["detail"].startswith("template") or sr["detail"].startswith("parses as a list") \
                or any(c.startswith("E-SYN") for c in sr.get("codes", [])):     # a correct syntax rejection has nothing to resolve
            counts["SKIP"] += 1
            continue
        start = "module" if "statements" not in (sr.get("detail") or "") else "block"
        fragment = start == "block"
        try:
            records = names.resolve_source(lib, src if start == "module" else "{ " + src + "\n}\n", path, start)
        except Exception as e:  # noqa: BLE001
            records = [{"code": "E-INT-000", "message": f"resolver failure: {e}", "line": 1}]
        nam = [r for r in records if r["code"].startswith("E-NAM")]
        codes = sorted({r["code"] for r in nam})
        # resolution codes (E-NAM-034: a head whose kit is not installed, X-231); E-NAM-002 is typing
        expected_nam = [c for c in sr.get("codes", []) if c in ("E-NAM-001", "E-NAM-003", "E-NAM-010", "E-NAM-034")]
        if v == "illustrative" or (fragment and not expected_nam):
            # a statement fragment has free names by construction: reported, never judged (LR 0)
            status = "INFO"
            expected, observed = ("illustrative" if v == "illustrative" else "fragment"), ("resolves" if not nam else "unresolved:" + ",".join(codes))
        elif expected_nam:
            ok = all(c in codes for c in expected_nam)
            status = "PASS" if ok else "FAIL"
            expected, observed = "reject:" + ",".join(expected_nam), "reject:" + ",".join(codes) if codes else "resolves"
        else:
            ok = not nam and not any(r["code"].startswith("E-INT") for r in records)
            status = "PASS" if ok else "FAIL"
            expected, observed = "resolves", ("resolves" if ok else "unresolved:" + ",".join(codes or [r["code"] for r in records]))
        counts[status] += 1
        failures += status == "FAIL"
        if status != "PASS" and "--quiet" not in argv:
            print(f"{status:4} {fid:<28} {observed}")
            for r in records[:6]:
                print(f"       {r.get('line')}:{r.get('column','?')} {r['code']} {r['message']}")
        results.append({"id": fid, "path": path, "verdict": v, "codes": sr.get("codes", []), "stage": sr.get("stage"), "packages": sr.get("packages", []),
                        "status": status, "backend": "names", "commit": commit, "sha256": hashlib.sha256(src.encode("utf-8")).hexdigest(),
                        "command": "python3 -I conformance/tools/names_check.py", "expected": expected, "observed": observed, "diagnostics": records})
    json.dump({"results": results}, open(os.path.join(ROOT, "conformance", "names", "results.json"), "w", encoding="utf-8"), indent=2)
    print(f"\nnames: {counts}")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
