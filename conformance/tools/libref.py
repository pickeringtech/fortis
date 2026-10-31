"""The core library reference (PROG-AGT-07; PROG-PRD-24 for the core package; R24, X-598), generated from the
declarations of `library/fortis` by the language's own viewer (`fortisc view --decls --json`) and from the effect
data of `reference/library/effects`, so that it cannot drift from the library.

  python3 -I conformance/tools/libref.py [--check]

Writes one page for each mode of the documentation, from the same model of the library:

  docs/agent/library.md   agent mode: dense tables, one row per declaration
  docs/guide/library.md   human mode: what each module is for, how to read an entry, then each type's operations
                          as sentences

Each page lists every exported declaration of each core module, with its signature, the effects it declares
(the `try`, `async` and `recovers` modifiers, the effect data's `alloc`, `block`, `io`, `mutate` and `nondet` rows,
the `io` that a capability's method or an operation taking an `Authority` has, LR 11.1, 11.6) and the contract its
documentation comment states (for a trusted intrinsic, that comment is its review record, LR 11.3). `--check`
fails, writing nothing, when either committed page differs from what the library gives now; the gate runs it
(the `libref` step), so a new library operation cannot reach the branch without its page being regenerated.
Neither page holds an example.
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
OUT_GUIDE = os.path.join(ROOT, "docs", "guide", "library.md")
EFFECT_DATA = os.path.join(ROOT, "reference", "library", "effects")
PREVIEW = "> Preview: the core runtime inventory (LR 15) arrives in v0.4."
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


GUIDE_ABOUT = {
    "prelude": "You never import this module; every module has it already (LR 4.3). It holds the types you use in every "
               "program: the integers and floats, `String` and `StringView`, lists, maps and sets, `Outcome` and `Failure`, "
               "the concurrency types, and the built-in interfaces such as `Equatable` and `Displayable`. The everyday text "
               "operations (`trim`, `split`, `find`, `replace` and the rest) are members of `String` and `StringView`, so "
               "they need no import either.",
    "text": "Building and inspecting text. Import what you use, for example `import fortis.text { Builder }`. It holds "
            "`Builder`, which accumulates text a piece at a time, the one-character `Scalar`, `graphemes`, which counts what a "
            "reader sees as characters, and `Text.words`.",
    "collections": "Algorithms over collections that the prelude's core list, map and set operations do not cover. Import "
                   "what you use, for example `import fortis.collections { sorted }`.",
    "numeric": "Small integer helpers. Import what you use, for example `import fortis.numeric { min, max }`.",
    "time": "Durations and timestamps, with checked arithmetic. Import what you use.",
    "money": "An amount of a currency, with exact decimal arithmetic. Import what you use.",
    "host": "The capabilities a program receives from its host (the file system, the network, the clock, the console and "
            "the rest), the `Path` they take, and `Secret`. Each capability is an authority: a function that holds none "
            "cannot reach the outside world (LR 11.6).",
}

EFFECT_WORDS = [
    ("pure", "written before the effects when the signature claims it: the operation reads nothing from outside its arguments and writes nothing (LR 11.2). The effects after it are what a pure operation may still do, such as allocate or fail."),
    ("noalloc", "written before the effects when the signature claims it: the operation promises not to allocate (LR 11.4)."),
    ("none", "an operation that does not claim `pure` and has no effect to declare."),
    ("alloc", "it allocates; it cannot be called from `noalloc` code (LR 11.4)."),
    ("fail", "it is declared `try`: it can fail, and the caller must handle the outcome (LR 10.1)."),
    ("suspend", "it is `async` and can wait (LR 12.3)."),
    ("recovers", "it discards a failure it handled (LR 10.4)."),
    ("block", "it blocks the calling thread (LR 11.1)."),
    ("io", "it reaches the outside world through an authority (LR 11.6)."),
    ("mutate", "it writes storage that tasks share (LR 11.1)."),
    ("nondet", "its result can differ between runs with the same arguments (LR 11.1)."),
    ("writes self", "it changes the value it is called on (`editable self`)."),
]


def effect_rows():
    """The effect data of the standard packages: {(module, owner, name): [effects]} (D-112)."""
    rows = {}
    for f in sorted(os.listdir(EFFECT_DATA)):
        if f.endswith(".json"):
            with open(os.path.join(EFFECT_DATA, f), encoding="utf-8") as fh:
                for r in json.load(fh)["operations"]:
                    rows[(r["module"], r["owner"], r["name"])] = list(r["effects"])
    return rows


def effects_of(d, module, rows, authorities):
    """The effects a declaration declares, in the order the Language Reference lists them: the modifiers' `fail`,
    `suspend` and `recovers`, the effect data's rows, the `io` of a capability's method or an operation taking an
    authority (checker/effects.py `library_summary`), and `writes self` for an impure `editable self` method."""
    if d["declaration"] not in ("method", "function", "func"):
        return []
    mods = set(d.get("modifiers") or [])
    out = []
    if "try" in mods:
        out.append("fail")
    if "async" in mods:
        out.append("suspend")
    if "recovers" in mods:
        out.append("recovers")
    out += rows.get((f"fortis.{module}", d.get("member_of"), d["name"]), [])
    head = d["head"]
    params = head[head.index("(") + 1:] if "(" in head else ""
    takes_authority = any(re.search(rf":\s*{re.escape(a)}\b", params) for a in authorities)
    has_self = bool(re.search(r"\((?:editable |owned )?self\b", head))
    if "pure" not in mods and ((d.get("member_of") in authorities and has_self) or takes_authority) and "io" not in out:
        out.append("io")
    if "pure" not in mods and "(editable self" in head:
        out.append("writes self")
    order = [w for w, _ in EFFECT_WORDS]
    return sorted(dict.fromkeys(out), key=lambda e: order.index(e) if e in order else len(order))


def claims_of(d):
    return [m for m in (d.get("modifiers") or []) if m in ("pure", "noalloc")]


def model():
    """The library as the pages show it: [(module, about, cite, types, free)], with each type's members and variants."""
    r = subprocess.run([FORTISC, "view", "--decls", "--json", "library/fortis"], capture_output=True, text=True, cwd=ROOT)
    if r.returncode != 0:
        raise SystemExit(f"libref: fortisc view failed: {r.stderr[-300:]}")
    decls = [json.loads(l) for l in r.stdout.splitlines() if l.startswith("{")]
    # interface members have no visibility of their own: they are public wherever the interface is (LR 4.4)
    interfaces = {(d["path"], d["name"]) for d in decls if d.get("declaration") == "interface" and d.get("exported")}
    authorities = {d["name"] for d in decls if d.get("declaration") == "class" and re.search(r"\bimplements\b.*\bAuthority\b", d["head"])}
    by_module = {}
    for d in decls:
        if d.get("kind") != "declaration":
            continue
        if not d.get("exported") and (d["path"], d.get("member_of")) not in interfaces:
            continue
        module = os.path.splitext(os.path.basename(d["path"]))[0]
        by_module.setdefault(module, []).append(d)
    rows = effect_rows()
    out = []
    for module, about, cite in MODULES:
        items = by_module.get(module, [])
        tops = [d for d in items if not d.get("member_of")]
        members = {}
        for d in items:
            if d.get("member_of"):
                members.setdefault(d["member_of"], []).append(d)
        free = [d for d in tops if d["declaration"] not in ("class", "enum", "interface", "error", "type")]
        types = []
        for t in [d for d in tops if d not in free]:
            vs = variants(t["path"], t["name"]) if t["declaration"] == "enum" else []
            ms = members.get(t["name"], [])
            COUNTS["variants"] += len(vs)
            COUNTS["members"] += len([m for m in ms if m["declaration"] in ("method", "function")])
            types.append({"decl": t, "variants": vs, "members": ms})
        COUNTS["free"] += len(free)
        for group in [free] + [t["members"] for t in types]:
            for d in group:
                d["_effects"] = effects_of(d, module, rows, authorities)
                d["_claims"] = claims_of(d)
        out.append((module, about, cite, types, free))
    return out


def effect_cell(d):
    """Pure/noalloc claims, then effects: `pure`, `pure; alloc`, `fail; block, alloc`, `none` for neither."""
    claims, effs = d["_claims"], d["_effects"]
    if d["declaration"] not in ("method", "function", "func"):
        return "—"
    if not claims and not effs:
        return "none"
    return ", ".join(claims) + ("; " if claims and effs else "") + ", ".join(effs)


def sentence(text):
    text = (text or "").strip()
    if not text:
        return ""
    text = text[0].upper() + text[1:]
    return text if text[-1] in ".!?" else text + "."


def agent_page(mods):
    out = [
        "# The core library",
        "",
        "<!-- covers: LR 15 -->",
        "*Agent mode. Human mode: [The core library](../guide/library.md). Generated by `conformance/tools/libref.py` from the declarations",
        "of `library/fortis` and the effect data of `reference/library/effects`; do not edit.",
        "Source of truth: LR 15 for the inventory, and each declaration's own contract.*",
        "",
        PREVIEW,
        "",
        "Every public declaration of the seven core modules (LR 15): each exported declaration, every member of an",
        "exported interface (whose members share its visibility, LR 4.4) and every variant of an exported enum, with its",
        "signature as the library declares it, the effects it declares and the contract its documentation comment",
        "states. The check counts these against the module sources independently of this page. A `trusted` operation is",
        "implemented by the toolchain, and its comment is its review record (LR 11.3). `fortis.prelude` is imported into",
        "every module; the other modules need an `import` (LR 4.3). What is not listed here does not exist in Language",
        "1.0's core library; the gaps against the Language Reference are recorded as D-58.",
        "",
        "The Effects column is `pure` or `noalloc` where the signature claims it, then the effects the operation has,",
        "which a caller inherits (LR 11.1, 11.3): `alloc`, `fail` (a `try` function), `suspend` (`async`), `recovers`,",
        "`block`, `io` (a capability's method, or an operation that takes an authority), `mutate`, `nondet`, and",
        "`writes self` (an impure `editable self` method). `none` is an impure operation with no effect to declare; a",
        "type has none.",
        "",
    ]
    for module, about, cite, types, free in mods:
        out += [f"## `fortis.{module}`", "", f"{about[0].upper()}{about[1:]} ({cite}).", ""]
        for t in types:
            d = t["decl"]
            # the prelude's types are listed in `fortis.prelude`; a module that redeclares one (time) lists it there too
            out.append(f"### `{d['head']}`")
            out.append("")
            doc = contract(d.get("doc"))
            if doc:
                out += [doc, ""]
            if d["declaration"] == "enum":
                out += ["Variants: " + ", ".join(f"`{v}`" for v in t["variants"]) + ".", ""]
            if t["members"]:
                out += ["| Declaration | Effects | Contract |", "|---|---|---|"]
                for m in t["members"]:
                    out.append(f"| `{cell(m['head'])}` | {effect_cell(m)} | {cell(contract(m.get('doc'))) or '—'} |")
                out.append("")
        if free:
            out += ["### Functions and aspects", "", "| Declaration | Effects | Contract |", "|---|---|---|"]
            for f in free:
                doc = contract(f.get("doc"))
                if f["name"] == "injectDefect":
                    doc = "the conformance corpus's fault-injection hook (LR 10.8); not part of the language"
                out.append(f"| `{cell(f['head'])}` | {effect_cell(f)} | {cell(doc) or '—'} |")
            out.append("")
    return "\n".join(out).rstrip("\n") + "\n"


def guide_page(mods):
    out = [
        "# The core library",
        "",
        "<!-- covers: LR 15 -->",
        "*Human mode. Agent mode: [Core library, by declaration](../agent/library.md). Generated by",
        "`conformance/tools/libref.py` from the declarations of `library/fortis`; do not edit. Source of truth: LR 15.*",
        "",
        PREVIEW,
        "",
        "This is the reference for the standard library that ships with the compiler: every type and function of the core",
        "package `fortis`, with its signature, what it promises and the effects it has. It is not a tutorial; the chapters",
        "of this guide teach the language, and this page is where you look an operation up. Nothing here is written by",
        "hand: each entry is the library's own declaration and the documentation comment above it, so the page cannot say",
        "something the library does not do. The same facts, as tables for tools, are in",
        "[the agent-mode page](../agent/library.md).",
        "",
        "## How to read an entry",
        "",
        "The library is one package, `fortis`, made of seven modules. Each section below is a module. `fortis.prelude` is",
        "imported into every module for you; the others you import by name (LR 4.3). Under a module come its types, each",
        "with its operations, and then its free functions.",
        "",
        "An entry is the operation's signature followed by what it promises. Read the signature the way you read your own:",
        "`self` is the value the operation is called on, a word before `func` such as `pure` or `try` is a claim the",
        "compiler checks (LR 11.2), and `from self` says the result is a loan of the receiver, so it cannot outlive it",
        "(LR 7.6). Most operations are `trusted`: the toolchain implements them, and the sentence under the signature is",
        "the contract a reviewer signed off (LR 11.3). A cost such as `O(n)` is the bound the contract promises.",
        "",
        "*Effects* tell you what calling the operation does beyond computing its result, and what your own function",
        "inherits by calling it (LR 11.1):",
        "",
    ]
    out += [f"- `{w}`: {text}" for w, text in EFFECT_WORDS]
    out += [""]
    for module, about, cite, types, free in mods:
        out += [f"## `fortis.{module}`", "", GUIDE_ABOUT[module] + f" ({cite}).", ""]
        for t in types:
            d = t["decl"]
            out.append(f"### `{d['head']}`")
            out.append("")
            doc = sentence(contract(d.get("doc")))
            if doc:
                out += [doc, ""]
            if d["declaration"] == "enum":
                out += ["Variants: " + ", ".join(f"`{v}`" for v in t["variants"]) + ".", ""]
            for m in t["members"]:
                out += guide_entry(m)
            if t["members"]:
                out.append("")
        if free:
            out += ["### Functions and aspects", ""]
            for f in free:
                doc = contract(f.get("doc"))
                if f["name"] == "injectDefect":
                    doc = "the conformance corpus's fault-injection hook (LR 10.8); not part of the language"
                out += guide_entry(f, doc)
            out.append("")
    return "\n".join(out).rstrip("\n") + "\n"


def guide_entry(d, doc=None):
    head = re.sub(r"^trusted ", "", d["head"])
    doc = sentence(contract(d.get("doc")) if doc is None else doc)
    effects = effect_cell(d)
    line = f"- `{head}`"
    if doc:
        line += f". {doc}"
    return [line + ("" if effects == "—" else f" *Effects: {effects}.*")]


def generate():
    for k in COUNTS:
        COUNTS[k] = 0
    mods = model()
    return agent_page(mods), guide_page(mods)


def main(argv):
    agent, guide = generate()
    # completeness, judged independently of the generated text: the page lists as many methods, variants and free
    # functions as the module sources declare publicly
    inventory = source_inventory()
    if inventory != COUNTS:
        print(f"libref: FAILED: the page lists {COUNTS}, the sources declare {inventory}")
        return 1
    pages = [(OUT, agent), (OUT_GUIDE, guide)]
    if "--check" in argv:
        bad = [os.path.relpath(path, ROOT) for path, text in pages if (open(path, encoding="utf-8").read() if os.path.exists(path) else "") != text]
        if bad:
            print("libref: FAILED: " + ", ".join(bad) + " differ from the library; run conformance/tools/libref.py")
            return 1
        print("libref: OK (docs/agent/library.md and docs/guide/library.md match the library)")
        return 0
    for path, text in pages:
        with open(path, "w", encoding="utf-8") as fh:
            fh.write(text)
        print(f"libref: wrote {os.path.relpath(path, ROOT)} ({text.count(chr(10))} lines)")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
