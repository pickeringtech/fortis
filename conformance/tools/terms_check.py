"""DEL-49 criterion 5: the terminology check of PROG-PRD-38. "Profile" now means performance profiling and nothing
else: the bundles of packages are kits, build profiles are build modes, and lint profiles are lint levels (X-150).

  python3 -I conformance/tools/terms_check.py [--json] [-v]

Scope: the specification (`docs/Fortis_*.md`, the grammar), the delivery plan in the maintainers' checkout (its
documents, chunk files, generated tables and tools), the documentation (`README.md`, `docs/guide`, `docs/agent`, `docs/site`, `skills`, the
READMEs of examples, acceptance and the implementation), the tools and their messages (`bin/fortis`,
`conformance/tools`, the reference checker and interpreter, the compiler crates, the telemetry pipeline), the
library (`library`, `reference/library`, every path in it), the manifests and the corpus programs. In scope, every
occurrence of `profil…` must be the profiling sense: `profiling`, `profiler`, `profile-guided`, or a performance,
CPU, heap, allocation or OpenTelemetry profile. The other permitted uses name what is not Fortis's: Cargo's own
`[profile.…]` table in a `Cargo.toml` or in one a tool generates (Rust's syntax), "what other toolchains call build profiles", and, on a line
that states the rename itself, the quoted old word and the old names in backticks.

Out of scope, because they are records of what was said or produced at the time and are cited by hash or date:
the review transcripts, frozen evidence (the delivery plan's evidence, the release record and the readability
preregistration), spike preregistrations, designs and decisions (`PREREGISTRATION*.md` and the spike records),
generated result files (`results*.json`, run logs, differential reports), Cargo's `target` directories, the
register's rows before X-150 and the defect register's entries before D-50 (an entry keeps the words of the day it
was written; later ones use the new words; each stream edits only its own entries), and the definitions of passed
chunks (their chunk files and their `chunk(...)` calls in `chunks.py`), which their criterion reports hash. This
file names the old word to find it. A file or directory whose path contains the old word is reported like a line.
The planning files are maintainers' files (planning.py): a checkout without them checks everything else.
"""
from __future__ import annotations

import hashlib
import json
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))

SCOPE = [
    "README.md", "AGENTS.md", "docs/Fortis_Specification.md", "docs/Fortis_Language_Reference.md",
    "docs/Fortis_Platform_Specification.md", "docs/Fortis_Delivery_Programme.md", "docs/Fortis_Specification_Review.md",
    "reference", "library", "docs/delivery", "docs/guide", "docs/agent", "docs/site", "skills", "bin", "conformance",
    "checker", "interpreter", "compiler", "telemetry", "examples", "acceptance", "benchmarks",
]
EXEMPT_DIRS = ("docs/reviews", "docs/delivery/evidence", "docs/delivery/readability", "docs/delivery/release", "spikes",
               "build", ".git")
# generated results and logs, preregistrations (frozen by hash), and this check itself, which has to name the word
EXEMPT_FILES = re.compile(r"(^|/)(results[\w-]*\.json|run-log\.json|[\w-]*-vs-[\w-]*\.json|self-comparison\.json|"
                          r"agreeing\.json|disagreeing\.json|Cargo\.lock|PREREGISTRATION[\w-]*\.md)$|^conformance/tools/terms_check\.py$")
TEXT = re.compile(r"\.(md|py|rs|ft|yaml|yml|json|ebnf|toml|txt|css|js|html|sh)$|^fortis$")
ALLOWED = re.compile(r"profil(?:ing|ers?)\b|profile[- ]guided|(?:performance|cpu|heap|allocation|memory)\s+profiles?\b",
                     re.I)
WORD = re.compile(r"profil", re.I)


def in_scope(rel):
    if any(rel == d or rel.startswith(d + "/") for d in EXEMPT_DIRS):
        return False
    if "target" in rel.split("/"):             # Cargo's build directories
        return False
    return not EXEMPT_FILES.search(rel)


def files():
    for top in SCOPE:
        full = os.path.join(ROOT, top)
        if os.path.isfile(full):
            yield top
            continue
        for root, dirs, fs in os.walk(full):
            rel_root = os.path.relpath(root, ROOT)
            dirs[:] = sorted(d for d in dirs if in_scope(os.path.join(rel_root, d)))
            for f in sorted(fs):
                rel = os.path.join(rel_root, f)
                if in_scope(rel):
                    yield rel


def frozen_chunks():
    """A passed chunk's definition is frozen: its criterion report hashes the generated chunk file (plan_check), so
    its file and its `chunk(...)` call in chunks.py keep the words it passed with. Returns (files, chunks.py lines).
    A checkout without the delivery plan has no chunks to exempt."""
    sys.path.insert(0, HERE)
    import planning
    if not planning.optional(planning.PLAN_TOOLS, "terms", "the delivery plan's documents and the exemption of passed chunks' definitions"):
        return set(), set()
    status = json.load(open(os.path.join(ROOT, "docs", "delivery", "status.json"), encoding="utf-8"))
    passed = {u for u, v in status.items() if isinstance(v, dict) and v.get("status") == "passed"}
    files = {f"docs/delivery/chunks/{f}" for f in os.listdir(os.path.join(ROOT, "docs", "delivery", "chunks"))
             if re.match(r"^(DEL-\d+)-", f) and re.match(r"^(DEL-\d+)-", f).group(1) in passed}
    lines = set()
    src = open(os.path.join(ROOT, "docs", "delivery", "tools", "chunks.py"), encoding="utf-8").read().split("\n")
    current = None
    for i, l in enumerate(src, 1):
        m = re.match(r'^chunk\(id="([\w-]+)"', l)
        if m:
            current = m.group(1)
        elif l and not l[0].isspace() and not l.startswith(")"):
            current = None
        if current in passed:
            lines.add(i)
    return files, lines


FROZEN_FILES, FROZEN_LINES = frozen_chunks()


# The register's prose (outside its rows) that used the old words before the rename: these lines, by content, keep
# their words. A position would not do: new prose written next to old prose would inherit its exemption.
REGISTER_HISTORY = {
    "80c5348d47b16df1", "63ffb1d6d447afe7", "26e62c8f8ce6433f", "7f33f93a5ab4ea28", "f4204200e05c8d81",
    "16bbf9ea9f824088", "c9482baded205c43", "79555308be3ccd9b", "0882b61a5e718d6a", "8c8442432bffbcbd",
    "443bbfeffab7faf9", "5fcbaeb58806eff1", "7a5f4386152777aa", "18c9e088e10451ad",
}


def register_exempt(rel, line, i, first_new):
    """The register's history stays as written: any row numbered below X-150, wherever it stands, and the fourteen
    lines of prose that predate the rename, by their content. Every other line, wherever it stands, is checked."""
    if rel != "docs/Fortis_Specification_Review.md":
        return False
    m = re.match(r"^\|\s*X-(\d+)\s*\|", line)
    if m:
        return int(m.group(1)) < 150
    return hashlib.sha256(line.strip().encode("utf-8")).hexdigest()[:16] in REGISTER_HISTORY


def main(argv):
    findings, scanned = [], 0
    for rel in files():
        if WORD.search(rel):
            findings.append((rel, 0, "the path names the old word"))
        if not TEXT.search(os.path.basename(rel)):
            continue
        try:
            text = open(os.path.join(ROOT, rel), encoding="utf-8").read()
        except (UnicodeDecodeError, OSError):
            continue
        scanned += 1
        if rel in FROZEN_FILES:
            continue
        defect = None
        first_new = next((k for k, l in enumerate(text.split("\n"), 1) if (m := re.match(r"^\|\s*X-(\d+)\s*\|", l)) and int(m.group(1)) >= 150), None)
        for i, line in enumerate(text.split("\n"), 1):
            # the defect register, like the decision register, keeps an entry's words: entries from D-50 use the new ones
            dm = re.match(r'^\s*"id":\s*"D-(\d+)"', line) if rel == "docs/delivery/defects.json" else None
            if dm:
                defect = int(dm.group(1))
            if defect is not None and defect < 50:
                continue
            if not WORD.search(line) or register_exempt(rel, line, i, first_new):
                continue
            if rel == "docs/delivery/tools/chunks.py" and i in FROZEN_LINES:
                continue
            rest = ALLOWED.sub("", line)
            # naming other toolchains' term, and quoting the old names where the rename itself is stated
            rest = re.sub(r"\bcall(?:ed)? build profiles\b", "", rest, flags=re.I)
            if re.search(r"\brenam", line, re.I):
                rest = re.sub(r'`[^`]*`', lambda m: "" if re.search("profil", m.group(0), re.I) else m.group(0), rest)
                rest = re.sub(r'"[^"`]*profil[^"`]*"', "", rest, flags=re.I)
            # Cargo's `[profile.…]` table, in a Cargo.toml or in the text of one a tool generates
            rest = re.sub(r"\[profile\.[\w.-]+\]", "", rest)
            if WORD.search(rest):
                findings.append((rel, i, line.strip()[:160]))
    for rel, line, text in findings:
        print(f"FAIL {rel}:{line}: {text}")
    by_file = {}
    for rel, _, _ in findings:
        by_file[rel] = by_file.get(rel, 0) + 1
    if "-v" in argv:
        for rel, n in sorted(by_file.items(), key=lambda kv: -kv[1]):
            print(f"{n:5} {rel}")
    print(f"\n{scanned} files in scope; {len(findings)} uses of the old word outside the profiling sense in {len(by_file)} files")
    if "--json" in argv:
        os.makedirs(os.path.join(ROOT, "build"), exist_ok=True)
        json.dump({"rule": "PROG-PRD-38", "chunk": "DEL-49", "findings": [{"file": f, "line": l, "text": t} for f, l, t in findings]},
                  open(os.path.join(ROOT, "build", "terms.json"), "w", encoding="utf-8"), indent=1)
    print("terms check: " + ("OK" if not findings else "FAILED"))
    return 0 if not findings else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
