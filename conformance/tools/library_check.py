"""DEL-26 runner: the core library modules against their DEL-06 signatures.

For each of `fortis.prelude`, `fortis.text`, `fortis.collections` and `fortis.numeric`:
  1. every signature of `reference/library/<m>.ft` appears in `library/fortis/<m>.ft` with the same text once
     `trusted` and the body are removed (no signature missing, changed or added);
  2. every signature has a Fortis body or is a `trusted` intrinsic with a contract comment (`// intrinsic: …`)
     directly above it, and the intrinsic is registered in `interpreter/intrinsics.py` (0 missing);
  3. the module checks with 0 diagnostics on the reference checker (package `fortis`);
  4. every `pure`/`noalloc` claim of the reference is carried (implied by 1) and satisfied (implied by 3 for
     bodies; intrinsics are labelled trusted, 11.3);
  5. every row of the standard packages' effect data (reference/library/effects; D-112) names an operation its module
     declares; where the implementation gives that operation a Fortis body, the row declares at least the effects the
     body's summary has (a signature module's operations are bodiless, so callers see the row, not the body).

  python3 -I conformance/tools/library_check.py [--json]"""
from __future__ import annotations

import json
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
sys.path.insert(0, ROOT)
sys.path.insert(0, os.path.join(ROOT, "conformance", "tools"))
MODULES = ("prelude", "text", "collections", "numeric", "time", "money", "host")
SIG = re.compile(r"^\s*(export\s+)?(trusted\s+)?((?:[A-Z]\w*\s+)*(?:pure\s+)?(?:noalloc\s+)?(?:async\s+)?(?:try\s+)?(?:recovers\s+)?)func\s+(\w+)")


def signatures(path: str) -> dict:
    """{key: {"text": normalised signature, "trusted": bool, "line": n, "contract": str|None, "body": bool}}"""
    out, owner, kind = {}, None, None
    lines = open(path, encoding="utf-8").read().split("\n")
    for i, l in enumerate(lines):
        m = re.match(r"export (class|enum|interface) (\w+)", l)
        if m:
            kind, owner = m.group(1), m.group(2)
        if l.startswith("}") or l.rstrip().endswith("{}"):
            owner, kind = None, None                     # `export class X implements Authority {}` closes on its own line
            continue
        sm = SIG.match(l)
        if not sm or kind == "interface":
            continue
        key = (owner + "." if owner else "") + sm.group(4)
        text = l.strip()
        body = False
        if " => " in text:
            text, body = text.split(" => ", 1)[0], True
        elif text.endswith(" {"):
            text, body = text[:-2], True
        text = re.sub(r"\btrusted\s+", "", text).strip()
        prev = lines[i - 1].strip() if i > 0 else ""
        contract = prev[len("// intrinsic:"):].strip() if prev.startswith("// intrinsic:") else None
        out[key] = {"text": text, "trusted": bool(sm.group(2)), "line": i + 1, "contract": contract, "body": body}
    return out


def registered(key: str, receiver: bool) -> bool:
    from interpreter import intrinsics
    if "." not in key:
        return key in intrinsics.FUNCS
    t, n = key.split(".", 1)
    table = intrinsics.METHODS if receiver else intrinsics.ASSOC
    return (t, n) in table or (t, n) in intrinsics.METHODS or (t, n) in intrinsics.ASSOC


def effect_data_problems(summaries: dict) -> list:
    """Each effect-data row names a declared operation of its module, and covers what the implementation's body proves
    when it has one (D-112). `summaries`: {core module: {label: Summary}} of the implementation modules."""
    from checker.effects import DECLARABLE, TRUSTED_EFFECTS
    out = []
    for (module, owner, name), effs in sorted(TRUSTED_EFFECTS.items(), key=lambda kv: (kv[0][0], kv[0][1] or "", kv[0][2])):
        key = (owner + "." if owner else "") + name
        short = module[len("fortis."):] if module.startswith("fortis.") else module
        if short in MODULES:
            ref = signatures(os.path.join(ROOT, "reference", "library", short + ".ft"))
            impl = signatures(os.path.join(ROOT, "library", "fortis", short + ".ft"))
            if key not in ref:
                out.append(f"effect data: {module} {key} ({', '.join(effs)}) names no operation of the module")
            elif key in impl and impl[key]["body"]:
                label = f"method {owner}.{name}" if owner else f"function {name}"
                s = summaries.get(short, {}).get(label)
                proved = set(s.effects) & set(DECLARABLE) if s is not None else None
                if proved is None:
                    out.append(f"effect data: {module} {key}: the implementation's body has no summary ({label})")
                elif proved - set(effs):
                    out.append(f"effect data: {module} {key} declares {', '.join(effs)}, but its implementation's body has {', '.join(sorted(proved))}")
        else:
            prof = os.path.join(ROOT, "reference", "library", "kits", module + ".ft")
            if not os.path.exists(prof) or key not in signatures(prof):
                out.append(f"effect data: {module} {key} ({', '.join(effs)}) names no operation of the module")
    print(f"effect data: {len(TRUSTED_EFFECTS)} rows, {len(out)} problems")
    return out


def main(argv):
    import lexer
    import names
    import parser
    from checker import check
    rec = parser.load_grammar(os.path.join(ROOT, "reference", "grammar", "fortis.ebnf"))
    lib = names.Library(rec)
    report, problems, summaries = {}, [], {}
    for mod in MODULES:
        ref = signatures(os.path.join(ROOT, "reference", "library", mod + ".ft"))
        impl = signatures(os.path.join(ROOT, "library", "fortis", mod + ".ft"))
        bodies, intrinsic, missing = [], [], []
        for key, r in ref.items():
            i = impl.get(key)
            if i is None:
                problems.append(f"{mod}: {key} is missing from the implementation")
                missing.append(key)
                continue
            if i["text"] != r["text"]:
                problems.append(f"{mod}: {key} signature differs:\n    reference: {r['text']}\n    library:   {i['text']}")
            if i["body"]:
                bodies.append(key)
            elif i["trusted"]:
                receiver = "(self" in i["text"] or "(editable self" in i["text"] or "(owned self" in i["text"]
                if not i["contract"]:
                    problems.append(f"{mod}: intrinsic {key} has no `// intrinsic:` contract above it (line {i['line']})")
                if not registered(key, receiver):
                    problems.append(f"{mod}: intrinsic {key} is not implemented in interpreter/intrinsics.py")
                    missing.append(key)
                intrinsic.append(key)
            else:
                problems.append(f"{mod}: {key} has neither a body nor `trusted`")
                missing.append(key)
        for key in impl:
            if key not in ref:
                problems.append(f"{mod}: {key} is in the implementation but not in the DEL-06 signatures")
        src = open(os.path.join(ROOT, "library", "fortis", mod + ".ft"), encoding="utf-8").read()
        records = [r for r in check.check_source(lib, src, f"library/fortis/{mod}.ft", module=f"fortis.{mod}", package="fortis") if r.get("code")]
        summaries[mod] = dict(check.LAST_TYPER.summaries)
        for r in records:
            problems.append(f"{mod}: checker {r['code']} line {r['line']}: {r['message'][:120]}")
        claims = sorted(k for k, r in ref.items() if re.search(r"\b(pure|noalloc)\b", r["text"]))
        report[mod] = {"signatures": len(ref), "bodies": bodies, "intrinsics": len(intrinsic), "missing": missing,
                       "diagnostics": len(records), "pure_or_noalloc_claims": claims}
        print(f"{mod:<12} {len(ref):3} signatures: {len(bodies):2} Fortis bodies, {len(intrinsic):3} intrinsics, "
              f"{len(missing)} missing, {len(records)} checker diagnostics, {len(claims)} pure/noalloc claims carried")
    problems += effect_data_problems(summaries)
    for p in problems:
        print("FAIL", p)
    if "--json" in argv:
        json.dump({"modules": report, "problems": problems}, open(os.path.join(ROOT, "conformance", "library_results.json"), "w"), indent=1)
    print(f"\n{sum(r['signatures'] for r in report.values())} signatures, {len(problems)} problems")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
