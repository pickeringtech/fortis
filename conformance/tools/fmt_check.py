"""DEL-41 criterion 1 and DEL-43 criterion 3: `fortis fmt` is idempotent, preserves the parse tree on every
fixture, and prints back every inferred fact (Programme 9.3, PROG-AGT-06, X-130).

`python3 -I conformance/tools/fmt_check.py [--json]`

For every `.ft` file of the corpus, the package fixtures, the library, the examples, the acceptance application and
the benchmarks, `fortisc fmt` formats it once and then formats the result again. Three things must hold:

1. the second formatting changes nothing: `fmt(fmt(x)) == fmt(x)`;
2. the formatted text has the same parse tree as the original under the **reference** parser
   (conformance/tools/parser.py, independent of the formatter's own check), comparing rule names and token kinds and
   texts, without positions or the text of newline tokens; a statement fragment is parsed as a block, as the
   conformance harness parses one. The permitted differences are a reprinted fact: a `pure` or `noalloc` added to
   a modifier list, in canonical order (the formatter itself checks that the module then checks as before), and an
   old form v0.0.2 rewrites (X-345): a `Void` return type after `)`, an `editable` before a receiver or a callee
   (a place followed by `(`), and the `move` of `return move` or `return (move` (the formatter checks that the
   module then checks with the same records, and the program's tokens must be the original's with only those
   removed);
3. layout does not matter: the file with every line re-indented at random (lines inside a multi-line token
   untouched), trailing spaces and extra blank lines added, formats to the same text as the file itself;
4. the formatter never reports a defect;
6. **the v0.0.2 reading migrations** (X-345): every `.ft` of `conformance/fmt/migrate` formats, migrations included,
   to its `.formatted` file, idempotently, and an accepted module stays accepted and does what it did: its tests give
   the same results, or its `main` the same output and exit status, before and after, on the interpreter and on the
   VM (D-324, D-329). The migrations rewrite the entry
   `main`, a test's `or fail` and a map lookup's copy (fortis-fmt `migrate`), and their result is not the same tree, so
   criteria 1 to 3 and 5 run with `--no-migrate`, which leaves those three alone (the old-form deletions of criterion
   2 still apply): the corpus keeps its v0.0.1 forms until the migration stream rewrites them;
5. **inferred-fact coverage** (Programme 9.3): every fact 9.3 requires in formatted source is either printed back by
   the formatter or required by the checker. Printed back: a contract's single-word modifiers after its name (X-130);
   a lambda's inferred `pure` and `noalloc` are not (LR 8.2, X-345), which `lambdas.ft` pins; the golden corpus
   `conformance/fmt/facts` must format to its `.formatted` files, idempotently, and an accepted module must stay
   accepted (a rejected one is laid out only, apart from its `Void` return types, which are syntax: X-345).
   Required: a closure kind weaker than the body needs (`E-FUN-010`), a call-site marker left out (`E-OWN-001`)
   and a loan result without its `from` region (`E-OWN-053`) are rejected, so no accepted module lacks them.

6. **rewrites into the reading forms** (X-345): `fortis fmt` rewrites an accepted module into the v0.0.2 forms where
   they mean the same (a pun, an or-pattern arm, a checked operator, a bare variant; fortis-types `migrate`) and says
   so on standard error. Such a file's parse tree changes by design: its layout alone (`fmt --layout`) must keep the
   tree, the rewritten module must check (the formatter refuses otherwise), and the golden corpus
   `conformance/fmt/rewrites` must format to its `.formatted` files, idempotently, accepted.

A file the formatter refuses because it does not parse is listed with its header; each must be a fixture whose
verdict expects a syntax error, or an illustrative fragment.
"""
import difflib
import json
import os
import random
import re
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
sys.path.insert(0, HERE)
import lexer   # noqa: E402
import parser  # noqa: E402
from tree import children, is_node, name  # noqa: E402

FORTISC = os.path.join(ROOT, "compiler", "target", "release", "fortisc")
DIRS = ["conformance/corpus", "conformance/programs", "conformance/packages", "library", "reference/library", "examples", "acceptance", "benchmarks"]


def files():
    out = []
    for d in DIRS:
        for root, dirs, fs in os.walk(os.path.join(ROOT, d)):
            dirs[:] = sorted(x for x in dirs if not x.startswith(".") and x not in ("build", "target"))
            out += [os.path.relpath(os.path.join(root, f), ROOT) for f in sorted(fs) if f.endswith(".ft")]
    return out


REPRINTABLE = ("pure", "noalloc")
ORDER = ["trusted", "pure", "noalloc", "async", "try", "recovers"]


def shape(node, mods) -> str:
    """The tree without positions; each modifier list's words go to `mods` instead, in order."""
    if not is_node(node):
        if node.kind in ("NL", "NEWLINE"):
            return node.kind
        return f"{node.kind}={node.text}"
    if name(node) == "modifiers":
        words = [t.text for t in children(node) if not is_node(t)]
        mods.append(words)
        return "(modifiers " + " ".join(w for w in words if w not in ORDER) + ")"
    return "(" + name(node) + " " + " ".join(shape(c, mods) for c in children(node)) + ")"


def tree_of(rec, src):
    mods = []
    try:
        return shape(rec.parse(lexer.lex(src), "module"), mods), mods
    except Exception:  # noqa: BLE001
        mods = []
        return shape(rec.parse(lexer.lex("{ " + src + "\n}\n"), "block"), mods), mods


def same_program(a, b):
    """(equal, facts): the trees equal except for reprinted facts; `facts` counts the words added."""
    (sa, ma), (sb, mb) = a, b
    if sa != sb or len(ma) != len(mb):
        return False, 0
    added = 0
    for x, y in zip(ma, mb):
        extra = [w for w in y if w not in x]
        if [w for w in y if w in x] != x or any(w not in REPRINTABLE for w in extra):
            return False, 0
        ranks = [ORDER.index(w) for w in y if w in ORDER]
        if ranks != sorted(ranks):
            return False, 0
        added += len(extra)
    return True, added


def returned_in_parens(toks, i):
    """Whether the token at `i` follows `return` and one or more `(` only: the `move` of `return (move x)`."""
    j = i - 1
    while j >= 0 and toks[j].text == "(":
        j -= 1
    return j < i - 1 and j >= 0 and toks[j].text == "return"


def old_forms_removed(src, once):
    """The old forms `fortis fmt` dropped (X-345), when the only differences between the two texts' tokens are those
    and reprinted facts; None when anything else differs. A dropped token must be a `Void` right after `)`, a `move`
    right after `return` (or `return (`), or an `editable` before a place that a `(` follows (a receiver or a
    callee)."""
    def toks(text):
        try:
            return [t for t in lexer.lex(text) if t.kind not in ("NL", "NEWLINE", "EOF")]
        except Exception:  # noqa: BLE001
            return None
    a, b = toks(src), toks(once)
    if a is None or b is None:
        return None
    sm = difflib.SequenceMatcher(a=[(t.kind, t.text) for t in a], b=[(t.kind, t.text) for t in b], autojunk=False)
    removed = []
    for op, i1, i2, j1, j2 in sm.get_opcodes():
        if op == "equal":
            continue
        if op == "insert" and all(t.text in REPRINTABLE for t in b[j1:j2]):
            continue
        if op != "delete":
            return None
        for i in range(i1, i2):
            t, prev = a[i], (a[i - 1] if i > 0 else None)
            if t.text == "Void" and prev is not None and prev.text == ")":
                pass
            elif t.text == "move" and prev is not None and (prev.text == "return" or returned_in_parens(a, i)):
                pass                # `return move x`, and `return (move x)`, which is `return (x)` (D-313)
            elif t.text == "editable":
                j = i + 1
                if j >= len(a) or not (a[j].kind == "LOWER" or a[j].text == "self"):
                    return None
                j += 1
                while j + 1 < len(a) and a[j].text == "." and a[j + 1].kind in ("LOWER", "KW"):
                    j += 2
                if j >= len(a) or a[j].text != "(":
                    return None
            else:
                return None
            removed.append(t.text)
    return removed


def scramble(src: str, rnd: random.Random) -> str:
    """Random indentation, trailing spaces and doubled blank lines; a line inside a multi-line token is kept."""
    inner, opens = set(), set()
    for t in lexer.lex(src):
        n = t.text.count("\n")
        if n and t.kind not in ("NL", "NEWLINE"):
            inner.update(range(t.line + 1, t.line + n + 1))
            opens.add(t.line)             # trailing spaces on this line would belong to the token
    out = []
    for i, line in enumerate(src.split("\n"), start=1):
        if i in inner:
            out.append(line)
            continue
        body = line.strip()
        if not body:
            out.append(" " * rnd.randint(0, 3))
            if rnd.random() < 0.3:
                out.append("")
            continue
        out.append(" " * rnd.randint(0, 9) + body + ("" if i in opens else " " * rnd.randint(0, 2)))
    return "\n".join(out)


def fmt(text, migrate=False):
    with tempfile.NamedTemporaryFile("w", suffix=".ft", delete=False, encoding="utf-8") as f:
        f.write(text)
        path = f.name
    try:
        r = subprocess.run([FORTISC, "fmt"] + ([] if migrate else ["--no-migrate"]) + [path], capture_output=True, text=True, timeout=120)
        return r.returncode, r.stdout, r.stderr
    finally:
        os.unlink(path)


MIGRATE = os.path.join(ROOT, "conformance", "fmt", "migrate")
FORTIS = os.path.join(ROOT, "bin", "fortis")


def behaviour(path):
    """What a module does, to compare a migration's two versions: its tests' results (each test and case with its
    status, actual and expected), or its `main`'s output and exit status, on the interpreter and on the VM; None
    for a module with neither. Evidence lines are left out: a migration may remove lines above a test."""
    src = open(path, encoding="utf-8").read()
    if re.search(r"^(test|property) ", src, re.M):
        def results(lines):
            out = []
            for line in lines:
                if line.strip().startswith("{"):
                    r = json.loads(line)
                    if r.get("kind") == "test-result":
                        ev = r.get("evidence") if isinstance(r.get("evidence"), dict) else {}
                        out.append((r["test"], r["case"], r["status"], str(ev.get("actual", "")), str(ev.get("expected", ""))))
            return out
        i = subprocess.run([sys.executable, "-I", FORTIS, "test", "--format", "json", path], capture_output=True, text=True, cwd=ROOT, timeout=300)
        v = subprocess.run([FORTISC, "test", "--format", "json", path], capture_output=True, text=True, cwd=ROOT, timeout=300)
        return {"interpreter": (i.returncode, results(i.stderr.splitlines())), "vm": (v.returncode, results(v.stderr.splitlines()))}
    if re.search(r"^(export\s+)?((try|recovers)\s+)*func\s+main\s*\(", src, re.M):
        i = subprocess.run([sys.executable, "-I", FORTIS, "run", path], capture_output=True, text=True, cwd=ROOT, timeout=300)
        v = subprocess.run([FORTISC, "vm", path], capture_output=True, text=True, cwd=ROOT, timeout=300)
        observed = "\n".join(l for l in v.stdout.splitlines() if l.startswith("observed: "))
        # a reading of the system clock differs from run to run, so it is compared as a reading
        clock = lambda s: re.sub(r"\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(\.\d+)?Z", "<now>", s)  # noqa: E731
        return {"interpreter": (i.returncode, clock(i.stdout)), "vm": (v.returncode, clock(observed))}
    return None


def migrations():
    """Rows for criterion 6: each migration golden formats to its `.formatted` text, idempotently, checks as before
    and does what it did (`behaviour`)."""
    rows = []
    for f in sorted(x for x in os.listdir(MIGRATE) if x.endswith(".ft")):
        path = os.path.join(MIGRATE, f)
        src = open(path, encoding="utf-8").read()
        want = open(path[:-3] + ".formatted", encoding="utf-8").read()
        code, once, err = fmt(src, migrate=True)
        code2, twice, _ = fmt(once, migrate=True)
        before = subprocess.run([FORTISC, "check", path], capture_output=True, text=True).stdout.strip() == ""
        # the migrated text beside a copy of the original, under the original's name, so that the two runs differ
        # only in the text
        d = tempfile.mkdtemp(prefix="fortis-migrate-")
        try:
            moved = os.path.join(d, f)
            with open(moved, "w", encoding="utf-8") as t:
                t.write(once)
            after = subprocess.run([FORTISC, "check", moved], capture_output=True, text=True).stdout.strip() == ""
            same = True
            if before and after:
                was, now = behaviour(path), behaviour(moved)
                same = was == now
        finally:
            shutil.rmtree(d, ignore_errors=True)
        ok = code == 0 and once == want and twice == once and (after if before else True) and same
        detail = err[:200] if code else ("" if once == want else "differs from the .formatted file")
        if not same:
            detail = f"runs differently after the migration: before {was}, after {now}"[:400]
        rows.append({"migration": f, "ok": ok, "accepted": before, "ran": was is not None if before and after else False, "detail": detail})
    return rows


FACTS = os.path.join(ROOT, "conformance", "fmt", "facts")
REWRITES = os.path.join(ROOT, "conformance", "fmt", "rewrites")
# the 9.3 facts the checker requires: a fixture leaving each out, and its code
REQUIRED = [
    ("closure kind", "conformance/programs/fun_kind_too_weak.ft", "E-FUN-010"),
    ("parameter-mode marker at a call site", "conformance/programs/own_adv_from_without_marker.ft", "E-OWN-001"),
    ("`from` region", "conformance/programs/own_from_no_origin.ft", "E-OWN-053"),
]


def facts_coverage():
    """Rows for criterion 5."""
    rows = []
    for d, f in [(d, x) for d in (FACTS, REWRITES) for x in sorted(os.listdir(d)) if x.endswith(".ft")]:
        path = os.path.join(d, f)
        src = open(path, encoding="utf-8").read()
        want = open(path[:-3] + ".formatted", encoding="utf-8").read()
        code, once, err = fmt(src)
        code2, twice, _ = fmt(once)
        checked = subprocess.run([FORTISC, "check", path], capture_output=True, text=True).stdout
        with tempfile.NamedTemporaryFile("w", suffix=".ft", delete=False, encoding="utf-8") as t:
            t.write(once)
        after = subprocess.run([FORTISC, "check", t.name], capture_output=True, text=True).stdout
        os.unlink(t.name)
        accepted = checked.strip() == ""
        layout_only = once == fmt_layout(src) or set(old_forms_removed(src, once) or ["?"]) == {"Void"}   # X-345: Void is syntax
        ok = code == 0 and once == want and twice == once and (after.strip() == "" if accepted else layout_only)
        rows.append({"fact": f"{'rewritten' if d == REWRITES else 'reprinted'} ({f})", "ok": ok, "accepted": accepted, "detail": err[:200] if code else ""})
    for what, fixture, code_want in REQUIRED:
        out = subprocess.run([FORTISC, "check", "--format", "text", os.path.join(ROOT, fixture)], capture_output=True, text=True).stdout
        rows.append({"fact": f"required: {what}", "ok": f" {code_want} " in out, "fixture": fixture, "code": code_want})
    return rows


def fmt_layout(text):
    with tempfile.NamedTemporaryFile("w", suffix=".ft", delete=False, encoding="utf-8") as f:
        f.write(text)
        path = f.name
    try:
        return subprocess.run([FORTISC, "fmt", "--layout", path], capture_output=True, text=True, timeout=120).stdout
    finally:
        os.unlink(path)


def main(argv):
    rec = parser.load_grammar(os.path.join(ROOT, "reference", "grammar", "fortis.ebnf"))
    rows, problems = [], 0
    for p in files():
        src = open(os.path.join(ROOT, p), encoding="utf-8").read()
        code, once, err = fmt(src)
        row = {"path": p}
        if code == 4 or "defect" in err:
            row.update(status="defect", detail=err.strip()[:300])
            problems += 1
        elif code != 0:
            head = src.splitlines()[0] if src.strip() else ""
            syntax = re.search(r"E-SYN-\d+", head) is not None or head.startswith(("// illustrative", "// base:"))
            row.update(status="not formatted", detail=head[:120], expected=syntax)
            problems += not syntax
        else:
            code2, twice, err2 = fmt(once)
            idem = code2 == 0 and twice == once
            code3, again, err3 = fmt(scramble(src, random.Random(p)))
            row["layout_free"] = code3 == 0 and again == once
            rewritten = "into the reading forms" in err
            try:
                if rewritten:
                    # rewritten into the reading forms (X-345): the layout alone keeps the tree, and the formatter has
                    # checked the rewritten module
                    same, _ = same_program(tree_of(rec, src), tree_of(rec, fmt_layout(src)))
                    facts = 0
                else:
                    same, facts = same_program(tree_of(rec, src), tree_of(rec, once))
                if not same and not rewritten:
                    # X-345: the old forms the rewrite dropped are the only other difference permitted
                    removed = old_forms_removed(src, once)
                    if removed:
                        same, row["old_forms"] = True, len(removed)
                        facts = sum(1 for t in lexer.lex(once) if t.text in REPRINTABLE) - sum(1 for t in lexer.lex(src) if t.text in REPRINTABLE)
            except Exception as e:  # noqa: BLE001
                same, facts = False, 0
                row["detail"] = f"reference parser: {e}"
            row.update(status="formatted", idempotent=idem, tree_equal=same, changed=once != src, facts=facts, rewritten=rewritten)
            problems += not (idem and same and row["layout_free"])
        rows.append(row)
    formatted = [r for r in rows if r["status"] == "formatted"]
    refused = [r for r in rows if r["status"] == "not formatted"]
    for r in rows:
        if r["status"] == "defect" or (r["status"] == "formatted" and not (r["idempotent"] and r["tree_equal"] and r["layout_free"])) or (r["status"] == "not formatted" and not r["expected"]):
            print(f"FAIL {r['path']}: {r}")
    print(f"{len(rows)} files: {len(formatted)} formatted, {sum(r['idempotent'] for r in formatted)} idempotent, "
          f"{sum(r['tree_equal'] for r in formatted)} with the reference parse tree unchanged, {sum(r['layout_free'] for r in formatted)} format the same after random re-indentation, "
          f"{sum(r['changed'] for r in formatted)} changed by formatting, {sum(r.get('facts', 0) for r in formatted)} inferred facts printed back in {sum(1 for r in formatted if r.get('facts'))} files, "
          f"{sum(r.get('old_forms', 0) for r in formatted)} old forms rewritten in {sum(1 for r in formatted if r.get('old_forms'))} files (X-345), "
          f"{sum(1 for r in formatted if r.get('rewritten'))} rewritten into the reading forms; "
          f"{len(refused)} not formatted (syntax-error fixtures and fragments: {sum(r['expected'] for r in refused)} expected); "
          f"{sum(r['status'] == 'defect' for r in rows)} formatter defects")
    coverage = facts_coverage()
    for c in coverage:
        print(f"{'PASS' if c['ok'] else 'FAIL'} inferred-fact coverage: {c['fact']}" + (f" ({c['code']} in {c['fixture']})" if "code" in c else ""))
        problems += not c["ok"]
    migrated = migrations()
    for m in migrated:
        print(f"{'PASS' if m['ok'] else 'FAIL'} migration {m['migration']}" + (f": {m['detail']}" if m["detail"] else ""))
        problems += not m["ok"]
    if "--json" in argv:
        os.makedirs(os.path.join(ROOT, "build"), exist_ok=True)
        json.dump({"rows": rows, "facts": coverage, "migrations": migrated}, open(os.path.join(ROOT, "build", "fmt.json"), "w"), indent=1)
    print("fmt check: " + ("OK" if problems == 0 else "FAILED"))
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
