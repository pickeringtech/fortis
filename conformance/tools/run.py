"""G0 conformance runner: syntax layer.

For every fixture in conformance/corpus/manifest.json and every hand-written program in
conformance/programs/, lex and parse the source against reference/grammar/fortis.ebnf.

  accept / execute        must parse as a module                       -> PASS / FAIL
  reject: E-SYN-*         must fail to lex or parse                    -> PASS / FAIL
  reject: other code      must parse (the rejection is semantic, and a later layer's job)
  illustrative            parsed for information only: as a module, else as a statement
                          sequence, else reported as not parsing; never fails the run
                          unless it is a complete (non-elided) program that does not parse

Exit status is non-zero if any gating fixture fails. --json writes conformance/results.json.
"""
from __future__ import annotations

import json
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import hashlib    # noqa: E402
import subprocess # noqa: E402

import diagnostics  # noqa: E402
import extract    # noqa: E402
import lexer      # noqa: E402
import parser     # noqa: E402

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
GRAMMAR = os.path.join(ROOT, "reference", "grammar", "fortis.ebnf")
MANIFEST = os.path.join(ROOT, "conformance", "corpus", "manifest.json")
PROGRAMS = os.path.join(ROOT, "conformance", "programs")
_MARKER = re.compile(r"^//\s*(accept|reject|execute|illustrative)\b\s*[:—-]?\s*(.*)$")
_CODE = re.compile(r"E-[A-Z]{3}-\d{3}")


def _keyword_as_identifier(rec, toks, fail_idx, start):
    """E-SYN-002 (LR 3.4): returns the first keyword of the failing statement that stands where an
    identifier is required. Keywords in the statement are read as identifiers of their case, one
    at a time from the failure backwards, until the parse completes or passes the end of the
    statement; a statement may contain several offending keywords, so reclassification is
    cumulative (bounded)."""
    stmt_end = next((k for k in range(fail_idx, len(toks)) if toks[k].kind in ("NL", "EOF")), len(toks) - 1)
    stmt_start = fail_idx
    while stmt_start > 0 and toks[stmt_start - 1].kind != "NL":
        stmt_start -= 1
    alt = [parser.Token(t.kind, t.text, t.line, t.col, t.interps, t.closes_type_args) for t in toks]
    reclassified = []
    idx = fail_idx
    while True:                       # each round strictly advances the failure index, so this terminates
        progressed = False
        for cand in range(idx, stmt_start - 1, -1):
            if alt[cand].kind != "KW":
                continue
            saved = alt[cand].kind
            alt[cand].kind = "UPPER" if alt[cand].text[0].isupper() else "LOWER"
            probe = parser.Recogniser(rec.rules)
            try:
                probe.recognise(alt, start)
                new_idx = len(alt)
            except parser.ParseFailure as e2:
                new_idx = alt.index(e2.token)
            if new_idx > idx:
                reclassified.append(cand)
                idx = new_idx
                progressed = True
                break
            alt[cand].kind = saved
        if not progressed:
            return None
        if idx > stmt_end:
            return toks[min(reclassified)]
    return None


RECORDS = []   # diagnostic records produced during a run (written to conformance/diagnostics.jsonl)
CURRENT_FILE = [""]


def record_syntax(code, message, rule, line, col, fields, correction=None):
    r = diagnostics.make(code, message, rule, CURRENT_FILE[0], line, col, fields, correction=correction)
    RECORDS.append(r)
    return r


def try_parse(rec, src, start="module", line_offset=0):
    """Returns (ok, detail, code, line, col). Order: lex, parse (a non-sentence is E-SYN-000), E-SYN-004
    on the parse tree, then every string interpolation parsed as an expression (LR 3.6:
    it may not await or handle outcomes)."""
    try:
        toks = lexer.lex(src)
    except lexer.LexError as e:
        corr = None
        if e.code == "E-SYN-001":
            corr = {"kind": "delete the semicolon", "edits": [{"file": CURRENT_FILE[0], "line": e.line + line_offset, "column": e.col, "end_line": e.line + line_offset, "end_column": e.col + 1, "replacement": ""}]}
        record_syntax(e.code, str(e).split(": ", 1)[1].rsplit(" at ", 1)[0], "LANG-03-01" if e.code == "E-SYN-001" else ("LANG-03-04" if e.code == "E-SYN-002" else "LANG-03-02"),
                      e.line + line_offset, e.col, {"expected_tokens": [], "found_token": str(e).rsplit(" at ", 1)[0].split("'")[-2] if "'" in str(e) else ""}, corr)
        return False, f"lex: {e}", e.code, e.line + line_offset, e.col
    try:
        tree = rec.parse(toks, start)
    except parser.ParseFailure as e:
        code = "E-SYN-000"
        why = f"parse: {e}"
        kw = _keyword_as_identifier(rec, toks, toks.index(e.token), start)
        if kw is not None:
            record_syntax("E-SYN-002", f"keyword {kw.text!r} used as an identifier", "LANG-03-04", kw.line + line_offset, kw.col,
                          {"expected_tokens": ["lowerIdent"], "found_token": kw.text})
            return False, f"E-SYN-002: keyword {kw.text!r} used as an identifier at {kw.line}:{kw.col}", "E-SYN-002", kw.line + line_offset, kw.col
        record_syntax("E-SYN-000", f"unexpected {e.token.kind} {e.token.text!r}", "LANG-03-02", e.token.line + line_offset, e.token.col,
                      {"expected_tokens": sorted(e.expected)[:12], "found_token": e.token.text})
        return False, why, code, e.token.line + line_offset, e.token.col
    mixed = parser.check_operator_mixing(tree)
    if mixed is not None:
        bad, first = mixed
        corr = {"kind": "parenthesise the left operand", "edits": [
            {"file": CURRENT_FILE[0], "line": first.line + line_offset, "column": first.col, "end_line": first.line + line_offset, "end_column": first.col, "replacement": "("},
            {"file": CURRENT_FILE[0], "line": bad.line + line_offset, "column": bad.col, "end_line": bad.line + line_offset, "end_column": bad.col, "replacement": ") "}]}
        record_syntax("E-SYN-004", f"operator {bad.text!r} mixes operator families in one unparenthesised expression", "LANG-05-06",
                      bad.line + line_offset, bad.col, {"expected_tokens": ["parentheses around one operand"], "found_token": bad.text}, corr)
        return False, f"E-SYN-004: mixed outcome, ?? and logical operators at {bad.line}:{bad.col}", "E-SYN-004", bad.line + line_offset, bad.col
    for t in toks:
        if t.kind != "STR":
            continue
        for isrc, iline, icol in t.interps:
            ok, why, code, line, col = try_parse(rec, isrc, "expr", line_offset=line_offset + iline - 1)
            if not ok:
                return False, f"interpolation `${{{isrc}}}`: {why}", code, line, col
            bad = parser.find_suspend_or_outcome(rec.parse(lexer.lex(isrc), "expr"))
            if bad is not None:
                record_syntax("E-SYN-000", "an interpolated expression may not await or handle an outcome", "LANG-03-06", iline + line_offset, icol,
                              {"expected_tokens": [], "found_token": bad.text})
                return False, f"interpolation `${{{isrc}}}` may not await or handle an outcome (LANG-03-06)", "E-SYN-000", iline + line_offset, icol
    return True, "", None, None, None


def judge(rec, src):
    """Module first, then as a function body; returns (ok, why, code, line, col, form) and leaves
    RECORDS holding only the records of the interpretation that is reported."""
    before = list(RECORDS)
    ok, why, code, line, col = try_parse(rec, src)
    module_records = RECORDS[len(before):]
    form = "module"
    if not ok:
        del RECORDS[len(before):]
        ok2, why2, code2, line2, col2 = try_parse(rec, "{ " + src + "\n}\n", "block")
        block_records = RECORDS[len(before):]
        if ok2:
            ok, form = True, "statements"
            del RECORDS[len(before):]
        elif (line2 or 0, col2 or 0) > (line or 0, col or 0):
            why, code, line, col = why2, code2, line2, col2       # block interpretation got further: keep its records
        else:
            del RECORDS[len(before):]
            RECORDS.extend(module_records)
    return ok, why, code, line, col, form


def programs():
    out = []
    if not os.path.isdir(PROGRAMS):
        return out
    for name in sorted(os.listdir(PROGRAMS)):
        if not name.endswith(".ft"):
            continue
        path = os.path.join(PROGRAMS, name)
        body = open(path, encoding="utf-8").read().split("\n")
        m = _MARKER.match(body[0].strip())
        verdict, note = (m.group(1), m.group(2)) if m else ("unmarked", "")
        marked = extract.inline_markers(body)
        codes = _CODE.findall(note)
        for ml in marked:
            if ml["code"] not in codes:
                codes.append(ml["code"])
        packages = extract.kit_packages(body)
        out.append({
            "id": "PRG-" + name[:-3], "doc": "conformance/programs", "section": "-", "doc_line": 1,
            "verdict": verdict, "codes": codes, "marked_lines": marked, "note": note,
            "path": os.path.relpath(path, ROOT), "elided": False,
            "stage": "kit" if packages else "core", "packages": packages,
        })
    return out


def main(argv):
    rec = parser.load_grammar(GRAMMAR)
    fixtures = json.load(open(MANIFEST, encoding="utf-8"))["fixtures"] + programs()
    try:
        commit = subprocess.run(["git", "rev-parse", "--short", "HEAD"], capture_output=True, text=True, cwd=ROOT).stdout.strip() or None
    except OSError:
        commit = None
    results = []
    all_records = []
    failures = 0
    for fx in fixtures:
        src = open(os.path.join(ROOT, fx["path"]), encoding="utf-8").read()
        v = fx["verdict"]
        CURRENT_FILE[0] = fx["path"]
        del RECORDS[:]
        ok, why, code, line, col, form = judge(rec, src)
        status, detail = "", ""
        if v in ("accept", "execute"):
            status = "PASS" if ok else "FAIL"
            detail = form if ok else why
        elif v == "reject":
            syntactic = any(c.startswith("E-SYN") for c in fx["codes"])
            if syntactic:
                got = code or ("E-SYN-000" if not ok else None)
                want = [c for c in fx["codes"] if c.startswith("E-SYN")]
                matched = (not ok) and got in want
                where = [m["line"] for m in fx["marked_lines"] if m["code"] == got]
                if matched and where and line not in where:
                    matched = False
                    why = f"{got} reported at line {line}, but the marked line is {where}: {why}"
                status = "PASS" if matched else "FAIL"
                detail = why if matched else (f"parsed, but {','.join(want)} was expected" if ok else f"rejected with {got}, but {','.join(want)} was expected: {why}")
            else:
                status = "PASS" if ok else "FAIL"
                detail = (form + "; semantic rejection " + ",".join(fx["codes"]) + " is not checked at this layer") if ok else why
        elif v == "illustrative":
            if not ok:
                # a list of types or of function heads, one per line
                for alt_start in ("type", "funcHead"):
                    body = [l for l in src.split("\n")[1:] if l.strip() and not l.strip().startswith("//")]
                    if body and all(try_parse(rec, l.split("//")[0].strip() + "\n", alt_start)[0] for l in body):
                        ok, form = True, "a list of " + ("types" if alt_start == "type" else "function heads")
                        break
            if ok:
                status, detail = "INFO", "parses as " + form
            elif fx["elided"] or src.split("\n")[1:2] and src.split("\n")[1].startswith("["):
                status, detail = "INFO", "template or elided fragment; not parsed"
            else:
                status, detail = "WARN", why
        else:
            status, detail = "FAIL", "unmarked example"
        if status == "FAIL":
            failures += 1
        fixture_records = [r for r in RECORDS if r["file"] == fx["path"]]
        all_records.extend(fixture_records)
        results.append({**fx, "status": status, "detail": detail, "backend": "syntax", "commit": commit, "diagnostics": fixture_records,
                        "sha256": hashlib.sha256(src.encode("utf-8")).hexdigest(),
                        "command": "python3 -I conformance/tools/run.py --json",
                        "expected": v if v != "reject" else "reject:" + ",".join(c for c in fx["codes"] if c.startswith("E-SYN")),
                        "observed": (v if v != "reject" else "reject:" + ",".join(c for c in fx["codes"] if c.startswith("E-SYN"))) if status in ("PASS", "INFO") else (("accept" if ok else "reject:" + (code or "E-SYN-000")))})
        print(f"{status:4} {fx['id']:<12} {v:<12} {detail}")
    counts = {}
    for r in results:
        counts[r["status"]] = counts.get(r["status"], 0) + 1
    print("\n" + ", ".join(f"{k} {v}" for k, v in sorted(counts.items())))
    if "--json" in argv:
        with open(os.path.join(ROOT, "conformance", "results.json"), "w", encoding="utf-8") as f:
            json.dump({"grammar": os.path.relpath(GRAMMAR, ROOT), "results": results}, f, indent=2)
            f.write("\n")
        with open(os.path.join(ROOT, "conformance", "diagnostics.jsonl"), "w", encoding="utf-8") as f:
            for r in all_records:
                f.write(diagnostics.to_json_line(r) + "\n")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
