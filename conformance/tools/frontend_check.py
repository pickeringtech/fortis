"""The production frontend (compiler/, Rust) against the reference checker (checker/, Python): DEL-31 onwards.

Every module fixture of the corpus and every package fixture is checked by both; an observation is the set of
`code@line` records restricted to the codes the reference judges (checker/check.py CODES and PARTIAL; a syntax
fixture is judged by the syntax runner's record). A package fixture whose expect.json pins its complete records
(`records`: file, line, column, code, message, candidates) must give exactly those in both checkers. A fixture's completing chunk is the latest completing chunk of the
rules its header `covers` and the codes it expects (docs/delivery/ownership.json); `--slice DEL-13` selects the
fixtures complete by that chunk, which must agree exactly (DEL-31 criterion 1). Every other fixture is compared too and
reported, so later slices see what remains.

Also compared on every fixture: the declaration IR (checker/decl_ir.py against `fortisc decls`), the control-flow
graph of every block-bodied function (checker/cfg.py against `fortisc cfg`), and the parse tree (the generated
Python parser and the recogniser against `fortisc parse`, the ambiguity corpus included); every frontend record is
validated against the DEL-03 schema (diagnostics.validate); `--binary` names another build of fortisc (the debug
build), whose observations must equal the release build's (PROG-VM-04).

  python3 -I conformance/tools/frontend_check.py [--slice DEL-13] [--binary compiler/target/debug/fortisc] [--all]
Writes build/rust-frontend/results.json (fixture -> {reference, frontend, slice, agree}) and the summary."""
from __future__ import annotations

import json
import os
import re
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
sys.path.insert(0, HERE)
sys.path.insert(0, ROOT)
FORTISC = os.path.join(ROOT, "compiler", "target", "release", "fortisc")
OUT = os.path.join(ROOT, "build", "rust-frontend", "results.json")


def chunk_num(c):
    """A core chunk's number; a kit chunk (`DEL-40s`) or none is in no numeric slice."""
    n = c.split("-")[1] if c and c.startswith("DEL-") else ""
    return int(n) if n.isdigit() else 999


def completing(src: str, codes, own) -> int:
    m = re.search(r"^// covers: (.*)$", src, re.M)
    covers = re.findall(r"LANG-\d\d-\d\d", m.group(1)) if m else []
    owners = [own["rules"].get(r, {}).get("completes") for r in covers] + [own["diagnostics"].get(c) for c in codes]
    owners = [o for o in owners if o]
    return max((chunk_num(o) for o in owners), default=None) if owners else None


def rust_sections(cmd, paths, binary=None):
    """`fortisc <cmd>` output split at its `=== key` lines."""
    r = subprocess.run([binary or FORTISC, cmd] + paths, capture_output=True, text=True, cwd=ROOT, timeout=3600)
    out, cur = {}, None
    for line in r.stdout.split("\n"):
        if line.startswith("=== "):
            cur = line[4:]
            out[cur] = []
        elif cur is not None:
            out[cur].append(line)
    return {k: "\n".join(v).rstrip("\n") for k, v in out.items()}


def rust_records(paths, binary=None):
    out = {}
    r = subprocess.run([binary or FORTISC, "check", "--json"] + paths, capture_output=True, text=True, cwd=ROOT, timeout=3600)
    cur = None
    for line in r.stdout.splitlines():
        if line.startswith("=== "):
            cur = line[4:]
            out[cur] = []
        elif line.startswith("{") and cur is not None:
            out[cur].append(json.loads(line))
    return out


def main(argv):
    import names
    import parser as fparser
    import packages_check
    import resolve
    from checker import check
    slice_to = chunk_num(argv[argv.index("--slice") + 1]) if "--slice" in argv else 13
    own = json.load(open(os.path.join(ROOT, "docs", "delivery", "ownership.json"), encoding="utf-8"))
    syntax = json.load(open(os.path.join(ROOT, "conformance", "results.json"), encoding="utf-8"))["results"]
    rec = fparser.load_grammar(os.path.join(ROOT, "reference", "grammar", "fortis.ebnf"))
    lib = names.Library(rec)
    judged = check.CODES | set(check.PARTIAL)
    fixtures = []
    for r in syntax:
        if r["verdict"] == "illustrative" or "statements" in (r.get("detail") or "") or r["status"] not in ("PASS", "INFO"):
            continue
        fixtures.append(r)
    paths = [r["path"] for r in fixtures]
    pkg_dir = os.path.join(ROOT, "conformance", "packages")
    pkgs = sorted(d for d in os.listdir(pkg_dir) if os.path.exists(os.path.join(pkg_dir, d, "expect.json")))
    cor_dir = os.path.join(ROOT, "conformance", "corrections")
    corrections = sorted(os.path.join("conformance", "corrections", f) for f in os.listdir(cor_dir) if f.endswith(".ft")) if os.path.isdir(cor_dir) else []
    rust = rust_records(paths + [os.path.relpath(os.path.join(pkg_dir, d), ROOT) for d in pkgs] + corrections)
    results = {}
    pinned_bad = []      # package fixtures whose pinned records either checker misses: a failure in any slice

    def obs(records, syn=False):
        keep = [x for x in records if x.get("code") and (x["code"] in judged or x["code"].startswith("E-INT") or x["code"].startswith("E-SYN") or syn)]
        return sorted({f"{x['code']}@{x.get('line')}" for x in keep})

    for r in fixtures:
        src = open(os.path.join(ROOT, r["path"]), encoding="utf-8").read()
        syn = [d for d in r.get("diagnostics", []) if d["code"].startswith("E-SYN")]
        if r["observed"].startswith("reject:E-SYN") and syn:
            ref = [f"{syn[0]['code']}@{syn[0]['line']}"]
        else:
            try:
                ref = obs(check.check_source(lib, src, r["path"]))
            except Exception as e:  # noqa: BLE001
                ref = [f"E-INT-000@1 {e!r}"[:80]]
        mine = obs(rust.get(r["path"], [{"code": "E-INT-001", "line": 0}]))
        if r["observed"].startswith("reject:E-SYN"):
            mine = [m for m in mine if m.startswith("E-SYN")][:1]
        cc = completing(src, r.get("codes", []), own)
        results[r["id"]] = {"path": r["path"], "reference": ref, "frontend": mine, "completing": cc, "slice": cc is not None and cc <= slice_to, "agree": ref == mine}
    for path in corrections:   # the pinned fixtures of corrections_check.py
        src = open(os.path.join(ROOT, path), encoding="utf-8").read()
        ref = obs(check.check_source(lib, src, path))
        mine = obs(rust.get(path, [{"code": "E-INT-001", "line": 0}]))
        results["COR-" + os.path.basename(path)] = {"path": path, "reference": ref, "frontend": mine, "completing": None, "slice": False, "agree": ref == mine}
    for d in pkgs:
        full = os.path.join(pkg_dir, d)
        expect = json.load(open(os.path.join(full, "expect.json"), encoding="utf-8"))
        try:
            ps = resolve.PackageSet(full, pkg_dir, rec)
            recs = ps.run() or check.check_package_set(ps)
        except Exception as e:  # noqa: BLE001
            recs = [{"code": "E-NAM-001", "line": 1, "message": str(e)}]
        ref = sorted({f"{x['code']}@{x.get('line')}" for x in recs if x.get("code")})
        mine = sorted({f"{x['code']}@{x.get('line')}" for x in rust.get(os.path.relpath(full, ROOT), []) if x.get("code")})
        if "records" in expect:
            # a fixture that pins its complete records holds both checkers to them (packages_check.py `pinned`)
            want = packages_check.pinned(expect["records"])
            for who, got in (("reference", packages_check.pinned(recs)), ("frontend", packages_check.pinned(rust.get(os.path.relpath(full, ROOT), [])))):
                if got != want:
                    print(f"PINNED RECORDS DIFFER PKG-{d} ({who}): {got}")
                    (ref if who == "reference" else mine).append(f"{who}: pinned records differ")      # never agrees
                    pinned_bad.append(f"PKG-{d} ({who})")
        covers = expect.get("covers", [])
        owners = [own["rules"].get(c, {}).get("completes") for c in covers] + ([own["diagnostics"].get(expect["code"])] if expect.get("code") else [])
        owners = [o for o in owners if o]
        cc = max((chunk_num(o) for o in owners), default=None) if owners else None
        results["PKG-" + d] = {"path": os.path.relpath(full, ROOT), "reference": ref, "frontend": mine, "completing": cc, "slice": cc is not None and cc <= slice_to, "agree": ref == mine}
    # ---- the declaration IR, the CFGs and the parse trees -------------------------------------
    import diagnostics
    import lexer
    sys.path.insert(0, os.path.join(ROOT, "conformance", "generated"))
    import fortis_parser
    import tree as treemod
    from checker import cfg as cfgmod
    from checker import decl_ir
    from checker.decls import DeclChecker
    module_paths = [r["path"] for r in fixtures if not r["observed"].startswith("reject:E-SYN")]
    rust_ir = rust_sections("decls", module_paths)
    rust_sum = rust_sections("summaries", module_paths)
    from checker import effects as fxmod
    sum_bad, sum_n = [], 0
    rust_cfg = rust_sections("cfg", module_paths)
    amb = sorted(os.path.join("conformance", "ambiguity", f) for f in os.listdir(os.path.join(ROOT, "conformance", "ambiguity")) if f.endswith(".ft"))
    rust_parse = rust_sections("parse", module_paths + amb)
    ir_bad, cfg_bad, cfg_n, parse_bad, parse_n = [], [], 0, [], 0

    def pydump(t, d, out):
        if isinstance(t, lexer.Token):
            out.append("  " * d + f"{t.kind} {t.text!r} {t.line}:{t.col}")
            return
        out.append("  " * d + t[0])
        for c in t[1]:
            pydump(c, d + 1, out)

    for path in module_paths + amb:
        src = open(os.path.join(ROOT, path), encoding="utf-8").read()
        try:
            mt = rec.parse(lexer.lex(src), "module")
        except Exception:  # noqa: BLE001
            continue
        lines = []
        pydump(mt, 0, lines)
        gen = fortis_parser.FortisParser(lexer.lex(src)).parse("module")
        glines = []
        pydump(gen, 0, glines)
        parse_n += 1
        if rust_parse.get(path) != "\n".join(lines) or "\n".join(glines) != "\n".join(lines):
            parse_bad.append(path)
        if path in amb:
            continue
        check.check_tree(lib, mt, path)
        sum_n += len(check.LAST_TYPER.summaries)
        if rust_sum.get(path, "").rstrip("\n") != fxmod.summaries_text(check.LAST_TYPER.summaries).rstrip("\n"):
            sum_bad.append(path)
        dc = DeclChecker(lib, path, "main", "")
        dc.load(mt)
        dc.check()
        if rust_ir.get(path, "").rstrip("\n") != decl_ir.dump(dc).rstrip("\n"):
            ir_bad.append(path)
        for d in treemod.declarations(mt):
            if d["kind"] != "func":
                continue
            g = cfgmod.build(d["node"], d["name"])
            if g is None:
                continue
            cfg_n += 1
            if rust_cfg.get(f"{path} {d['name']}", "").rstrip("\n") != cfgmod.dump(g).rstrip("\n"):
                cfg_bad.append(f"{path}:{d['name']}")
    for k in ir_bad[:20]:
        print(f"IR DIFFERS {k}")
    for k in cfg_bad[:20]:
        print(f"CFG DIFFERS {k}")
    for k in sum_bad[:20]:
        print(f"SUMMARIES DIFFER {k}")
    for k in parse_bad[:20]:
        print(f"TREE DIFFERS {k}")
    # ---- records against the DEL-03 schema ------------------------------------------------------
    invalid = []
    for path, recs in rust.items():
        for x in recs:
            errs = diagnostics.validate(x)
            if errs:
                invalid.append(f"{path}: {x.get('code')} {errs}")
    for k in invalid[:20]:
        print(f"INVALID RECORD {k}")
    # ---- another build (debug) --------------------------------------------------------------------
    build_diff = None
    if "--binary" in argv:
        other = rust_records(paths + [os.path.relpath(os.path.join(pkg_dir, d), ROOT) for d in pkgs] + corrections, argv[argv.index("--binary") + 1])
        norm = lambda m: {k: sorted(json.dumps(x, sort_keys=True) for x in v) for k, v in m.items()}
        build_diff = sorted(k for k in set(norm(rust)) | set(norm(other)) if norm(rust).get(k) != norm(other).get(k))
        for k in build_diff[:20]:
            print(f"BUILD DIFFERS {k}")
    sl = {k: v for k, v in results.items() if v["slice"]}
    bad_slice = sorted(k for k, v in sl.items() if not v["agree"])
    bad_all = sorted(k for k, v in results.items() if not v["agree"])
    for k in bad_slice:
        print(f"SLICE DISAGREE {k}: reference {results[k]['reference']} frontend {results[k]['frontend']}")
    if "--all" in argv:
        for k in bad_all:
            if k not in bad_slice:
                print(f"disagree {k}: reference {results[k]['reference']} frontend {results[k]['frontend']}")
    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    summary = {"slice": [len(sl), len(bad_slice)], "corpus": [len(results), len(bad_all)], "decl_ir": [len(module_paths), len(ir_bad)], "cfg": [cfg_n, len(cfg_bad)],
               "parse": [parse_n, len(parse_bad)], "ambiguity": len(amb), "records": sum(len(v) for v in rust.values()), "invalid_records": len(invalid),
               "builds": None if build_diff is None else [len(rust), len(build_diff)], "pinned": pinned_bad}
    json.dump({"slice_to": f"DEL-{slice_to:02d}", "summary": summary, "results": results}, open(OUT, "w", encoding="utf-8"), indent=1)
    print(f"slice DEL-01..DEL-{slice_to:02d}: {len(sl)} fixtures, {len(sl) - len(bad_slice)} agree, {len(bad_slice)} disagree")
    print(f"whole corpus: {len(results)} fixtures, {len(results) - len(bad_all)} agree, {len(bad_all)} disagree")
    print(f"declaration IR: {len(module_paths)} modules, {len(ir_bad)} differ; CFGs: {cfg_n} functions, {len(cfg_bad)} differ")
    print(f"effect summaries: {sum_n} function summaries in {len(module_paths)} modules, {len(sum_bad)} modules differ")
    print(f"parse trees: {parse_n} modules ({len(amb)} of the ambiguity corpus), {len(parse_bad)} differ from the generated parser or the recogniser")
    print(f"records: {summary['records']} from the frontend, {len(invalid)} schema-invalid")
    if build_diff is not None:
        print(f"builds: {len(rust)} fixtures, {len(build_diff)} differ between the release and the other build")
    summary["summaries"] = [sum_n, len(sum_bad)]
    if pinned_bad:
        print(f"pinned records: {len(pinned_bad)} differ ({', '.join(pinned_bad)})")
    failed = bad_slice or ir_bad or cfg_bad or parse_bad or invalid or build_diff or sum_bad or pinned_bad
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
