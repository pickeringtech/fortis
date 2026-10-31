#!/usr/bin/env python3
"""One conservative path table for CI and gate.py --tier quick --changed BASE.

Only PATH_RULES can narrow a run. Order matters: rendered pages precede process
documents; tools and data under a documentation directory are not prose.
See docs/delivery/ci-change-aware.md for the audit of the steps' inputs.
"""
import argparse
import json
import os
from pathlib import Path
import posixpath
import re
import subprocess
import sys
from urllib.parse import unquote

sys.path.insert(0, str(Path(__file__).resolve().parent))
import gate

ROOT = Path(__file__).resolve().parents[2]
DOC_CHECKS = {"syntax", "plan", "cite", "terms", "hygiene"}
DOC_STEPS = DOC_CHECKS | {"unit", "cargo-build-release", "docs", "docset", "website", "site", "libref", "limitations",
                         "showcase", "shapes", "checker"}
CLASS_STEPS = {"process": DOC_CHECKS, "documentation": DOC_STEPS, "tests": {"unit", "cargo-test"}}
CLASS_ORDER = ("process", "documentation", "tests", "full")

# (full-match regular expression, class, why). This is the only path-to-impact table.
# Unknown Markdown is deliberately not a catch-all: newly added areas need review.
PATH_RULES = (
    (r"spikes/SP-01/README\.md", "full", "parity reads this ownership-corpus inventory"),
    (r"(?:README\.md|docs/(?:guide|agent)/.+\.(?:md|html|txt)|docs/Fortis_(?:Specification|Language_Reference|Platform_Specification|Delivery_Programme)\.md|skills/fortis-code/SKILL\.md)",
     "documentation", "docset/cite inventories, extract.DOCS and site.SPEC_PAGES"),
    (r"docs/releases/.+\.md", "documentation", "website.EXTRA_RENDER renders each release's notes"),
    (r"docs/delivery/(?:defects|triage)\.json", "documentation", "docset known defects and limitations inputs"),
    (r"(?:tests/(?:README|cases/README)|acceptance/README|benchmarks/(?:README|PREREGISTRATION-DEL-(?:40|43))|"
     r"compiler/experimental-lowering/README|conformance/(?:README|ambiguity/README|api/README|codec/FORMAT|"
     r"differential/TEMPLATE-decision-record|doc-contexts/README|doc-packages/README|generated/lookahead)|"
     r"examples/(?:README|showcase/README)|interpreter/(?:ARCHITECTURE|INTRINSICS)|pilot/(?:README|FRICTION)|"
     r"website/README|spikes/.+)\.md",
     "process", "other named Markdown outside the checked/rendered documentation inventories"),
    (r"(?:tests/.+|compiler/crates/[^/]+/tests/.+|compiler/crates/fortis-testkit/.+)",
     "tests", "Python/shared cases or Rust test sources"),
    (r"(?:docs/(?:delivery|reviews|releases)/.+\.md|docs/Fortis_Specification_Review\.md|AGENTS\.md|CLAUDE\.md|CONTRIBUTING\.md|SECURITY\.md)",
     "process", "process prose outside the checked/rendered documentation inventories"),
)


def git(tree, *args):
    return subprocess.run(["git", *args], cwd=tree, check=True, capture_output=True, timeout=60).stdout


def changed_paths(tree, base, head=None):
    """Include both sides of renames and deletions, with NUL-safe path handling.

    CI supplies the PR head explicitly (not Actions' synthetic merge commit).
    Locally, diff the working tree against the merge base, including staged edits.
    Untracked files are included locally so a new source cannot silently escape.
    """
    merge_base = os.fsdecode(git(tree, "merge-base", base, head or "HEAD")).strip()
    args = ["diff", "--name-only", "-z", "--no-renames", merge_base]
    if head:
        args.append(head)
    paths = git(tree, *args, "--").split(b"\0")
    if head is None:
        paths += git(tree, "ls-files", "--others", "--exclude-standard", "-z").split(b"\0")
    return sorted({os.fsdecode(p) for p in paths if p}), merge_base


def cited_tests(tree, paths):
    """Test paths mentioned or linked by tracked Markdown need the docs checks too.

    Resolve relative links even when their targets were deleted; a link to a test
    directory also covers its children. No compiler or documentation build is needed.
    """
    cited = set()
    for raw in git(tree, "ls-files", "-z", "*.md").split(b"\0"):
        if not raw:
            continue
        page = os.fsdecode(raw)
        file = Path(tree, page)
        if not file.is_file():
            continue
        text = file.read_text(encoding="utf-8")
        targets = [posixpath.normpath(posixpath.join(posixpath.dirname(page), unquote(t.split("#")[0])))
                   for t in re.findall(r"\]\(<?([^\s)>]+)", text) if not re.match(r"[a-z]+:", t)]
        for path in paths:
            if path in text or any(path == t or path.startswith(t.rstrip("/") + "/") for t in targets):
                cited.add(path)
    return cited


def classify(path):
    for pattern, kind, why in PATH_RULES:
        if re.fullmatch(pattern, path):
            return kind, why
    return "full", "unrecognised path: the full quick tier"


def plan(paths, cited=(), force_full=False):
    reasons = []
    for path in sorted(set(paths)):
        kind, why = classify(path)
        reasons.append({"path": path, "class": kind, "why": why})
    classes = {r["class"] for r in reasons}
    if force_full or not reasons:
        classes.add("full")
        reasons.append({"path": None, "class": "full", "why": "manual run or empty diff: the full quick tier"})
    selected = set().union(*(CLASS_STEPS.get(c, set()) for c in classes))
    for r in reasons:
        if r["class"] == "tests" and r["path"] in cited:
            selected |= DOC_CHECKS
            r["why"] += "; cited by Markdown: also run the docs checks"
    steps = [nm for nm, _, _ in gate.tier_steps("quick") if "full" in classes or nm in selected]
    build = "cargo-build-release" in steps
    # Small plans fit one Python shard. Documentation gets three toolchain shards;
    # the full plan keeps today's 14-way split and build overlap unchanged.
    n, py = (14, 3) if "full" in classes else (4, 1) if build else (1, 1)
    return {"version": 1, "tier": "quick", "classes": [c for c in CLASS_ORDER if c in classes],
            "steps": steps, "build": build, "rust_tests": "cargo-test" in steps,
            "shards": n, "py_shards": py, "build_seconds": 100 if build else 0, "reasons": reasons}


def from_diff(tree, base, head=None):
    paths, merge_base = changed_paths(tree, base, head)
    tests = [p for p in paths if classify(p)[0] == "tests"]
    result = plan(paths, cited_tests(tree, tests) if tests else ())
    result["merge_base"] = merge_base
    return result


def describe(result):
    print("change plan: " + ", ".join(result["classes"]), flush=True)
    print("steps: " + ", ".join(result["steps"]), flush=True)
    print(f"jobs: build={result['build']}, rust-tests={result['rust_tests']}, shards={result['shards']}", flush=True)
    for r in result["reasons"]:
        print(f"  {r['path']!r}: {r['class']} ({r['why']})", flush=True)


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--base")
    ap.add_argument("--head")
    ap.add_argument("--full", action="store_true")
    ap.add_argument("--json", type=Path)
    ap.add_argument("--github-output", type=Path)
    args = ap.parse_args(argv)
    if not args.full and not args.base:
        ap.error("provide --base or --full")
    result = plan([], force_full=True) if args.full else from_diff(ROOT, args.base, args.head)
    describe(result)
    if args.json:
        args.json.parent.mkdir(parents=True, exist_ok=True)
        args.json.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    if args.github_output:
        outputs = {"plan": result, "build": result["build"], "rust_tests": result["rust_tests"],
                   "python_matrix": list(range(1, result["py_shards"] + 1)),
                   "toolchain_matrix": list(range(result["py_shards"] + 1, result["shards"] + 1))}
        with args.github_output.open("a", encoding="utf-8") as out:
            for key, value in outputs.items():
                out.write(f"{key}={json.dumps(value, separators=(',', ':'))}\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
