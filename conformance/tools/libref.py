"""The core library reference (PROG-AGT-07; PROG-PRD-24 for the core package), generated from the declarations of
`library/fortis` by the language's own viewer (`fortisc view --decls --json`), so that it cannot drift from the
library.

  python3 -I conformance/tools/libref.py [--check]

Writes `docs/agent/library.md`: every exported declaration of each core module, with its signature and the
contract its documentation comment states (for a trusted intrinsic, that comment is its review record, LR 11.3).
`--check` fails, writing nothing, when the committed page differs from what the library gives now. The page is
read by both modes (the guide links it); it holds no examples.
"""
from __future__ import annotations

import json
import os
import re
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
sys.path.insert(0, HERE)
from cite_check import SPECS, spec_sections  # noqa: E402
FORTISC = os.path.join(ROOT, "compiler", "target", "release", "fortisc")
OUT = os.path.join(ROOT, "docs", "agent", "library.md")
MODULES = [
    ("prelude", "imported into every module: the built-in types, the optional and outcome operations, the built-in interfaces", "LR 4.3, LR 6.2, LR 6.10"),
    ("text", "text building, scalars and graphemes", "LR 6.6"),
    ("collections", "collection algorithms", "LR 6.7"),
    ("numeric", "integer helpers", "LR 6.4"),
    ("time", "durations and timestamps", "LR 15"),
    ("money", "an amount of a currency", "LR 15"),
    ("host", "the capabilities, `Path`, `Secret` and `reveal`", "LR 11.6, LR 15"),
]


LR = spec_sections(SPECS["LR"])


def cell(text):
    return text.replace("|", "\\|").replace("\n", " ")


def contract(doc):
    """The contract a comment states, without the implementation notes of the library's sources."""
    doc = (doc or "").strip()
    doc = re.sub(r"^-{4}.*?-{4}\s*", "", doc)         # a section separator of the source, not a contract
    if re.match(r"^`fortis\.\w+` implementation", doc) or doc.startswith("Fault injection for the conformance corpus"):
        return ""
    doc = re.sub(r"^intrinsic:\s*", "", doc)
    # a diagnostic code a contract names is cited with the Language Reference subsection that states it
    for code in sorted(set(re.findall(r"E-[A-Z]{2,3}-\d{3}", doc))):
        where = next((sid for sid, (_, text, _) in LR.items() if "." in sid and code in text), None)
        if where:
            doc += f" ({code}: LR {where})"
    return doc


COUNTS = {"members": 0, "variants": 0, "free": 0}


def variants(path, name):
    """An enum's variants, from the viewer's text of the declaration (`--decls` does not list them)."""
    r = subprocess.run([FORTISC, "view", "--json", "--symbol", name, path], capture_output=True, text=True, cwd=ROOT)
    text = ""
    for l in r.stdout.splitlines():
        if l.startswith("{"):
            rec = json.loads(l)
            if rec.get("kind") == "symbol":
                text = rec["text"]
                break
    body = text.split("{", 1)[1].rsplit("}", 1)[0] if "{" in text else ""
    out = []
    for line in body.split("\n"):
        line = line.split("//")[0].strip()
        if re.match(r"^[A-Z]\w*(\(.*\))?$", line):
            out.append(line)
    return out


def source_inventory():
    """An independent count of the public API, read from the module sources line by line: the exported methods of
    exported classes, every member of an exported interface, every variant of an exported enum, and the exported
    free functions and aspects."""
    counts = {"members": 0, "variants": 0, "free": 0}
    for module, _, _ in MODULES:
        lines = open(os.path.join(ROOT, "library", "fortis", module + ".ft"), encoding="utf-8").read().split("\n")
        kind, depth = None, 0
        for l in lines:
            code = l.split("//")[0]
            if depth == 0:
                m = re.match(r"^export\s+(?:trivial\s+)?(class|enum|interface|error)\b", code)
                if m and "{" in code:
                    kind = m.group(1)
                    depth = code.count("{") - code.count("}")
                    continue
                if re.match(r"^export\s+(?:\w+\s+)*(func|aspect)\b", code):
                    counts["free"] += 1
                continue
            s = code.strip()
            if depth == 1 and s:
                if kind == "interface" and re.match(r"^(?:\w+\s+)*func\s", s):
                    counts["members"] += 1
                elif kind == "enum" and re.match(r"^[A-Z]\w*(\(.*\))?$", s):
                    counts["variants"] += 1
                elif kind == "class" and s.startswith("export ") and re.search(r"\bfunc\s", s):
                    counts["members"] += 1
            depth += code.count("{") - code.count("}")
    return counts


def generate():
    r = subprocess.run([FORTISC, "view", "--decls", "--json", "library/fortis"], capture_output=True, text=True, cwd=ROOT)
    if r.returncode != 0:
        raise SystemExit(f"libref: fortisc view failed: {r.stderr[-300:]}")
    decls = [json.loads(l) for l in r.stdout.splitlines() if l.startswith("{")]
    # interface members have no visibility of their own: they are public wherever the interface is (LR 4.4)
    interfaces = {(d["path"], d["name"]) for d in decls if d.get("declaration") == "interface" and d.get("exported")}
    by_module = {}
    for d in decls:
        if d.get("kind") != "declaration":
            continue
        if not d.get("exported") and (d["path"], d.get("member_of")) not in interfaces:
            continue
        module = os.path.splitext(os.path.basename(d["path"]))[0]
        by_module.setdefault(module, []).append(d)
    out = [
        "# The core library",
        "",
        "<!-- covers: LR 15 -->",
        "*Both modes. Generated by `conformance/tools/libref.py` from the declarations of `library/fortis`; do not edit.",
        "Source of truth: LR 15 for the inventory, and each declaration's own contract.*",
        "",
        "> Preview: the core runtime inventory (LR 15) arrives in v0.4.",
        "",
        "Every public declaration of the seven core modules (LR 15): each exported declaration, every member of an",
        "exported interface (whose members share its visibility, LR 4.4) and every variant of an exported enum, with its",
        "signature as the library declares it and the contract its documentation comment states. The check counts these",
        "against the module sources independently of this page. A `trusted` operation is implemented by the toolchain, and",
        "its comment is its review record (LR 11.3). `fortis.prelude` is imported into every module; the other modules",
        "need an `import` (LR 4.3). What is not listed here does not exist in Language 1.0's core library; the gaps",
        "against the Language Reference are recorded as D-58.",
        "",
    ]
    for module, about, cite in MODULES:
        items = by_module.get(module, [])
        out += [f"## `fortis.{module}`", "", f"{about[0].upper()}{about[1:]} ({cite}).", ""]
        tops = [d for d in items if not d.get("member_of")]
        members = {}
        for d in items:
            if d.get("member_of"):
                members.setdefault(d["member_of"], []).append(d)
        free = [d for d in tops if d["declaration"] not in ("class", "enum", "interface", "error", "type")]
        types = [d for d in tops if d not in free]
        for t in types:
            # the prelude's types are listed in `fortis.prelude`; a module that redeclares one (time) lists it there too
            out.append(f"### `{t['head']}`")
            out.append("")
            doc = contract(t.get("doc"))
            if doc:
                out += [doc, ""]
            if t["declaration"] == "enum":
                vs = variants(t["path"], t["name"])
                COUNTS["variants"] += len(vs)
                out += ["Variants: " + ", ".join(f"`{v}`" for v in vs) + ".", ""]
            ms = members.get(t["name"], [])
            COUNTS["members"] += len([m for m in ms if m["declaration"] in ("method", "function")])
            if ms:
                out += ["| Declaration | Contract |", "|---|---|"]
                for m in ms:
                    out.append(f"| `{cell(m['head'])}` | {cell(contract(m.get('doc'))) or '—'} |")
                out.append("")
        COUNTS["free"] += len(free)
        if free:
            out += ["### Functions and aspects", "", "| Declaration | Contract |", "|---|---|"]
            for f in free:
                doc = contract(f.get("doc"))
                if f["name"] == "injectDefect":
                    doc = "the conformance corpus's fault-injection hook (LR 10.8); not part of the language"
                out.append(f"| `{cell(f['head'])}` | {cell(doc) or '—'} |")
            out.append("")
    return "\n".join(out).rstrip("\n") + "\n"


def main(argv):
    text = generate()
    # completeness, judged independently of the generated text: the page lists as many methods, variants and free
    # functions as the module sources declare publicly
    inventory = source_inventory()
    if inventory != COUNTS:
        print(f"libref: FAILED: the page lists {COUNTS}, the sources declare {inventory}")
        return 1
    if "--check" in argv:
        old = open(OUT, encoding="utf-8").read() if os.path.exists(OUT) else ""
        ok = old == text
        print("libref: " + ("OK (docs/agent/library.md matches the library)" if ok else "FAILED: docs/agent/library.md differs from the library; run conformance/tools/libref.py"))
        return 0 if ok else 1
    open(OUT, "w", encoding="utf-8").write(text)
    print(f"libref: wrote {os.path.relpath(OUT, ROOT)} ({text.count(chr(10))} lines)")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
