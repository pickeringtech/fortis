"""DEL-06 criterion 2: every type and operation the Language Reference names in sections 6.2–6.10,
10.3–10.6, 12.5–12.7 and 15 appears in the signature modules. Names are taken from backticked
snippets: `Type.member`, `value.method(`, bare `Type` (optionally with type arguments). Prints the
misses and exits 1 if any."""
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import names   # noqa: E402
import parser  # noqa: E402

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
LR = os.path.join(ROOT, "docs", "Fortis_Language_Reference.md")
SECTIONS = [("### 6.2", "### 6.11"), ("### 10.3", "### 10.7"), ("### 12.5", "## 13"), ("## 15", "## 16")]
# words that look like types but are grammar or prose, not library names
IGNORE = {"Self", "Void", "Never", "T", "K", "V", "N", "E", "I", "A", "B", "R", "S", "U", "Params", "Ret", "Type", "Name", "Enum", "Variant",
          "LANG", "PLAT", "PROG", "X", "NFC", "UTF", "IEEE", "NaN", "MIN", "Equal", "Found", "NotFound", "Hasher",
          # per-activation authorities minted by kits (15) and example newtypes in prose
          "RequestContext", "Identity", "Db", "Tx", "TaskContext", "Mail", "Sanitiser", "UserId", "CustomerId", "Invoice"}


def main():
    rec = parser.load_grammar(os.path.join(ROOT, "reference", "grammar", "fortis.ebnf"))
    lib = names.Library(rec)
    types = {}
    methods = set()
    for mod in lib.modules.values():
        for n, d in mod.decls.items():
            types.setdefault(n, set()).update(d["members"])
            methods |= d["members"]
    text = open(LR, encoding="utf-8").read()
    misses, checked = [], 0
    for a, b in SECTIONS:
        sec = text[text.index(a):text.index(b)]
        for snip in re.findall(r"`([^`]+)`", sec):
            m = re.match(r"^([A-Z][A-Za-z0-9]*)\.([a-z][A-Za-z0-9]*)", snip)
            if m:
                checked += 1
                t, mem = m.groups()
                if t in IGNORE:
                    continue
                if t not in types:
                    misses.append(f"{snip}: type {t} not in inventory")
                elif mem not in types[t]:
                    misses.append(f"{snip}: {t} has no member {mem}")
                continue
            m = re.match(r"^[a-z][A-Za-z0-9]*\.([a-z][A-Za-z0-9]*)\(", snip)
            if m:
                checked += 1
                if m.group(1) not in methods:
                    misses.append(f"{snip}: no class declares method {m.group(1)}")
                continue
            m = re.match(r"^([A-Z][A-Za-z0-9]*)(\[.*\])?\??$", snip)
            if m and m.group(1) not in IGNORE:
                checked += 1
                if m.group(1) not in types:
                    misses.append(f"{snip}: type {m.group(1)} not in inventory")
    print(f"checked {checked} named types/operations across {len(SECTIONS)} section ranges; {len(misses)} missing")
    for mi in sorted(set(misses)):
        print(" -", mi)
    return 1 if misses else 0


if __name__ == "__main__":
    sys.exit(main())
