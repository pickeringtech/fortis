"""DEL-49 criterion 2: every example of the documentation set runs as a fixture through the real toolchain
(PROG-TST-08 over the documentation, PROG-AGT-07).

  python3 -I conformance/tools/docset_check.py [--json] [--release] [--page PATH] [--backends B,...] [--shard I/N] [-j N] [-v]

The documentation set is `README.md` and every Markdown file under `docs/guide` (human mode) and `docs/agent`
(agent mode). Each fenced block is judged by its language and its first line:

- A `fortis` block starts with one marker, as in the Language Reference (section 0):
  - `// accept`, optionally `// accept (exit status N)`: `fortisc check` and the reference checker
    (`bin/fortis check`) report nothing at all, and the program runs on all three backends: the definitional
    interpreter (`bin/fortis run`), the development VM (`fortisc vm`) and native code (`fortisc build`, then the
    executable). Each backend must exit with status N (0 when not given) and print the same standard output, and
    when the next fenced block is an `output` block the output must be exactly its text; a program that prints
    something must show it, so an `accept` block whose program prints with no `output` block fails. A module
    without `main` must declare tests, and a module's tests (with or without `main`) pass on all three
    (`bin/fortis test`, `fortisc test`, `fortisc native --tests`). A positive example therefore always runs.
  - `// reject: E-XXX-NNN, …`: every line that carries `// reject: E-XXX-NNN` (the marked lines) must be
    reported with exactly those codes by both checkers, and nothing else may be reported. An `output` block
    after it is `fortisc check --format text`'s rendering; a `records` block is its JSON lines; the file is
    called `example.ft` (or its `// file:` name) in both.
  - `// illustrative`: a shape with placeholders, not a program. It must not parse as a module, so that no
    complete program escapes being run.
  A second line `// file: NAME.ft` names a single module; `// file: PKG/src/PATH.ft` makes the block a module of
  the package `PKG`, whose manifest is a `yaml` block starting `# file: PKG/project.yaml`. A package is checked,
  run (from its `entry`) and tested as a directory, with every block of one page that names it; its `output`
  block follows its last block.
- A `yaml` block is either a package manifest (`# file: …`) or starts `# illustrative`.
- An `output` or `records` block belongs to the `fortis` block before it (only blank lines between).

Every check is attributed to the tool it judges: `checker` (the two static checkers), `interpreter`, `vm`,
`native`, or `docs` (the page itself). A failing fixture is KNOWN when every one of its failing checks is of a
tool that an open release-blocking defect of the defect register names for it
(`"results": {"fixtures": ["docset:PAGE#N"], "backends": [tools]}`); a `docs` failure is never KNOWN. The defect
register is a maintainers' planning file: where it is present, the open defects this check reads are written to
`conformance/known-defects/docset.json`, and a checkout without the register reads that copy instead. A KNOWN
result is not a pass: the run exits 0 with KNOWN results (the merge gate), and with `--release` (DEL-49 criterion
2 at the release check) it exits 1, as it does for every other failure.

The report goes to `build/docset.json` (`--json`). Generated files go under `build/docset/`.
"""
from __future__ import annotations

import argparse
import hashlib
import concurrent.futures
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
sys.path.insert(0, HERE)
import planning  # noqa: E402

# the open release-blocking defects that name documentation examples, as this check reads them from the defect
# register: written when the register is present, read when it is not
KNOWN_COPY = os.path.join(ROOT, "conformance", "known-defects", "docset.json")
FORTISC = os.path.join(ROOT, "compiler", "target", "release", "fortisc")
FORTIS = os.path.join(ROOT, "bin", "fortis")
WORK = os.path.join(ROOT, "build", "docset")
BACKENDS = ("interpreter", "vm", "native")
# the backends this run judges (`--backends`, the gate's quick tier); a release run judges all three
RUN_BACKENDS = BACKENDS
CODE = r"E-[A-Z]{2,3}-\d{3}"
TIMEOUT = 900


def doc_pages():
    """The documentation set, in a stable order: the README, then the guide, then the agent mode."""
    pages = ["README.md"]
    for d in ("docs/guide", "docs/agent"):
        for root, _, files in sorted(os.walk(os.path.join(ROOT, d))):
            for f in sorted(files):
                if f.endswith(".md"):
                    pages.append(os.path.relpath(os.path.join(root, f), ROOT))
    return [p for p in pages if os.path.exists(os.path.join(ROOT, p))]


FENCE = re.compile(r"^(\s*)(`{3,}|~{3,})(.*)$")


def fences(text):
    """Every fenced block, at any indentation (a fence inside a list item is still a fence): its language, the
    line of its content, and its content lines with the fence's indentation removed. A `~~~` fence, a fence with
    an info string that is not one word, and an unclosed fence are returned as `bad` so the run fails on them:
    no block may escape being judged by being written in a form this reader would skip."""
    lines = text.split("\n")
    out, i = [], 0
    while i < len(lines):
        if re.match(r"^\s*(?:>\s*)+(`{3,}|~{3,})", lines[i]):
            # a fence inside a block quote, at any depth, renders as code but would not be judged: refuse it
            out.append({"lang": "", "start": i + 1, "open": i, "close": i, "lines": [], "bad": "a fence inside a block quote: write examples outside quotes"})
            i += 1
            continue
        m = FENCE.match(lines[i])
        if not m:
            i += 1
            continue
        indent, tick, info = m.group(1), m.group(2), m.group(3).strip()
        j = i + 1
        while j < len(lines) and not (lines[j].strip() == tick and lines[j].startswith(indent)):
            j += 1
        body = [l[len(indent):] if l.startswith(indent) else l.lstrip() for l in lines[i + 1:j]]
        bad = None
        if tick.startswith("~"):
            bad = "a `~~~` fence: write fenced blocks with backticks"
        elif info and not re.fullmatch(r"[\w-]+", info):
            bad = f"a fence whose info string is not one word ({info!r})"
        elif j >= len(lines):
            bad = "an unclosed fence"
        out.append({"lang": info, "start": i + 1, "open": i, "close": j, "lines": body, "bad": bad})
        i = j + 1
    # the block after which only blank lines stand before the next fence is its neighbour
    for a, b in zip(out, out[1:]):
        a["next"] = b if all(not l.strip() for l in lines[a["close"] + 1:b["open"]]) else None
    if out:
        out[-1]["next"] = None
    return out


MARK = re.compile(r"^//\s*(accept|reject|illustrative)\b(.*)$")


def judge_marker(first):
    """(verdict, codes, exit status, error) from a block's first line."""
    m = MARK.match(first.strip())
    if not m:
        return None, [], 0, f"the first line must be `// accept`, `// reject: CODE` or `// illustrative`, not {first.strip()!r}"
    verdict, rest = m.group(1), m.group(2).strip()
    if verdict == "accept":
        if not rest:
            return "accept", [], 0, None
        e = re.fullmatch(r"\(exit status (\d+)\)", rest)
        if e:
            return "accept", [], int(e.group(1)), None
        return None, [], 0, f"`// accept` takes nothing but `(exit status N)`, not {rest!r}"
    if verdict == "reject":
        m2 = re.match(rf"^:\s*({CODE}(?:\s*,\s*{CODE})*)\s*(?:—.*)?$", rest)
        if not m2:
            return None, [], 0, f"`// reject:` must name codes, not {rest!r}"
        return "reject", re.findall(CODE, m2.group(1)), 0, None
    if rest and not rest.startswith("—"):
        return None, [], 0, f"`// illustrative` takes only a `— comment`, not {rest!r}"
    return "illustrative", [], 0, None


def release_counts():
    """The release check's count (DW-11): over README.md, docs/guide/** and docs/agent/** (.md, .html, .txt), the
    fortis blocks whose first line is not `// illustrative` (examples) and those whose first line starts with
    `// reject` (negatives)."""
    files = ["README.md"]
    for d in ("docs/guide", "docs/agent"):
        for root, _, fs in sorted(os.walk(os.path.join(ROOT, d))):
            files += [os.path.relpath(os.path.join(root, f), ROOT) for f in sorted(fs) if f.endswith((".md", ".html", ".txt"))]
    n = m = 0
    for f in files:
        for b in fences(open(os.path.join(ROOT, f), encoding="utf-8").read()):
            if b["lang"] == "fortis":
                first = b["lines"][0].strip() if b["lines"] else ""
                n += first != "// illustrative"
                m += first.startswith("// reject")
    return n, m


def slug(page):
    """A page's name for its fixtures: readable, a valid module name, and distinct for distinct pages (two pages
    whose names differ only in punctuation or directory get different digests)."""
    readable = re.sub(r"[^a-z0-9]+", "_", os.path.splitext(page)[0].lower()).strip("_")
    return f"{readable}_{hashlib.sha256(page.encode('utf-8')).hexdigest()[:6]}"


def run(cmd, cwd=ROOT, env=None, timeout=TIMEOUT):
    e = dict(os.environ)
    e.update(env or {})
    try:
        r = subprocess.run(cmd, capture_output=True, text=True, cwd=cwd, env=e, timeout=timeout)
        return r.returncode, r.stdout, r.stderr
    except subprocess.TimeoutExpired:
        return 124, "", f"timeout after {timeout} s"


def spellings(path):
    """A path as the tools may print it: as given, as its symbolic links resolve, and each escaped in a JSON string
    (where a name outside ASCII is written \\uXXXX); longest first, so that no spelling is cut by a shorter one."""
    return sorted({f for p in (path, os.path.realpath(path)) for f in (p, json.dumps(p)[1:-1])}, key=len, reverse=True)


ROOTS = spellings(ROOT)
HOME = os.path.expanduser("~")
HOMES = spellings(HOME) if HOME not in ("", "/", "~") else []


def rooted(t):
    """`t` with this tree's path written `<root>`. A check's detail is rooted before it is cut to a length for
    display (`head`, `tail`), so the cut keeps the same text wherever the tree is checked out."""
    for p in ROOTS:
        t = t.replace(p, "<root>")
    return t


def head(t, n):
    """The first n characters of a detail, rooted first."""
    return rooted(t)[:n]


def tail(t, n):
    """The last n characters of a detail, rooted first."""
    return rooted(t)[-n:]


def records(out):
    """The records of a stream, strictly: every nonblank line must be one JSON object. Returns (records, the lines
    that are not), so a shown `records` block or a checker's output with anything else in it cannot match."""
    recs, bad = [], []
    for l in out.splitlines():
        if not l.strip():
            continue
        try:
            r = json.loads(l)
        except ValueError:
            bad.append(l)
            continue
        if isinstance(r, dict):
            recs.append(r)
        else:
            bad.append(l)
    return recs, bad


def collect(pages):
    """Fixtures: single modules and packages, each with its blocks, verdict, expected output and problems."""
    fixtures, problems = [], []
    for page in pages:
        text = open(os.path.join(ROOT, page), encoding="utf-8").read()
        blocks = fences(text)
        n = 0
        packages = {}
        for b in blocks:
            if b["bad"]:
                problems.append({"id": f"{page}:{b['start']}", "line": b["start"], "detail": b["bad"]})
                continue
            if b["lang"] == "fortis":
                n += 1
                fid = f"{page}#{n}"
                if not b["lines"]:
                    problems.append({"id": fid, "line": b["start"], "detail": "an empty fortis block"})
                    continue
                verdict, codes, status, err = judge_marker(b["lines"][0])
                if err:
                    problems.append({"id": fid, "line": b["start"], "detail": err})
                    continue
                fm = re.match(r"^//\s*file:\s*(\S+)\s*$", b["lines"][1]) if len(b["lines"]) > 1 else None
                name = fm.group(1) if fm else None
                after = b.get("next")
                expect = None
                if after and after["lang"] in ("output", "records"):
                    expect = (after["lang"], "\n".join(after["lines"]))
                    after["owned"] = True
                src = "\n".join(b["lines"]) + "\n"
                fx = {"id": fid, "page": page, "line": b["start"], "verdict": verdict, "codes": codes, "exit": status, "expect": expect, "src": src,
                      "source": hashlib.sha256(src.encode("utf-8")).hexdigest()[:12]}
                if name and "/" in name and not re.fullmatch(r"[a-z][a-z0-9_.]*/src/(?:[a-z_][a-z0-9_]*/)*[a-z_][a-z0-9_]*\.ft", name):
                    # a module of a package is a canonical `.ft` path under src/, the only files the checkers load
                    problems.append({"id": fid, "line": b["start"], "detail": f"`// file: {name}` is not a canonical module path PKG/src/PATH.ft"})
                    continue
                if name and "/src/" in name:
                    pkg = name.split("/")[0]
                    p = packages.setdefault(pkg, {"id": f"{page}#{pkg}", "page": page, "line": b["start"], "package": pkg, "files": {}, "blocks": [], "expect": None, "manifest": None})
                    if name in p["files"]:
                        problems.append({"id": fid, "line": b["start"], "detail": f"{name} is written twice on this page; each file of a package example appears once"})
                        continue
                    p["files"][name] = src
                    p["blocks"].append(fx)
                    if expect:
                        if p["expect"]:
                            problems.append({"id": fid, "line": b["start"], "detail": f"package {pkg} shows its output twice; it belongs after its last block"})
                        p["expect"] = expect
                else:
                    fx["display"] = name or "example.ft"
                    fixtures.append(fx)
            elif b["lang"] == "yaml":
                first = b["lines"][0] if b["lines"] else ""
                fm = re.match(r"^#\s*file:\s*(\S+)/project\.yaml\s*$", first)
                if fm:
                    p = packages.setdefault(fm.group(1), {"id": f"{page}#{fm.group(1)}", "page": page, "line": b["start"], "package": fm.group(1), "files": {}, "blocks": [], "expect": None, "manifest": None})
                    if p["manifest"] is not None:
                        problems.append({"id": p["id"], "line": b["start"], "detail": f"package {fm.group(1)} has two manifests on this page"})
                    p["manifest"] = "\n".join(b["lines"][1:]) + "\n"
                elif not first.startswith("# illustrative"):
                    problems.append({"id": f"{page}:{b['start']}", "line": b["start"], "detail": "a yaml block must start `# file: PKG/project.yaml` or `# illustrative`"})
        for b in blocks:
            if b["lang"] in ("output", "records") and not b.get("owned"):
                problems.append({"id": f"{page}:{b['start']}", "line": b["start"], "detail": f"an `{b['lang']}` block that follows no fortis block"})
        for pkg, p in packages.items():
            if p["manifest"] is None:
                problems.append({"id": p["id"], "line": p["line"], "detail": f"package {pkg} has no `# file: {pkg}/project.yaml` block"})
                continue
            verdicts = {b["verdict"] for b in p["blocks"]}
            p["source"] = hashlib.sha256((p["manifest"] + "".join(p["files"][k] for k in sorted(p["files"]))).encode("utf-8")).hexdigest()[:12]
            p["verdict"] = "reject" if "reject" in verdicts else "accept"
            if "reject" in verdicts and "accept" in verdicts:
                # the release counts each `// accept` module as an example that runs; in a rejected package it never would
                problems.append({"id": p["id"], "line": p["line"], "detail": "a rejected package holds `// accept` modules, which would never run: put them in an accepted package the rejected one depends on"})
            p["exit"] = max((b["exit"] for b in p["blocks"]), default=0)
            fixtures.append(p)
    return fixtures, problems


def destinations(fx):
    """The files a fixture writes, and its native executable."""
    if "package" in fx:
        d = os.path.join(WORK, slug(fx["page"]), "packages")
        return [os.path.join(d, fx["package"]), os.path.join(WORK, "bin", fx["package"])] + [os.path.join(d, n) for n in fx["files"]]
    n = fx["id"].rsplit("#", 1)[1]
    return [os.path.join(WORK, slug(fx["page"]), f"{slug(fx['page'])}_{n}.ft"), os.path.join(WORK, "bin", f"{slug(fx['page'])}_{n}")]


def write_single(fx):
    d = os.path.join(WORK, slug(fx["page"]))
    os.makedirs(d, exist_ok=True)
    n = fx["id"].rsplit("#", 1)[1]
    path = os.path.join(d, f"{slug(fx['page'])}_{n}.ft")
    open(path, "w", encoding="utf-8").write(fx["src"])
    return path


def write_package(fx):
    d = os.path.join(WORK, slug(fx["page"]), "packages")
    pdir = os.path.join(d, fx["package"])
    shutil.rmtree(pdir, ignore_errors=True)
    os.makedirs(pdir, exist_ok=True)
    open(os.path.join(pdir, "project.yaml"), "w", encoding="utf-8").write(fx["manifest"])
    for name, src in fx["files"].items():
        dest = os.path.join(d, name)
        os.makedirs(os.path.dirname(dest), exist_ok=True)
        open(dest, "w", encoding="utf-8").write(src)
    return pdir


def marked(src):
    """{(line, code)} of the lines that carry `// reject: CODE[, CODE]` after the first line."""
    out = set()
    for i, l in enumerate(src.split("\n"), 1):
        if i == 1:
            continue
        m = re.search(rf"//\s*reject:\s*({CODE}(?:\s*,\s*{CODE})*)", l)
        if m:
            for c in re.findall(CODE, m.group(1)):
                out.add((i, c))
    return out


def has_main(src):
    return re.search(r"^\s*(export\s+)?(async\s+)?(try\s+)?(recovers\s+)?func\s+main\s*\(", src, re.M) is not None


def has_tests(src):
    return re.search(r"^\s*(test|property)\s+\"", src, re.M) is not None


def check_static(path, rel_of):
    """Both checkers' records as {(file, line, code)} plus each checker's records, whole. A checker that does not end
    with status 0 (nothing to report) or 1 (findings), or whose status disagrees with its records, contributes a
    TOOL entry, so a crash, a signal or a timeout can never pass a fixture, even after printing the expected
    record."""
    code, out, err = run([FORTISC, "check", "--format", "json", path])
    prod, pbad = records(out)
    rcode, rout, rerr = run([sys.executable, "-I", FORTIS, "check", "--format", "json", path])
    ref, rbad = records(rout)
    errors = lambda rs: any(r.get("severity") == "error" for r in rs)  # noqa: E731
    # a checker's JSON output is records and nothing else: any other line is a TOOL entry too
    if code != (1 if errors(prod) else 0) or pbad:
        prod.append({"code": "TOOL", "line": 0, "file": "", "message": f"fortisc check exit {code}: {err} {pbad}"})
    if rcode != (1 if errors(ref) else 0) or rbad:
        ref.append({"code": "TOOL", "line": 0, "file": "", "message": f"fortis check exit {rcode}: {rerr} {rbad}"})
    key = lambda r: (rel_of(r.get("file", "")), r.get("line"), r.get("code"))  # noqa: E731
    return prod, ref, {key(r) for r in prod}, {key(r) for r in ref}


def run_backend(backend, target, is_dir, stem):
    """(exit status, standard output, what else was observed) of the program's main on one backend, uncut."""
    if backend == "interpreter":
        code, out, err = run([sys.executable, "-I", FORTIS, "run", target, "--format", "json"])
        return code, out, err
    if backend == "vm":
        code, out, err = run([FORTISC, "vm", target])
        # the observed text runs to the trace line: a printed line feed inside one print stays part of it
        m = re.search(r"^observed: (.*?)\ntrace: ", out, re.M | re.S)
        obs = m.group(1) if m else None
        if obs is None:
            # a refusal prints `observed:` and no trace line. It used to exit 0, and the recorded
            # observation is that text (D-52). A crash that prints no observation is not a run.
            line = next((l[len("observed: "):] for l in out.splitlines() if l.startswith("observed: ")), None)
            if line is not None and line.startswith(("refused", "unbuilt", "dynamic:")):
                return 4, "", "fortisc vm exit 0: " + out + err
            return 4, "", f"fortisc vm exit {code}: " + out + err
        if obs.startswith(("refused", "unbuilt", "dynamic:")):
            # a loud refusal stays a refusal when the process now exits non-zero (D-325)
            return 4, "", obs
        parts = obs.split("|") if obs else []
        status, end, want = 0, "", 0
        if parts and parts[-1].startswith("fail:"):
            status, end, parts, want = 1, parts[-1], parts[:-1], 1
        elif parts and parts[-1].startswith("defect:"):
            # the VM process exits 101 for a defect; the example's status stays 3, as the interpreter's
            status, end, parts, want = 3, parts[-1], parts[:-1], 101
        if code != want:
            return 4, "", f"fortisc vm exit {code}: " + out + err
        return status, "".join(p + "\n" for p in parts), end
    exe = os.path.join(WORK, "bin", stem)
    os.makedirs(os.path.dirname(exe), exist_ok=True)
    # an executable left by an earlier run must not stand in for this build's (a build can exit 0 without one, D-55)
    if os.path.lexists(exe):
        os.remove(exe)
    code, out, err = run([FORTISC, "build", "--format", "json", "--out", exe, target])
    if code != 0 or not os.path.exists(exe):
        return 4, "", "native build failed: " + out + err
    code, out, err = run([exe])
    return code, out, err


def run_tests(backend, target, stem):
    """(ok, detail, observed) of the module's or package's tests on one backend; `observed` adds both streams of
    the run, whole. The stream is read strictly and reconciled:
    every line a record (native: every `record:` line), every test result a pass, and the totals agree (the
    interpreter's summary ends its stream and reports no failure, the VM's counts its results, native's observed
    line counts its records), so a malformed or contradictory stream never passes."""
    if backend == "native":
        code, out, err = run([FORTISC, "native", target, "--tests"])
        obs = [l[len("observed: "):] for l in out.splitlines() if l.startswith("observed: ")]
        tests, bad = records("\n".join(l[len("record: "):] for l in out.splitlines() if l.startswith("record: ")))
        m = re.fullmatch(r"(\d+) passed, (\d+) failed", obs[0]) if len(obs) == 1 else None
        # other records (observations, task lifecycle) may stand beside the test results; only these are counted
        results = [r for r in tests if r.get("kind") == "test-result"]
        failed = [r for r in results if r.get("status") != "pass"]
        ok = (code == 0 and m is not None and not bad and bool(results) and not failed
              and int(m.group(1)) == len(results) and int(m.group(2)) == 0)
        detail = f"exit {code}, observed {obs[:1]}, {len(results)} results, {len(failed)} not passing, malformed {bad[:2]}"
        return ok, detail, f"{detail}; stdout {out}; stderr {err}"
    if backend == "interpreter":
        code, out, err = run([sys.executable, "-I", FORTIS, "test", target, "--format", "json"])
    else:
        code, out, err = run([FORTISC, "test", "--format", "json", target])
    recs, bad = records(out)
    summaries = [r for r in recs if r.get("kind") == "test-summary"]
    # both runners write the summary on stdout and every result on stderr, which is read as strictly (other
    # records, such as observations and reported failures, may stand beside the results)
    rest, ebad = records(err)
    results, bad = [r for r in rest if r.get("kind") == "test-result"], bad + ebad
    failed = [r for r in results if r.get("status") != "pass"]
    s = summaries[0] if len(summaries) == 1 else {}
    ok = (code == 0 and not bad and len(summaries) == 1 and recs[-1] is s and not failed
          and s.get("failed") == 0 and s.get("passed", 0) > 0 and len(results) == s.get("passed"))
    detail = f"exit {code}, summary {json.dumps(s) if s else summaries}, {len(results)} results, {len(failed)} not passing, malformed {bad[:2]}"
    return ok, detail, f"{detail}; stdout {out}; stderr {err}"


def judge(fx):
    """Run one fixture; returns its result row."""
    is_pkg = "package" in fx
    if is_pkg:
        target = fx["target"]                 # written before any fixture is judged (packages depend on each other)
        stem = fx["package"]
        base = os.path.dirname(target)
        rel_of = lambda f: os.path.relpath(os.path.abspath(f), base) if f else ""  # noqa: E731
        srcs = list(fx["files"].values())
    else:
        target = fx["target"]
        stem = os.path.splitext(os.path.basename(target))[0]
        rel_of = lambda f: fx["display"] if f else ""  # noqa: E731
        srcs = [fx["src"]]
    row = {"id": fx["id"], "page": fx["page"], "line": fx["line"], "verdict": fx["verdict"], "source": fx.get("source"), "backends": {}}
    # every check names the tool it judges: `checker` (the two static checkers), a backend, or `docs` (the page)
    checks = []
    if fx["verdict"] == "illustrative":
        # the syntax tool must have judged the block and rejected it with a syntax code; a crash, a timeout or an
        # empty answer is not evidence that the block is not a program
        code, out, err = run([FORTISC, "syntax", target])
        judged = re.search(rf"^{re.escape(target)}\t(E-SYN-\d{{3}})\t\d+:\d+\t", out, re.M)
        ok = code == 0 and judged is not None
        checks.append(("docs", "an illustrative block is judged by the syntax tool and does not parse as a program", ok,
                       f"exit {code}: {head((out + err).strip(), 200)}", f"exit {code}: {(out + err).strip()}"))
        if fx.get("expect"):
            checks.append(("docs", "an illustrative block shows no output", False, fx["expect"][0]))
        return finish(row, checks)
    prod, refrecs, found, ref = check_static(target, rel_of)
    whole = lambda recs: json.dumps(recs, sort_keys=True, ensure_ascii=False)  # noqa: E731
    if fx["verdict"] == "reject":
        want = set()
        if is_pkg:
            for name, b in zip(fx["files"], fx["blocks"]):
                want |= {(name, l, c) for l, c in marked(b["src"])}
        else:
            want = {(fx["display"], l, c) for l, c in marked(fx["src"])}
        declared = set(fx["codes"]) if not is_pkg else {c for b in fx["blocks"] for c in b["codes"]}
        checks.append(("docs", "every declared code is on a marked line", declared == {c for _, _, c in want} and bool(want), f"declared {sorted(declared)}, marked {sorted(want)}"))
        checks.append(("checker", "fortisc check reports exactly the marked lines", found == want, f"reported {sorted(found)}",
                       f"reported {sorted(found)}: {whole(prod)}"))
        checks.append(("checker", "the reference checker reports exactly the marked lines", ref == want, f"reported {sorted(ref)}",
                       f"reported {sorted(ref)}: {whole(refrecs)}"))
        if fx.get("expect") and is_pkg:
            checks.append(("docs", "a rejected package shows no output (only a single module's records are compared)", False, fx["expect"][0]))
        if fx.get("expect") and not is_pkg:
            kind, text = fx["expect"]
            # these runs must end as a rejection does (status 1): a crash after printing the shown text is no match
            if kind == "output":
                code, out, err = run([FORTISC, "check", "--format", "text", target])
                shown = out.replace(target, fx["display"]).rstrip("\n")
                checks.append(("checker", "the shown check output is fortisc's", code == 1 and shown == text.rstrip("\n"), f"exit {code}: {head(shown, 600)} {tail(err, 200)}", f"exit {code}: {shown} {err}"))
            else:
                code, out, err = run([FORTISC, "check", "--format", "json", target])
                got, bad = records(out)
                got = [dict(r, file=fx["display"]) for r in got]
                want, wbad = records(text)
                checks.append(("docs", "the shown records block holds only records, one JSON object a line", not wbad, f"not records: {wbad[:3]}"))
                checks.append(("checker", "the shown records are fortisc's", code == 1 and not bad and got == want, f"exit {code}: {head(json.dumps(got), 600)} {bad[:2]}", f"exit {code}: {json.dumps(got)} {bad}"))
        return finish(row, checks)
    checks.append(("checker", "fortisc check reports nothing", not found, "; ".join(f"{r.get('code')}@{r.get('line')}: {head(r.get('message', ''), 100)}" for r in prod[:3]),
                   whole(prod)))
    checks.append(("checker", "the reference checker reports nothing", not ref, head(str(sorted(ref)), 300), f"{sorted(ref)}: {whole(refrecs)}"))
    main = any(has_main(s) for s in srcs)
    tests = any(has_tests(s) for s in srcs)
    if is_pkg:
        main = main and re.search(r"^entry:", fx["manifest"], re.M) is not None
    if not main and not tests:
        checks.append(("docs", "the example runs (it has main or tests)", False, "a positive example must have `main` or tests, so that it runs"))
    if fx.get("expect") and not main:
        checks.append(("docs", "an output block belongs to a program with main", False, "tests write records, not shown output; remove the block or give the program a main"))
    if main:
        want = None
        if fx.get("expect"):
            kind, text = fx["expect"]
            if kind != "output":
                checks.append(("docs", "a program's shown output is an `output` block", False, kind))
            want = text + "\n" if text else ""
        for b in RUN_BACKENDS:
            status, out, err = run_backend(b, target, is_pkg, stem)
            row["backends"][b] = {"exit": status, "stdout": out[-2000:]}
            checks.append((b, f"exits with status {fx['exit']}", status == fx["exit"], f"exit {status}: {tail(err, 300)}", f"exit {status}: {err}"))
            if want is not None:
                checks.append((b, "prints the shown output", out == want, f"printed {tail(out, 400)!r}", f"printed {rooted(out)!r}"))
            else:
                checks.append((b, "prints nothing (a program that prints shows its output)", out == "", f"printed {tail(out, 200)!r} with no output block", f"printed {rooted(out)!r} with no output block"))
    if tests:
        for b in RUN_BACKENDS:
            ok, detail, observed = run_tests(b, target, stem)
            row["backends"].setdefault(b, {})["tests"] = detail
            checks.append((b, "every test passes", ok, detail, observed))
    return finish(row, checks)


def finish(row, checks):
    """A check is (tool, check, ok, detail) or (tool, check, ok, detail, observed): `detail` is cut to a length to be
    read, `observed` is all of it, kept for a failing check (it is what `details` digests)."""
    row["checks"] = []
    for t, c, ok, d, *whole in checks:
        row["checks"].append({"tool": t, "check": c, "ok": ok, "detail": d})
        if whole and not ok:
            row["checks"][-1]["observed"] = whole[0]
    row["ok"] = all(c["ok"] for c in row["checks"])
    return row


def open_docset_defects():
    """The open release-blocking defects that list documentation fixtures, each as {id, backends, fixtures,
    docset}, in the register's order: from the defect register when it is present, and then also written to
    KNOWN_COPY so that the copy follows the register; otherwise from KNOWN_COPY."""
    path = os.path.join(ROOT, planning.DEFECTS)
    copy_rel = os.path.relpath(KNOWN_COPY, ROOT)
    if not os.path.exists(path):
        if not os.path.exists(KNOWN_COPY):
            planning.skipped("docset", f"matching failures against the open defects (and {copy_rel} is missing too)", planning.DEFECTS)
            return []
        print(f"docset: {planning.DEFECTS} is a maintainers' planning file and is not in this checkout; failures are "
              f"matched against its copy of the open defects, {copy_rel}", file=sys.stderr, flush=True)
        return json.load(open(KNOWN_COPY, encoding="utf-8"))["defects"]
    entries = []
    for d in json.load(open(path, encoding="utf-8"))["defects"]:
        if d.get("release_blocking") and d.get("status") != "fixed":
            rs = d.get("results", {})
            fixtures = [f for f in rs.get("fixtures", []) if f.startswith("docset:")]
            if fixtures:
                entries.append({"id": d["id"], "backends": sorted(set(rs.get("backends", []))), "fixtures": fixtures,
                                "docset": {f[len("docset:"):]: rs.get("docset", {}).get(f[len("docset:"):]) for f in fixtures}})
    copy = json.dumps({"about": "Written by conformance/tools/docset_check.py from the defect register: the open "
                                "release-blocking defects that name documentation examples, with what each records of the "
                                "example's failure. A checkout without the register matches failures against this copy. "
                                "Do not edit it by hand.",
                       "defects": entries}, indent=1, ensure_ascii=False) + "\n"
    if not os.path.exists(KNOWN_COPY) or open(KNOWN_COPY, encoding="utf-8").read() != copy:
        with open(KNOWN_COPY, "w", encoding="utf-8") as out:
            out.write(copy)
    return entries


def known_defects():
    """{fixture id: [(defect, tools, recorded)]} from the open release-blocking defects that list documentation
    fixtures; `recorded` is the defect's `results.docset[id]`: the example's source hash and its failing checks."""
    known = {}
    for d in open_docset_defects():
        for f in d["fixtures"]:
            fid = f[len("docset:"):]
            known.setdefault(fid, []).append((d["id"], set(d["backends"]), d["docset"].get(fid)))
    return known


RECORD_CHECK = "the defect's record matches this failure"


def signature(row):
    """The failing checks of a result, as `tool: check` (the check names are fixed strings)."""
    return sorted({f"{c['tool']}: {c['check']}" for c in row["checks"] if not c["ok"] and c["check"] != RECORD_CHECK})


def _spellings(path):
    """`path` as printed and as a JSON string escapes it (`spellings`), so a non-ASCII temporary directory matches
    both forms. Longest first is the caller's job."""
    esc = json.dumps(path)[1:-1]
    return [path] if esc == path else [path, esc]


def temp_roots():
    """Directories a run may create under, longest first, each in the spellings a tool may print. A checkout that
    itself lives in one is already written `<root>` by `rooted` before this runs, so it is not shortened further.
    A temporary directory under the home directory is also given in the `<home>` form `norm` has already written.
    `/` and an empty value are not roots."""
    found = []
    for p in (os.environ.get("TMPDIR"), tempfile.gettempdir(), "/tmp", "/var/tmp", "/private/tmp"):
        if not p or p in ("/", "~"):
            continue
        for q in (p.rstrip("/"), os.path.realpath(p).rstrip("/")):
            if q and q != "/":
                found.extend(_spellings(q))
    extra = []
    for p in found:
        for h in HOMES:
            if (p == h or p.startswith(h + os.sep)) and len(p) > len(h):
                extra.extend(_spellings("<home>" + p[len(h):]))
    return sorted(set(found + extra), key=len, reverse=True)


# cargo's own status, not a line a program happens to start the same way: `Compiling <crate> vN.N.N (…)`,
# `Finished … target(s) in Ns`, and a lock wait. Anything else, including `Downloading …` and `Finished the job in Ns`,
# is program output and stays.
_CARGO_STATUS = re.compile(
    r"^(?:Compiling \S+ v\d+\.\d+\.\d+\S* \(.*\)|Finished .+ target\(s\) in \d+(?:\.\d+)?s|Blocking waiting for file lock on \S+ directory)$"
)

# a harness record (`record: {"kind": …}`), as fortisc prints each stderr record on its own line. A program's own
# `record:` text is not one of these and is not reordered.
_HARNESS_RECORD = "record: {"

# a temporary-directory path ends at a separator or the end of the text. `/tmp` is not a prefix of `/tmpfile.rs`.
_AFTER_ROOT = set("/\\ \n\t\"'`),;]}")

# directly under that directory: `fortis-dev-<pid>` or mkdtemp's suffix (`[a-z0-9_]`). A printed `fortis-dev-alpha`
# that is not a path stays, so two program lines do not hash as one.
_GENERATED_DIR = re.compile(r"<tmp>/fortis-(?:dev|bs|tel|host-test|querydb|effects-test)-[A-Za-z0-9_]+")


def _replace_root(t, root):
    parts, i = [], 0
    while True:
        j = t.find(root, i)
        if j < 0:
            parts.append(t[i:])
            return "".join(parts)
        end = j + len(root)
        if end == len(t) or t[end] in _AFTER_ROOT:
            parts.append(t[i:j])
            parts.append("<tmp>")
            i = end
        else:
            parts.append(t[i:end])
            i = end


def normalize_observation(t):
    """`t` with nothing left that a second run, a second temporary directory or a second checkout changes, and with
    everything the program prints and the compiler reports kept.

    Removed or rewritten, and only these: a temporary-directory path (its printed and JSON-escaped spellings, and
    only where it is a whole directory), cargo's own status lines, a generated `fortis-*-<pid or mkdtemp suffix>`
    directory under that path, and the order of harness `record: {…}` lines (a parallel harness may interleave
    them). A different compiler error, or a different line the program prints, is still a different text."""
    for p in temp_roots():
        t = _replace_root(t, p)
    def _generated(m):
        name = m.group(0).rsplit("/", 1)[-1]
        return "<tmp>/" + name.rsplit("-", 1)[0] + "-N"
    t = _GENERATED_DIR.sub(_generated, t)
    lines = [l for l in t.split("\n") if not _CARGO_STATUS.match(l.strip())]
    records = sorted(l for l in lines if l.startswith(_HARNESS_RECORD))
    if len(records) > 1:
        it = iter(records)
        lines = [next(it) if l.startswith(_HARNESS_RECORD) else l for l in lines]
    return "\n".join(lines)


def details(row):
    """A digest of what the failing checks observed, all of it and not the detail cut to be read (the output printed,
    the records reported, the build error, the tests that did not pass), with only what differs between runs of the
    same failure removed: this tree's path and the home directory (wherever they are), a temporary directory, cargo's
    own status lines, a generated directory's process id or mkdtemp suffix, a panicking thread's number
    (`thread 'main' (12345) panicked`), addresses, and the order of harness `record: {…}` lines. Everything a
    program prints and everything the compiler reports stays, so a different failure of the same example under the
    same check names does not pass for the recorded one."""
    def norm(t):
        t = rooted(t)
        for p in HOMES:
            t = t.replace(p, "<home>")
        t = re.sub(r"(thread '[^']*' )\(\d+\)", r"\1(N)", t)
        t = re.sub(r"0x[0-9a-fA-F]{6,}", "0xN", t)
        return normalize_observation(t)
    parts = sorted(f"{c['tool']}: {c['check']}: {norm(c.get('observed', c['detail']))}" for c in row["checks"] if not c["ok"] and c["check"] != RECORD_CHECK)
    return hashlib.sha256("\n".join(parts).encode("utf-8")).hexdigest()[:12]


def _selftest():
    """The normalization keeps two runs of one failure identical and two failures distinct. Runs at startup, so a
    normalization that starts erasing a compiler error fails the docset check."""
    same = [
        "error[E0599]: no method named `area` found for reference `&T`\n"
        "   Compiling fx v0.1.0 (/tmp/a/fx)\n   Compiling native v0.1.0 (/tmp/a/native)\n"
        "record: {\"test\": \"b\", \"status\": \"fail\"}\nrecord: {\"test\": \"a\", \"status\": \"pass\"}\n"
        "thread 'main' (111) panicked at 0xabc123\nFinished release [optimized] target(s) in 1.25s\n",
        "error[E0599]: no method named `area` found for reference `&T`\n"
        "   Compiling native v0.1.0 (/var/tmp/b/native)\n   Compiling fx v0.1.0 (/tmp/b/fx)\n"
        "record: {\"test\": \"a\", \"status\": \"pass\"}\nrecord: {\"test\": \"b\", \"status\": \"fail\"}\n"
        "thread 'main' (222) panicked at 0xdef456\nFinished release [optimized] target(s) in 9s\n",
    ]
    def row(obs):
        return {"checks": [{"tool": "native", "check": "every test passes", "ok": False, "detail": "cut", "observed": obs}]}
    # through details(), so dropping the call from norm() fails here and not only on a later run
    if details(row(same[0])) != details(row(same[1])):
        raise SystemExit("docset check: normalization still depends on the run")
    # the same text apart from the compiler error: dropping every error line would make these equal
    if details(row(same[0])) == details(row(same[0].replace("error[E0599]:", "error[E0308]:", 1))):
        raise SystemExit("docset check: normalization erased a different compiler error")
    got = normalize_observation(same[0])
    if "record: {\"test\": \"a\"" not in got.split("record: {\"test\": \"b\"")[0]:
        raise SystemExit("docset check: harness records were not ordered")
    # a program's own lines are not cargo status and not harness records
    if details(row("Downloading alpha\n")) == details(row("Downloading beta\n")):
        raise SystemExit("docset check: normalization erased program output")
    if details(row("Finished the job in 1s\n")) == details(row("Finished the job in 9s\n")):
        raise SystemExit("docset check: normalization erased program output")
    if details(row("record: beta\nrecord: alpha\n")) == details(row("record: alpha\nrecord: beta\n")):
        raise SystemExit("docset check: normalization reordered program output")
    if details(row("pid: 1\n")) == details(row("pid: 2\n")):
        raise SystemExit("docset check: normalization erased program output")
    if details(row("printed 'fortis-dev-alpha'\n")) == details(row("printed 'fortis-dev-beta'\n")):
        raise SystemExit("docset check: normalization erased program output")
    if details(row("error: couldn't read /tmpfile.rs\n")) == details(row("error: couldn't read /var/tmpfile.rs\n")):
        raise SystemExit("docset check: normalization erased a path that is not a temporary directory")
    # mkdtemp's suffix is not a pid; two of them are one directory
    dev = [details(row(s)) for s in ("root /tmp/fortis-dev-k3j9s0ab/out\n", "root /tmp/fortis-dev-91/out\n")]
    if dev[0] != dev[1]:
        raise SystemExit("docset check: a development-root directory still depends on the run")
    old_tmp = os.environ.get("TMPDIR")

    def under(tmpdir, text):
        os.environ["TMPDIR"] = tmpdir
        try:
            return details(row(text))
        finally:
            if old_tmp is None:
                os.environ.pop("TMPDIR", None)
            else:
                os.environ["TMPDIR"] = old_tmp

    # the directory itself, not only a `/tmp` prefix: disabling the rewrite leaves these two texts different
    if under("/tmp/ds2-a", "wrote /tmp/ds2-a/out\n") != under("/var/tmp/ds2-b", "wrote /var/tmp/ds2-b/out\n"):
        raise SystemExit("docset check: the temporary directory itself still depends on the run")
    cafe = "/tmp/caf\u00e9"
    escaped = json.dumps(cafe + "/fortis-dev-k3j9s0ab/out")[1:-1]
    if under(cafe, "wrote " + escaped + "\n") != under("/tmp/else", "wrote /tmp/else/fortis-dev-91/out\n"):
        raise SystemExit("docset check: a JSON-escaped temporary directory still depends on the run")
    # mkdtemp draws `_` as well as letters and digits; both suffixes are one directory
    if under("/tmp", "wrote /tmp/fortis-dev-ab_cd123/out\n") != under("/tmp", "wrote /tmp/fortis-dev-xy_98765/out\n"):
        raise SystemExit("docset check: a development-root directory still depends on the run")


def classify(row, known):
    """PASS, FAIL or KNOWN. KNOWN needs an open release-blocking defect that names this fixture, its tools, the
    example's source hash and exactly its failing checks: a changed example, a new failure or a different failure
    of the same example is a FAIL until the defect records it (and a review agrees)."""
    if row["ok"]:
        return "PASS", None
    entries = known.get(row["id"], [])
    if not entries:
        return "FAIL", None
    tools = set().union(*(t for _, t, _ in entries))
    recorded = next((r for _, _, r in entries if r), None)
    failing = [c for c in row["checks"] if not c["ok"]]
    if not all(c["tool"] in tools for c in failing):
        return "FAIL", "a failing check's tool is not one the defect names"
    if not recorded:
        return "FAIL", f"the defect records no source, failing checks and details for it; record {{\"source\": \"{row.get('source')}\", \"failing\": {signature(row)}, \"details\": \"{details(row)}\"}}"
    if recorded.get("source") != row.get("source"):
        return "FAIL", f"the example changed since the defect recorded it (source {row.get('source')}, recorded {recorded.get('source')})"
    # a run that leaves a backend out (`--backends`) holds the record's checks of the backends it ran; the digest of
    # the details covers every failing check, so it is compared only when the record names no backend left out
    skipped = set(BACKENDS) - set(RUN_BACKENDS)
    want = sorted(s for s in recorded.get("failing", []) if s.split(":", 1)[0] not in skipped)
    if want != signature(row):
        return "FAIL", f"the failing checks differ from the defect's record: now {signature(row)}"
    partial = any(s.split(":", 1)[0] in skipped for s in recorded.get("failing", []))
    if not partial and recorded.get("details") != details(row):
        return "FAIL", f"the failure observed differs from the defect's record (details {details(row)}, recorded {recorded.get('details')})"
    return "KNOWN", ", ".join(d for d, _, _ in entries)


def page_of(fid):
    return fid.split("#")[0].split(":")[0]


def shard_pages(fixtures, spec):
    """The pages of shard I of N: pages in decreasing number of fixtures, each to the shard with the fewest so far
    (ties to the lower shard), so the same pages always land in the same shard."""
    i, n = (int(x) for x in spec.split("/"))
    count = {}
    for fx in fixtures:
        count[page_of(fx["id"])] = count.get(page_of(fx["id"]), 0) + 1
    load, owner = [0] * n, {}
    for page in sorted(count, key=lambda p: (-count[p], p)):
        k = min(range(n), key=lambda s: (load[s], s))
        owner[page] = k + 1
        load[k] += count[page]
    return {p for p, k in owner.items() if k == i}


def main(argv):
    _selftest()
    ap = argparse.ArgumentParser()
    ap.add_argument("--json", action="store_true")
    ap.add_argument("--page", action="append")
    ap.add_argument("-j", type=int, default=max(1, (os.cpu_count() or 2) // 2))
    ap.add_argument("-v", action="store_true")
    ap.add_argument("--release", action="store_true")
    ap.add_argument("--shard", help="I/N: judge the I-th of N groups of pages, balanced by their number of examples (CI runs the groups at once)")
    ap.add_argument("--backends", help="the backends to run, comma-separated (default: all three); the gate's quick tier runs interpreter,vm")
    a = ap.parse_args(argv)
    if a.shard and (a.release or not re.fullmatch(r"[1-9]\d*/[1-9]\d*", a.shard) or int(a.shard.split("/")[0]) > int(a.shard.split("/")[1])):
        print("docset check: --shard takes I/N with 1 <= I <= N, and a release run judges every page in one run")
        return 2
    global RUN_BACKENDS
    if a.backends:
        chosen = tuple(b for b in BACKENDS if b in a.backends.split(","))
        unknown = sorted(set(a.backends.split(",")) - set(BACKENDS))
        if unknown or not chosen:
            print(f"docset check: --backends takes some of {', '.join(BACKENDS)}; not {unknown or a.backends}")
            return 2
        if a.release and chosen != BACKENDS:
            print("docset check: --release judges every backend; it takes no --backends")
            return 2
        RUN_BACKENDS = chosen
        if chosen != BACKENDS:
            print(f"docset check: backends {', '.join(chosen)} only; {', '.join(b for b in BACKENDS if b not in chosen)} not run "
                  f"(a known failure is matched on its source and the checks of these backends)", flush=True)
    if not os.path.exists(FORTISC):
        print("docset check: build the production tools first (cargo build --release in compiler/)")
        return 3
    pages = doc_pages()
    if a.page:
        # a release verdict covers the whole documentation set: a filtered run cannot give one
        if a.release:
            print("docset check: --release judges every page; it takes no --page")
            return 2
        unknown = [p for p in a.page if p not in pages]
        if unknown:
            print(f"docset check: not pages of the documentation set: {unknown}")
            return 2
        pages = [p for p in pages if p in a.page]
    fixtures, problems = collect(pages)
    if not fixtures and not problems:
        print("docset check: FAILED (no fixtures: nothing was judged)")
        return 1
    names = {}
    for fx in fixtures:
        if "package" in fx:
            names.setdefault(fx["package"], []).append(fx["id"])
    for pkg, ids in names.items():
        if len(ids) > 1:
            problems.append({"id": ids[0], "line": 0, "detail": f"package name {pkg} is used on more than one page ({ids}); each example package needs its own name"})
    # the native runtime is built once before the parallel builds share it
    os.makedirs(os.path.join(WORK, "bin"), exist_ok=True)
    warm = os.path.join(WORK, "warm.ft")
    open(warm, "w").write("// accept\nexport func main(args: List[String]) Void {\n}\n")
    if "native" in RUN_BACKENDS:
        run([FORTISC, "build", "--format", "json", "--out", os.path.join(WORK, "bin", "warm"), warm])
    # every destination is distinct (two fixtures writing one file would judge one source twice under two ids)
    seen = {}
    for fx in fixtures:
        for dest in destinations(fx):
            if dest in seen:
                print(f"docset check: FAILED: {fx['id']} and {seen[dest]} would both be written to {os.path.relpath(dest, ROOT)}")
                return 1
            seen[dest] = fx["id"]
    # every file is written before any fixture is judged: a page's packages depend on one another
    for fx in fixtures:
        fx["target"] = write_package(fx) if "package" in fx else write_single(fx)
    if a.shard:
        mine = shard_pages(fixtures, a.shard)
        fixtures = [fx for fx in fixtures if page_of(fx["id"]) in mine]
        problems = [p for p in problems if page_of(p["id"]) in mine]
        print(f"docset check: shard {a.shard}: {len(mine)} pages, {len(fixtures)} fixtures", flush=True)
    with concurrent.futures.ThreadPoolExecutor(max_workers=a.j) as pool:
        rows = list(pool.map(judge, fixtures))
    known = known_defects()
    for r in rows:
        r["status"], why = classify(r, known)
        if not r["ok"]:
            # what a defect records to make this result KNOWN (the defect register's results.docset)
            r["record"] = {"source": r.get("source"), "failing": signature(r), "details": details(r)}
        if r["status"] == "KNOWN":
            r["defect"] = why
        elif why:
            r["checks"].append({"tool": "docs", "check": RECORD_CHECK, "ok": False, "detail": why})
    for p in problems:
        rows.append({"id": p["id"], "page": p["id"].split("#")[0].split(":")[0], "line": p["line"], "verdict": "malformed", "status": "FAIL", "ok": False,
                     "checks": [{"tool": "docs", "check": "well-formed block", "ok": False, "detail": p["detail"]}]})
    for r in sorted(rows, key=lambda r: (r["page"], r["line"])):
        if r["status"] != "PASS" or a.v:
            print(f"{r['status']:<5} {r['id']} (line {r['line']}, {r['verdict']})" + (f" [{r['defect']}]" if r.get("defect") else ""))
            for c in r["checks"]:
                if not c["ok"] or a.v:
                    print(f"      {'ok ' if c['ok'] else 'NO '} {c['tool']}: {c['check']}: {c['detail'][:400]}")
    by_status = {}
    for r in rows:
        by_status[r["status"]] = by_status.get(r["status"], 0) + 1
    by_verdict = {}
    for r in rows:
        by_verdict[r["verdict"]] = by_verdict.get(r["verdict"], 0) + 1
    print(f"\n{len(pages)} pages, {len(rows)} fixtures (" + ", ".join(f"{k} {v}" for k, v in sorted(by_verdict.items())) + "): "
          + ", ".join(f"{k} {v}" for k, v in sorted(by_status.items())))
    # the summary of DEL-49 criterion 2, which the release check (DEL-44, DW-11) compares with its own count of the
    # pages: `examples` and `negatives` count fortis blocks (a package's modules one by one), `per_backend` the
    # positive blocks whose example ran clean on that backend (checked, ran, printed what is shown, tests passed),
    # and `failed` every result that is not a pass, KNOWN ones included
    if not a.page and not a.shard:
        nblocks = {fx["id"]: (len(fx["blocks"]) if "package" in fx else 1) for fx in fixtures}
        examples = sum(nblocks.get(r["id"], 1) for r in rows if r["verdict"] in ("accept", "reject"))
        negatives = sum(nblocks.get(r["id"], 1) for r in rows if r["verdict"] == "reject")
        n, m = release_counts()
        if (n, m) != (examples, negatives):
            # a block the release counts but this run did not judge (a page or a file type this reader skips)
            print(f"FAIL  the release counts {n} examples and {m} negatives in the pages; this run judged {examples} and {negatives}")
            by_status["FAIL"] = by_status.get("FAIL", 0) + 1
        per_backend = {}
        for b in BACKENDS:
            clean = lambda r: (any(c["tool"] == b for c in r["checks"])  # noqa: E731
                               and all(c["ok"] for c in r["checks"] if c["tool"] in (b, "checker", "docs")))
            per_backend[b] = sum(nblocks.get(r["id"], 1) for r in rows if r["verdict"] == "accept" and clean(r))
        print(json.dumps({"kind": "docs-check", "criterion": 2, "examples": n, "negatives": m, "per_backend": per_backend,
                          "failed": sum(1 for r in rows if r["status"] != "PASS") + (1 if (n, m) != (examples, negatives) else 0)}))
    if a.json:
        os.makedirs(os.path.join(ROOT, "build"), exist_ok=True)
        json.dump({"rule": "PROG-TST-08", "chunk": "DEL-49", "pages": pages, "fixtures": sorted(rows, key=lambda r: (r["page"], r["line"]))},
                  open(os.path.join(ROOT, "build", "docset.json"), "w", encoding="utf-8"), indent=1)
    # `--release` (DEL-49 criterion 2, the release check): a KNOWN result is not a pass, so it fails too
    fails = by_status.get("FAIL", 0) + (by_status.get("KNOWN", 0) if a.release else 0)
    print("docset check: " + ("OK" if not fails else "FAILED") + (f" ({by_status['KNOWN']} known, release-blocking)" if by_status.get("KNOWN") else ""))
    return 1 if fails else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
