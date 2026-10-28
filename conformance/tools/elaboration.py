"""The Dependency injection kit's elaboration as the reference toolchain reads it (SP-08; D-90).

A module that uses the kit's syntax is checked as `fortisc elaborate` writes it, and its core records are located
through the elaboration's line origins; a module whose elaboration fails is reported by the kit's findings alone.
These mirror fortis-di's `scan::uses_di`, `source` and `report`, so that the reference checker reads every module,
alone or in a package, exactly as the production frontend does: the core exempts the kit's `context` head only
because the kit elaborates and validates it before the check."""
from __future__ import annotations

import json
import os
import subprocess
from typing import Dict, List, Optional, Tuple

import lexer

ROOT = os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
FORTISC = os.path.join(ROOT, "compiler", "target", "release", "fortisc")
HEAD_PREFIX = ("export", "server", "client", "hybrid")


def uses_di(text: str) -> bool:
    """Whether a module uses the kit's syntax, read as tokens so that words in strings and comments do not count:
    an `inject` member, or a `context` head (first on its line after `export` and a prefix, a name after it)."""
    if "inject" not in text and "context" not in text:
        return False
    try:
        toks = lexer.lex(text)
    except Exception:  # noqa: BLE001  (text that does not lex is read line by line)
        return _uses_di_lines(text)
    word = ("KW", "LOWER")
    for i, t in enumerate(toks):
        if t.kind in word and t.text == "inject":
            return True
        if t.kind in word and t.text == "context":
            j = i
            while j > 0 and toks[j - 1].line == t.line and toks[j - 1].text in HEAD_PREFIX:
                j -= 1
            first = j == 0 or toks[j - 1].line < t.line or toks[j - 1].kind in ("NL", "NEWLINE")
            nxt = toks[i + 1] if i + 1 < len(toks) else None
            if first and nxt is not None and nxt.line == t.line and nxt.kind in ("UPPER", "LOWER"):
                return True
    return False


def _uses_di_lines(text: str) -> bool:
    for line in text.split("\n"):
        code = line.split("//")[0]
        t = code.lstrip()
        for w in ("export ", "server ", "client ", "hybrid "):
            if t.startswith(w):
                t = t[len(w):].lstrip()
        if t.startswith("context ") or ("inject" in code.replace("{", " ").split() and "inject " in code):
            return True
    return False


def elaborate(path: str) -> Tuple[str, List[dict], Optional[List[int]]]:
    """`fortisc elaborate --origins PATH` (PATH as the records should name it, relative to the repository root):
    (the elaborated text, the kit's records, the source line of each elaborated line)."""
    r = subprocess.run([FORTISC, "elaborate", "--origins", path], capture_output=True, text=True, cwd=ROOT)
    errs = [json.loads(line) for line in r.stderr.splitlines() if line.startswith("{")]
    origins = next((e["origins"] for e in errs if "origins" in e), None)
    return (r.stdout[:-1] if r.stdout.endswith("\n") else r.stdout), [e for e in errs if "code" in e], origins


def remap(recs: List[dict], origins: Optional[List[int]], source_lines: int) -> List[dict]:
    """A record found in generated code moves to the declaration it was generated from (fortis-di `remap`)."""
    if not origins:
        return recs
    for r in recs:
        line = r.get("line", 0)
        if 1 <= line <= len(origins):
            if line > source_lines:
                r.setdefault("fields", {})["generated_line"] = line
                r["column"] = 1
            r["line"] = origins[line - 1]
    return recs


def report(records: List[dict], modules) -> List[dict]:
    """The kit's records joined to the core's for a set of modules (each with `file`, `di_records`, `origins`,
    `source_lines`), as fortis-di's `report` joins them: a module whose elaboration failed keeps only its kit
    findings; the core records of the others are remapped through their origins."""
    # the kit's findings in one order, by file, line and column, whatever order the modules were read in
    di = sorted((r for m in modules for r in m.di_records), key=lambda r: (r.get("file", ""), r.get("line", 0), r.get("column", 0)))
    failed = {r["file"] for r in di}
    by_file: Dict[str, object] = {os.path.relpath(m.file, ROOT): m for m in modules}
    out = []
    for r in records:
        if r.get("file") in failed:
            continue
        m = by_file.get(r.get("file"))
        if m is not None and m.origins:
            remap([r], m.origins, m.source_lines)
        out.append(r)
    return out + di
