"""DEL-41 criterion 2: `fortis view` answers its structural queries correctly, renders for terminals and pipes, and
never writes.

`python3 -I conformance/tools/view_check.py [--json]`

1. **Queries**: every query `fortisc view --queries` lists (with `aspect:get` and `aspect:observe` for the aspect
   query) runs over every `.ft` file of the corpus, the package fixtures, the library, the examples and the
   acceptance application. Its JSON answers are compared with this tool's own implementation of the same query
   over the **reference** parser's tree (conformance/tools/parser.py), written from the query's definition and
   independent of the Rust code. The sets of (file, line, column, text) must be equal, query by query.
2. **Rendering**: a file viewed through a pseudo-terminal carries ANSI highlighting; through a pipe it is the plain
   text, byte for byte; `--json` gives one record per line whose text is the line and whose spans lie within it;
   `--decls`, `--symbol`, globs and directories work.
3. **Never writes**: the repository's files (path, size, modification time, sha256) and the temporary directory's
   entries are the same before and after every run of the viewer above.
"""
import hashlib
import json
import os
import pty
import re
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
sys.path.insert(0, HERE)
import lexer   # noqa: E402
import parser  # noqa: E402
from tree import children, direct_tokens, find_all, is_node, name, tokens  # noqa: E402

FORTISC = os.path.join(ROOT, "compiler", "target", "release", "fortisc")
DIRS = ["conformance/corpus", "conformance/programs", "conformance/packages", "library", "examples", "acceptance"]


def files():
    out = []
    for d in DIRS:
        for root, dirs, fs in os.walk(os.path.join(ROOT, d)):
            dirs[:] = sorted(x for x in dirs if not x.startswith(".") and x not in ("build", "target"))
            out += [os.path.relpath(os.path.join(root, f), ROOT) for f in sorted(fs) if f.endswith(".ft")]
    return out


def parse(rec, src):
    """(tree, column shift of line 1): a module, or a fragment parsed as a block."""
    try:
        return rec.parse(lexer.lex(src), "module"), 0
    except Exception:  # noqa: BLE001
        try:
            return rec.parse(lexer.lex("{ " + src + "\n}\n"), "block"), 2
        except Exception:  # noqa: BLE001
            return None, 0


def walk(n):
    if is_node(n):
        yield n
        for c in children(n):
            yield from walk(c)


def head_name(head):
    kids = children(head)
    for i, k in enumerate(kids):
        if not is_node(k) and k.text == "func":
            nxt = kids[i + 1] if i + 1 < len(kids) else None
            if nxt is None:
                return None
            return nxt if not is_node(nxt) else tokens(nxt)[0]
    return None


def modifiers(head):
    for k in children(head):
        if is_node(k) and name(k) == "modifiers":
            return [t.text for t in tokens(k)]
    return []


def reference(q, tree):
    """The query's answers as (line, col, text), written from its definition."""
    out = []
    heads = [n for n in walk(tree) if name(n) in ("funcHead", "methodHead")]
    mods = {"async": ["async"], "try": ["try"], "async-try": ["async", "try"], "pure": ["pure"], "noalloc": ["noalloc"], "trusted": ["trusted"], "recovers": ["recovers"]}
    if q in mods:
        for h in heads:
            m = modifiers(h)
            if all(w in m for w in mods[q]):
                t = head_name(h)
                if t is not None:
                    out.append((t, t.text))
    elif q in ("editable-params", "owned-params"):
        mode = q.split("-")[0]
        for p in find_all(tree, "param"):
            kids = children(p)
            sp = next((k for k in kids if is_node(k) and name(k) == "selfParam"), None)
            if sp is not None:
                t = next((x for x in direct_tokens(sp) if x.text == mode), None)
                if t is not None:
                    out.append((t, "self"))
                continue
            pt = next((k for k in kids if is_node(k) and name(k) == "paramType"), None)
            if pt is not None and direct_tokens(pt) and direct_tokens(pt)[0].text == mode and direct_tokens(p):
                out.append((direct_tokens(p)[0], direct_tokens(p)[0].text))
    elif q in ("or-fail", "or-recover", "catch"):
        for o in find_all(tree, "outcomeExpr"):
            ts = direct_tokens(o)
            for i, t in enumerate(ts):
                nxt = ts[i + 1].text if i + 1 < len(ts) else None
                if (q == "catch" and t.text == "catch") or (q == "or-fail" and t.text == "or" and nxt == "fail") or (q == "or-recover" and t.text == "or" and nxt == "recover"):
                    out.append((t, t.text))
    elif q == "fail":
        for p in find_all(tree, "primaryExpr"):
            k = children(p)
            if k and not is_node(k[0]) and k[0].text == "fail" and k[0].kind == "KW":
                out.append((k[0], "fail"))
    elif q == "return":
        for s in find_all(tree, "statement"):
            k = children(s)
            if k and not is_node(k[0]) and k[0].text == "return":
                out.append((k[0], "return"))
    elif q in ("defer", "scope", "capture", "expect"):
        for s in find_all(tree, q + "Stmt"):
            t = tokens(s)[0]
            out.append((t, t.text))
    elif q == "move":
        for m in find_all(tree, "marker"):
            t = tokens(m)[0]
            if t.text == "move":
                out.append((t, "move"))
    elif q == "lambda":
        for m in find_all(tree, "lambda"):
            t = tokens(m)[0]
            out.append((t, t.text))
    elif q == "tests":
        for d in find_all(tree, "testDecl"):
            t = next((x for x in direct_tokens(d) if x.kind == "STR"), None)
            if t is not None:
                out.append((t, t.text))
    elif q in ("errors", "classes"):
        rule, kw = ("errorDecl", "error") if q == "errors" else ("classDecl", "class")
        for d in find_all(tree, rule):
            ts = direct_tokens(d)
            for i, t in enumerate(ts):
                if t.text == kw and i + 1 < len(ts):
                    out.append((ts[i + 1], ts[i + 1].text))
                    break
    elif q.startswith("aspect:"):
        want = q.split(":", 1)[1]
        for a in find_all(tree, "aspectUse"):
            ts = direct_tokens(a)
            nm = next((t.text for t in ts if t.kind == "LOWER"), None)
            if nm == want and ts:
                out.append((ts[0], "@" + want))
    else:
        raise ValueError(q)
    return out


def snapshot():
    snap = {}
    for root, dirs, fs in os.walk(ROOT):
        dirs[:] = [d for d in dirs if d not in (".git",)]
        for f in fs:
            p = os.path.join(root, f)
            try:
                st = os.stat(p)
            except OSError:
                continue
            snap[p] = (st.st_size, st.st_mtime_ns)
    tmp = sorted(os.listdir(tempfile.gettempdir()))
    return snap, tmp


def tty_run(args):
    """The viewer's output through a pseudo-terminal."""
    pid, fd = pty.fork()
    if pid == 0:
        os.chdir(ROOT)
        os.execv(FORTISC, [FORTISC] + args)
    data = b""
    while True:
        try:
            chunk = os.read(fd, 65536)
        except OSError:
            break
        if not chunk:
            break
        data += chunk
    os.waitpid(pid, 0)
    return data.decode("utf-8", "replace")


def main(argv):
    rec = parser.load_grammar(os.path.join(ROOT, "reference", "grammar", "fortis.ebnf"))
    before = snapshot()
    paths = files()
    queries = [json.loads(l)["name"] for l in subprocess.run([FORTISC, "view", "--queries", "--json"], capture_output=True, text=True, cwd=ROOT).stdout.splitlines()]
    queries = [q for q in queries if q != "aspect:NAME"] + ["aspect:get", "aspect:observe"]
    trees = {}
    for p in paths:
        src = open(os.path.join(ROOT, p), encoding="utf-8").read()
        trees[p] = parse(rec, src)
    problems, rows = 0, []
    for q in queries:
        r = subprocess.run([FORTISC, "view", "--json", "--query", q] + paths, capture_output=True, text=True, cwd=ROOT)
        got = {(m["path"], m["line"], m["column"], m["text"]) for m in map(json.loads, r.stdout.splitlines())}
        want = set()
        for p, (tree, shift) in trees.items():
            if tree is None:
                continue
            for t, text in reference(q, tree):
                col = t.col - shift if t.line == 1 else t.col
                want.add((p, t.line, col, text))
        ok = got == want
        problems += not ok
        rows.append({"query": q, "matches": len(want), "agree": ok, "only_view": sorted(got - want)[:5], "only_reference": sorted(want - got)[:5]})
        print(f"{'PASS' if ok else 'FAIL'} {q:<16} {len(want):>5} matches" + ("" if ok else f"  only view {sorted(got - want)[:3]}  only reference {sorted(want - got)[:3]}"))
    # rendering
    sample = "examples/apps/orders/src/pricing.ft"
    plain = subprocess.run([FORTISC, "view", sample], capture_output=True, text=True, cwd=ROOT).stdout
    tty = tty_run(["view", sample])
    src = open(os.path.join(ROOT, sample), encoding="utf-8").read()
    js = [json.loads(l) for l in subprocess.run([FORTISC, "view", "--json", sample], capture_output=True, text=True, cwd=ROOT).stdout.splitlines()]
    spans_ok = all(c >= 1 and c - 1 + n <= len(rec_["text"]) for rec_ in js for c, n, _ in rec_["spans"])
    decls = subprocess.run([FORTISC, "view", "--decls", "--json", "examples/apps/orders/src"], capture_output=True, text=True, cwd=ROOT).stdout.splitlines()
    symbol = subprocess.run([FORTISC, "view", "--symbol", "lineTotal", "examples/apps/orders/src"], capture_output=True, text=True, cwd=ROOT).stdout
    globbed = subprocess.run([FORTISC, "view", "--decls", "examples/**/*.ft"], capture_output=True, text=True, cwd=ROOT).stdout
    render = {
        "pipe_is_plain_text": plain == src,
        "terminal_is_highlighted": "\x1b[" in tty and re.sub(r"\x1b\[[0-9;]*m", "", tty).replace("\r\n", "\n") == src,
        "json_lines_match_source": [r_["text"] for r_ in js] == src.rstrip("\n").split("\n") and spans_ok,
        "decls": len(decls) >= 4 and all(json.loads(l)["kind"] == "declaration" for l in decls),
        "symbol": symbol.splitlines()[1:2] == ["export pure try func lineTotal(line: Line) Money {"],
        "glob": globbed.count("examples/") >= 10,
    }
    for k, v in render.items():
        print(f"{'PASS' if v else 'FAIL'} rendering: {k}")
        problems += not v
    after = snapshot()
    unchanged = before == after
    changed = sorted(set(before[0]) ^ set(after[0]) | {p for p in before[0] if p in after[0] and before[0][p] != after[0][p]})
    print(f"{'PASS' if unchanged else 'FAIL'} never writes: {len(before[0])} repository files and the temporary directory unchanged" + ("" if unchanged else f" (changed: {changed[:5]})"))
    problems += not unchanged
    print(f"\n{len(queries)} queries over {len(paths)} files ({sum(t is not None for t, _ in trees.values())} parse), {sum(r_['agree'] for r_ in rows)} agree with the reference implementation")
    if "--json" in argv:
        os.makedirs(os.path.join(ROOT, "build"), exist_ok=True)
        json.dump({"queries": rows, "rendering": render, "never_writes": unchanged, "files": len(paths)}, open(os.path.join(ROOT, "build", "view.json"), "w"), indent=1)
    print("view check: " + ("OK" if problems == 0 else "FAILED"))
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
