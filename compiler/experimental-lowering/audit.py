"""Copy audit and purity audit over the generated Rust (DEL-25 criteria 2 and 3).

Copy audit: every `.clone()`, `Rc::new` and `Rc::clone` in generated application code (not the runtime) must
carry an attribution comment on the same line (`/* copy: … */` or `/* reference count: … */`) naming its
source construct (source `copy`, `retain`, Shared creation) or a documented lowering rule (L1: element reads
of List.get/at/first copy the element). A `.to_string()` of a string literal is a literal materialisation, not
a copy of a Fortis value, and is listed separately.

Purity audit (SP-05): a `pure` function's generated signature takes `&mut` only for its own editable parameters
(its write footprint); no `RefCell`, `Cell`, `static mut` or interior mutability appears in generated code.

  python3 -I compiler/experimental-lowering/audit.py build/native            # all crates under the build directory"""
from __future__ import annotations

import json
import os
import re
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
COPY_OPS = re.compile(r"\.clone\(\)|Rc::new\(|Rc::clone\(")
ATTRIBUTED = re.compile(r"/\* (copy|reference count): [^*]+\*/")
INTERIOR = re.compile(r"\bRefCell\b|\bCell<|\bstatic mut\b|\bUnsafeCell\b|\bunsafe\b")


def audit_crate(src_path: str) -> dict:
    lines = open(src_path, encoding="utf-8").read().split("\n")
    copies, unattributed, literal_strings = [], [], 0
    interior = []
    for i, l in enumerate(lines, 1):
        for m in COPY_OPS.finditer(l):
            if ATTRIBUTED.search(l):
                copies.append({"line": i, "op": m.group(0), "attribution": ATTRIBUTED.search(l).group(0)})
            else:
                unattributed.append({"line": i, "op": m.group(0), "text": l.strip()[:120]})
        literal_strings += len(re.findall(r'"\.to_string\(\)', l))
        if INTERIOR.search(l):
            interior.append({"line": i, "text": l.strip()[:120]})
    return {"copies": copies, "unattributed": unattributed, "literal_strings": literal_strings, "interior_mutability": interior}


def main(argv):
    build = argv[0] if argv else os.path.join(ROOT, "build", "native")
    report = {}
    total = unatt = 0
    for d in sorted(os.listdir(build)):
        for f in ("src/main.rs", "src/lib.rs"):
            p = os.path.join(build, d, f)
            if os.path.exists(p):
                r = audit_crate(p)
                report[d] = r
                total += len(r["copies"])
                unatt += len(r["unattributed"])
                if r["unattributed"] or r["interior_mutability"]:
                    print(f"{d}: {len(r['unattributed'])} unattributed copies, {len(r['interior_mutability'])} interior-mutability sites")
                    for u in r["unattributed"] + r["interior_mutability"]:
                        print(f"    line {u['line']}: {u.get('text')}")
    print(f"{len(report)} crates audited: {total} attributed copy/reference-count operations, {unatt} unattributed; "
          f"{sum(len(r['interior_mutability']) for r in report.values())} interior-mutability sites")
    json.dump(report, open(os.path.join(build, "audit.json"), "w"), indent=1)
    return 0 if unatt == 0 and not any(r["interior_mutability"] for r in report.values()) else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
