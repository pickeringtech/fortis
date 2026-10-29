"""DEL-44 (DW-09, PROG-SEC-02): the toolchain's build, test and agent contexts run with least privilege, and every
external effect a program has is authorised explicitly.

`python3 -I conformance/tools/privilege_check.py [--json]`

Each context runs under the restrictions its declaration (the delivery plan's privileges table, a maintainers'
planning file; without it the declaration check is skipped) states, and must still do
its job:

- **check, fmt, view** (and the agent skill, which uses only these and `test`): a new network namespace and a
  seccomp filter refusing process creation and network sockets (features_check.py's sandbox); the repository and
  the temporary directory are unchanged afterwards (they never write).
- **test** (a program's tests and runs on the VM): the same sandbox; a program's file writes land only in its
  development root (`FORTIS_DEV_ROOT`), and a program that escapes the root, reaches the network, starts a program
  or uses raw SQL gets a failure, not the effect.
- **build**: no network (a new network namespace), a scrubbed environment, and a `PATH` holding only cargo (offline,
  for the runtime's own path crates), rustc, the C linker driver and the linker rustc selects (ld.lld); the build succeeds, and without rustc on that
  `PATH` it fails with `E-INT-003` rather than finding another way.
- **agent**: the fortis-code skill names only `fortis view`, `fortis check`, `fortis fmt` and `fortis test`, and is
  installed as project-scope links (no user-level configuration is changed).
- **generator**: Language 1.0 has none (scaffolds belong to the Developer tooling, which declares them).
"""
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
from features_check import libc, sandbox  # noqa: E402

FORTISC = os.path.join(ROOT, "compiler", "target", "release", "fortisc")


def no_network():
    """preexec_fn: a new user and network namespace, and nothing else (the build context may start rustc and cc)."""
    if libc.unshare(0x10000000 | 0x40000000) != 0:      # CLONE_NEWUSER | CLONE_NEWNET
        os._exit(97)


def snapshot():
    snap = {}
    for root, dirs, files in os.walk(ROOT):
        dirs[:] = [d for d in dirs if d not in (".git", "build", "target")]
        for f in files:
            p = os.path.join(root, f)
            try:
                st = os.stat(p)
            except OSError:
                continue
            snap[p] = (st.st_size, st.st_mtime_ns)
    return snap, sorted(os.listdir(tempfile.gettempdir()))


def main(argv):
    rows, problems = [], 0

    def expect(name, ok, detail=""):
        nonlocal problems
        rows.append({"check": name, "ok": bool(ok), "detail": str(detail)[:500]})
        problems += not ok
        print(f"{'PASS' if ok else 'FAIL'} {name}" + ("" if ok else f": {str(detail)[:300]}"))

    sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
    import planning
    decl_path = planning.optional(planning.PRIVILEGES, "privilege", "checking the declarations (every context declared; no generator context)")
    if decl_path:
        decl = json.load(open(decl_path, encoding="utf-8"))
        expect("every context has a privilege declaration", {c["context"] for c in decl["contexts"]} >= {"check", "fmt", "view", "test", "build", "agent", "generator"})
    # check, fmt, view: sandboxed, and they never write
    before = snapshot()
    r = subprocess.run([FORTISC, "check", "--format", "json", "examples/apps/orders"], capture_output=True, text=True, cwd=ROOT, preexec_fn=sandbox, timeout=600)
    clean = r.returncode == 0 and not r.stdout.strip()
    r = subprocess.run([FORTISC, "check", "--format", "json", "examples/10_rejected.ft"], capture_output=True, text=True, cwd=ROOT, preexec_fn=sandbox, timeout=600)
    expect("`fortisc check` does its job with no network and no subprocess (a clean package, and a rejected example's records)", clean and r.returncode != 0 and '"code": "E-' in r.stdout, r.stdout[-200:])
    for args in (["fmt", "examples/04_collections.ft"], ["view", "--decls", "examples/apps/orders/src"]):
        r = subprocess.run([FORTISC] + args, capture_output=True, text=True, cwd=ROOT, preexec_fn=sandbox, timeout=600)
        expect(f"`fortisc {args[0]}` does its job with no network and no subprocess", r.returncode == 0 and r.stdout, (r.returncode, r.stderr[-200:]))
    after = snapshot()
    expect("check, fmt and view wrote nothing (repository and temporary directory unchanged)", before == after)
    # test: sandboxed; writes only in the development root; process and network effects refused
    root = tempfile.mkdtemp(prefix="fortis-priv-")
    try:
        before = snapshot()[0]
        r = subprocess.run([FORTISC, "vm", "conformance/programs/exec_host_filesystem.ft"], capture_output=True, text=True, cwd=ROOT, preexec_fn=sandbox,
                           env={**os.environ, "FORTIS_DEV_ROOT": root}, timeout=600)
        wrote = sorted(os.listdir(root))
        expect("a program run on the VM writes only into its development root", r.returncode == 0 and "observed: " in r.stdout and wrote and snapshot()[0] == before, (wrote, r.stdout[-200:]))
        r = subprocess.run([FORTISC, "vm", "conformance/programs/exec_host_violations.ft"], capture_output=True, text=True, cwd=ROOT, preexec_fn=sandbox,
                           env={**os.environ, "FORTIS_DEV_ROOT": root}, timeout=600)
        obs = next((l for l in r.stdout.splitlines() if l.startswith("observed: ")), "")
        want = "observed: fs.outside_root|fs.outside_root|fs.closed|network.unreachable|clock.future|process.denied|sql.unavailable|path.invalid"
        expect("in the test context, an effect outside the program's authorisation reaches it as a failure, not the effect (escaping the root, the network, a process, raw SQL)",
               obs == want and not os.path.exists(os.path.join(os.path.dirname(root), "outside.txt")), obs)
        r = subprocess.run([FORTISC, "test", "--format", "json", "examples/apps/orders"], capture_output=True, text=True, cwd=ROOT, preexec_fn=sandbox, timeout=600)
        expect("`fortisc test` runs a package's tests with no network and no subprocess", r.returncode == 0 and '"test-summary"' in r.stdout, r.stdout[-200:])
    finally:
        shutil.rmtree(root, ignore_errors=True)
    # build: no network, scrubbed environment, PATH of rustc and cc only
    bindir = tempfile.mkdtemp(prefix="fortis-path-")
    out = tempfile.mkdtemp(prefix="fortis-out-")
    try:
        for tool in ("cargo", "rustc", "cc", "ld.lld"):
            os.symlink(shutil.which(tool, path="/usr/bin") or f"/usr/bin/{tool}", os.path.join(bindir, tool))
        env = {"PATH": bindir, "HOME": os.environ.get("HOME", out), "LANG": "C.UTF-8"}
        r = subprocess.run([FORTISC, "build", "--format", "json", "--out", os.path.join(out, "plain"), "conformance/build/plain.ft"], capture_output=True, text=True, cwd=ROOT, env=env, preexec_fn=no_network, timeout=1800)
        expect("`fortisc build` builds with no network, a scrubbed environment and only cargo, rustc, cc and ld.lld on PATH", r.returncode == 0 and os.path.exists(os.path.join(out, "plain")), (r.stdout + r.stderr)[-300:])
        os.remove(os.path.join(bindir, "rustc"))
        shutil.rmtree(os.path.join(ROOT, "build", "native-build", "fb_imports"), ignore_errors=True)
        r = subprocess.run([FORTISC, "build", "--format", "json", "--out", os.path.join(out, "imports"), "conformance/build/imports.ft"], capture_output=True, text=True, cwd=ROOT, env=env, preexec_fn=no_network, timeout=1800)
        codes = sorted({json.loads(l)["code"] for l in r.stdout.splitlines() if l.startswith("{") and '"code"' in l})
        expect("without rustc on that PATH the build fails with E-INT-003 rather than finding another way", r.returncode != 0 and codes == ["E-INT-003"] and not os.path.exists(os.path.join(out, "imports")), (codes, (r.stdout + r.stderr)[-200:]))
    finally:
        shutil.rmtree(bindir, ignore_errors=True)
        shutil.rmtree(out, ignore_errors=True)
    # agent: the skill's commands, and project-scope installation
    skill = open(os.path.join(ROOT, "skills", "fortis-code", "SKILL.md"), encoding="utf-8").read()
    commands = sorted(set(re.findall(r"`fortis (\w+)", skill)))
    expect("the agent skill uses only fortis view, check, fmt and test", set(commands) <= {"view", "check", "fmt", "test"}, commands)
    links = [os.path.join(ROOT, d, "skills", "fortis-code") for d in (".claude", ".agents", ".grok")]
    expect("the skill is installed as project-scope links only", all(os.path.islink(l) and os.path.realpath(l) == os.path.join(ROOT, "skills", "fortis-code") for l in links if os.path.lexists(l)) and any(os.path.lexists(l) for l in links))
    if decl_path:
        gen = next(c for c in decl["contexts"] if c["context"] == "generator")
        expect("generator context: none in Language 1.0 (declared)", gen.get("present") is False)
    if "--json" in argv:
        os.makedirs(os.path.join(ROOT, "build"), exist_ok=True)
        json.dump({"rule": "PROG-SEC-02", "checks": rows}, open(os.path.join(ROOT, "build", "privilege.json"), "w"), indent=1)
    print(f"\n{len(rows)} checks, {sum(r['ok'] for r in rows)} pass")
    print("privilege check: " + ("OK" if problems == 0 else "FAILED"))
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
