"""Repository hygiene: what a public checkout of Fortis must never carry.

    python3 -I conformance/tools/hygiene_check.py [--json] [-v] [--root DIR]

It reads the tracked files (`git ls-files`, or, in a tree that is not a git checkout, every file outside build
output), less the maintainers' internal files when this checkout has them (the set is defined in the maintainers'
docs/delivery/tools/public_tree.py; a public checkout has no internal files). It fails on:

1. **An absolute local path**: a home directory (`/home/NAME/…`, `/Users/NAME/…`), an agent's scratch directory
   (`/tmp/claude-…`), or this checkout's own absolute path. Tools write paths relative to the repository root. A
   made-up name in an example or a test (ILLUSTRATIVE) is not a local path.
2. **A build product**: a file under a `target/`, `__pycache__/`, `node_modules/` or `.fortis/` directory, under
   the root `build/` or `website/dist/`, or a compiled artefact (`.rlib`, `.rmeta`, `.pyc`, `.o`, `.so`, …).
3. **A file over 2 MB**, unless LARGE_OK names it with the reason it is kept.
4. **A Markdown link that does not resolve**: every link in a `.md` file, outside code, must name a file or
   directory in the checkout, and never one of the internal files (they are not published).
5. **A tracked file that .gitignore ignores** (in a git checkout): a repository started afresh from the tree with
   `git add .` would leave it out.

`--json` writes the findings to build/hygiene.json. `--root DIR` checks the tree at DIR instead of this checkout,
for example a release's exported public tree.
"""
from __future__ import annotations

import importlib.util
import json
import os
import re
import subprocess
import sys
import urllib.parse

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
LIMIT = 2 * 1024 * 1024

# Directories that hold build output wherever they are, and directories that do only at the repository root (a
# `build` directory elsewhere is source: conformance/build holds fixtures, fortis-mir/src/build holds code).
BUILD_DIRS = {"target", "__pycache__", "node_modules", ".fortis"}
ROOT_BUILD = ("build/", "website/dist/")
BUILD_SUFFIXES = (".rlib", ".rmeta", ".pyc", ".pyo", ".o", ".obj", ".so", ".dylib", ".dll", ".exe", ".class", ".a",
                  ".lib", ".pdb")

# Files kept although they are over the limit, each with its reason.
LARGE_OK = {
    "conformance/generated/fortis_parser.py": "the Python parser generated from the grammar by gen_parser.py; committed "
                                              "so that the reference tools run without a build step, and the "
                                              "genparser step checks it against the recogniser",
}

# Names that examples and tests use for someone else's home directory.
ILLUSTRATIVE = {"user", "username", "alice", "bob", "carol", "other", "me", "you", "someone", "example", "name",
                "runner", "dev", "fortis"}
HOME_PATH = re.compile(r"(?<![\w.~-])/(?:home|Users)/([A-Za-z0-9._-]+)(?=/|\b)")
SCRATCH_PATH = re.compile(r"/tmp/claude-[0-9]+")

LINK = re.compile(r"(?<!!)\[(?:[^\[\]]|\[[^\]]*\])*\]\(\s*<?([^)\s>]+)>?(?:\s+\"[^\"]*\")?\s*\)")
IMAGE = re.compile(r"!\[[^\]]*\]\(\s*<?([^)\s>]+)>?(?:\s+\"[^\"]*\")?\s*\)")
REFDEF = re.compile(r"^\s{0,3}\[[^\]]+\]:\s*<?(\S+?)>?(?:\s+.*)?$")


def build_product(path: str) -> bool:
    """Whether repository-relative `path` is build output."""
    parts = path.split("/")
    return path.startswith(ROOT_BUILD) or any(p in BUILD_DIRS for p in parts[:-1]) or path.endswith(BUILD_SUFFIXES)


def internal_set():
    """is_internal(path) from the maintainers' public_tree.py, or None in a checkout without it."""
    p = os.path.join(ROOT, "docs", "delivery", "tools", "public_tree.py")
    if not os.path.exists(p):
        return None
    spec = importlib.util.spec_from_file_location("public_tree", p)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod.is_internal


def git_files():
    """The tracked files, or None when ROOT is not the top of a git checkout."""
    try:
        top = subprocess.run(["git", "rev-parse", "--show-toplevel"], cwd=ROOT, capture_output=True, text=True, timeout=60)
        if top.returncode != 0 or os.path.realpath(top.stdout.strip()) != os.path.realpath(ROOT):
            return None
        out = subprocess.run(["git", "ls-files", "-z"], cwd=ROOT, capture_output=True, timeout=120, check=True).stdout
    except (OSError, subprocess.SubprocessError):
        return None
    return [p for p in out.decode("utf-8").split("\0") if p]


def git_ignored():
    """Tracked files that .gitignore matches (only asked in a git checkout)."""
    try:
        out = subprocess.run(["git", "ls-files", "-z", "-i", "-c", "--exclude-standard"], cwd=ROOT, capture_output=True, timeout=120, check=True).stdout
    except (OSError, subprocess.SubprocessError):
        return []
    return [p for p in out.decode("utf-8").split("\0") if p]


def walked_files():
    out = []
    for base, dirs, files in os.walk(ROOT):
        rel_base = os.path.relpath(base, ROOT)
        rel_base = "" if rel_base == "." else rel_base + "/"
        dirs[:] = sorted(d for d in dirs if d != ".git" and not build_product(rel_base + d + "/x"))
        out += [rel_base + f for f in sorted(files)]
    return out


def strip_code(text):
    """The text with fenced blocks and code spans blanked, line numbers kept."""
    out, fence = [], None
    for line in text.split("\n"):
        s = line.strip()
        m = re.match(r"^(`{3,}|~{3,})", s)
        if fence:
            if m and s.startswith(fence):
                fence = None
            out.append("")
            continue
        if m:
            fence = m.group(1)
            out.append("")
            continue
        out.append(re.sub(r"(`+).+?\1", "", line))
    return out


def link_targets(text):
    for n, line in enumerate(strip_code(text), 1):
        for rx in (LINK, IMAGE, REFDEF):
            for m in rx.finditer(line):
                yield n, m.group(1)


def check_link(page, target, files, is_internal):
    if re.match(r"^[a-zA-Z][a-zA-Z0-9+.-]*:", target) or target.startswith(("#", "//")):
        return None
    if any(c in target for c in "<>{}") or target in ("...", "…"):
        return None     # a placeholder in a template
    path = urllib.parse.unquote(target.split("#", 1)[0].split("?", 1)[0])
    if not path:
        return None
    full = os.path.normpath(os.path.join(ROOT, path.lstrip("/")) if path.startswith("/")
                            else os.path.join(os.path.dirname(os.path.join(ROOT, page)), path))
    rel = os.path.relpath(full, ROOT).replace(os.sep, "/")
    if rel.startswith("../"):
        return f"link to {target} leaves the repository"
    if is_internal and (is_internal(rel) or is_internal(rel + "/")):
        return f"link to {target} names an internal file ({rel}), which is not published"
    if not os.path.exists(full):
        return f"link to {target}: {rel} is not in the checkout"
    return None


def main(argv):
    global ROOT
    if "--root" in argv:
        ROOT = os.path.abspath(argv[argv.index("--root") + 1])
    verbose = "-v" in argv
    is_internal = internal_set()
    files = git_files()
    source = "tracked files"
    if files is None:
        files = walked_files()
        source = "files (not a git checkout: every file outside build output)"
    public = [p for p in files if not (is_internal and is_internal(p))]
    findings = []

    def find(kind, path, line, detail):
        findings.append({"kind": kind, "path": path, "line": line, "detail": detail})

    local_roots = {r for r in (ROOT, os.path.realpath(ROOT), os.path.expanduser("~")) if r and r not in ("/", "~")}
    for p in public:
        full = os.path.join(ROOT, p)
        if os.path.islink(full) or not os.path.exists(full):
            continue
        size = os.path.getsize(full)
        if build_product(p):
            find("build product", p, 0, "build output is not committed: remove it and let .gitignore keep it out")
        if size > LIMIT and p not in LARGE_OK:
            find("large file", p, 0, f"{size / 1e6:.1f} MB, over the 2 MB limit (LARGE_OK names the files kept, with the reason)")
        data = open(full, "rb").read()
        if b"\0" in data[:8192]:
            continue
        text = data.decode("utf-8", errors="replace")
        for n, line in enumerate(text.split("\n"), 1):
            hits = [m.group(0) for m in HOME_PATH.finditer(line) if m.group(1).lower() not in ILLUSTRATIVE]
            hits += SCRATCH_PATH.findall(line) + [r for r in local_roots if r in line]
            if hits:
                find("absolute path", p, n, f"{sorted(set(hits))[0]}: write paths relative to the repository root")
        if p.endswith(".md"):
            for n, target in link_targets(text):
                why = check_link(p, target, files, is_internal)
                if why:
                    find("markdown link", p, n, why)
    if source == "tracked files":
        for p in git_ignored():
            if not (is_internal and is_internal(p)):
                find("ignored file", p, 0, "tracked, but .gitignore matches it: a repository started afresh with `git add .` would leave it out")
    for f in findings:
        print(f"FAIL {f['kind']}: {f['path']}" + (f":{f['line']}" if f["line"] else "") + f": {f['detail']}")
    kinds = {}
    for f in findings:
        kinds[f["kind"]] = kinds.get(f["kind"], 0) + 1
    scope = f"{len(public)} {source}" + (f" ({len(files) - len(public)} internal files set apart)" if is_internal else
                                         " (no internal set in this checkout: every file is public)")
    print(f"hygiene: {scope}; " + (", ".join(f"{v} {k}" for k, v in sorted(kinds.items())) if kinds else "no findings"))
    if verbose:
        print(f"allowed over 2 MB: {', '.join(sorted(LARGE_OK))}")
    if "--json" in argv:
        os.makedirs(os.path.join(ROOT, "build"), exist_ok=True)
        json.dump({"files": len(public), "source": source, "findings": findings}, open(os.path.join(ROOT, "build", "hygiene.json"), "w"), indent=1)
    print("hygiene check: " + ("OK" if not findings else "FAILED"))
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
