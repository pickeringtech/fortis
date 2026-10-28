"""Extract every ```fortis code block from the specification documents into the corpus.

Each block's first line must carry a marker (Language Reference 0, Overview "Status"):
  // accept            a conforming implementation must accept the program
  // reject: E-XXX-NNN it must reject it with that diagnostic at the marked line
  // execute: <result> it must produce that observable result
  // illustrative      a fragment or template that is not a complete program
An unmarked block is a documentation defect and makes this tool exit non-zero.

Fixture IDs are <doc>-<section>-<ordinal> (e.g. LR-6.3-02) so they survive line shifts.
"""
from __future__ import annotations

import json
import os
import re
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
DOCS = {
    "LR": "docs/Fortis_Language_Reference.md",
    "PS": "docs/Fortis_Platform_Specification.md",
    "OV": "docs/Fortis_Specification.md",
}
CORPUS = os.path.join(ROOT, "conformance", "corpus")

_HEADING = re.compile(r"^(#{2,3})\s+(?:(\d+(?:\.\d+)?)\.?|Appendix\s+([A-Z])\.?)\s")
_MARKER = re.compile(r"^//\s*(accept|reject|execute|illustrative)\b\s*[:—-]?\s*(.*)$")
_CODE = re.compile(r"E-[A-Z]{3}-\d{3}")


CORE_MODULES = {"prelude", "text", "collections", "numeric", "time", "money", "host"}
_IMPORT = re.compile(r"^\s*import\s+fortis\.([a-z]+)", re.M)


def kit_packages(body):
    """Kit packages a block imports (`fortis.<x>` where x is not a core module), sorted."""
    return sorted({m.group(1) for m in _IMPORT.finditer("\n".join(body)) if m.group(1) not in CORE_MODULES})


def inline_markers(body):
    """`// reject: E-…` markers inside a block. A marker on a comment-only line applies to the
    next line that holds code (LR 0: "at the marked line")."""
    out = []
    for k, text in enumerate(body[1:], start=2):
        if "// reject:" not in text:
            continue
        target = k
        if not text.split("//", 1)[0].strip():
            j = k  # 1-based index of this line; body[j] is the next line
            while j < len(body) and (not body[j].strip() or body[j].strip().startswith("//")):
                j += 1
            target = j + 1
        for code in _CODE.findall(text.split("// reject:", 1)[1]):
            out.append({"line": target, "code": code})
    return out


def extract():
    fixtures = []
    unmarked = []
    for doc, rel in DOCS.items():
        path = os.path.join(ROOT, rel)
        lines = open(path, encoding="utf-8").read().split("\n")
        section = "0"
        counters = {}
        i = 0
        while i < len(lines):
            line = lines[i]
            m = _HEADING.match(line)
            if m:
                section = m.group(2) or m.group(3)
            if line.strip() == "```fortis":
                start = i + 1
                j = start
                while j < len(lines) and not lines[j].startswith("```"):
                    j += 1
                body = lines[start:j]
                counters[section] = counters.get(section, 0) + 1
                fid = f"{doc}-{section}-{counters[section]:02d}"
                verdict, note, codes = "unmarked", "", []
                mm = _MARKER.match(body[0].strip()) if body else None
                if mm:
                    verdict, note = mm.group(1), mm.group(2).strip()
                    codes = _CODE.findall(note)
                else:
                    unmarked.append((rel, start + 1, fid))
                marked_lines = inline_markers(body)
                for ml in marked_lines:
                    if ml["code"] not in codes:
                        codes.append(ml["code"])
                packages = kit_packages(body)
                fixtures.append({
                    "id": fid,
                    "stage": "kit" if (packages or doc == "PS") else "core",
                    "packages": packages,
                    "doc": rel,
                    "section": section,
                    "doc_line": start + 1,
                    "verdict": verdict,
                    "codes": codes,
                    "marked_lines": marked_lines,
                    "note": note,
                    "path": f"conformance/corpus/{fid}.ft",
                    "elided": any("…" in t for t in body),
                })
                os.makedirs(CORPUS, exist_ok=True)
                with open(os.path.join(CORPUS, fid + ".ft"), "w", encoding="utf-8") as f:
                    f.write("\n".join(body) + "\n")
                i = j + 1
                continue
            i += 1
    # remove stale fixtures
    keep = {f["id"] + ".ft" for f in fixtures}
    for name in os.listdir(CORPUS):
        if name.endswith(".ft") and name not in keep:
            os.remove(os.path.join(CORPUS, name))
    with open(os.path.join(CORPUS, "manifest.json"), "w", encoding="utf-8") as f:
        json.dump({"fixtures": fixtures}, f, indent=2)
        f.write("\n")
    return fixtures, unmarked


if __name__ == "__main__":
    fixtures, unmarked = extract()
    by = {}
    for fx in fixtures:
        by[fx["verdict"]] = by.get(fx["verdict"], 0) + 1
    print(f"{len(fixtures)} fixtures: " + ", ".join(f"{k} {v}" for k, v in sorted(by.items())))
    for rel, line, fid in unmarked:
        print(f"UNMARKED {rel}:{line} ({fid})")
    sys.exit(1 if unmarked else 0)
