"""DEL-42: the lockfile pins every digest PROG-ARC-04 names, resolution uses only it, and divergence is refused.

`python3 -I conformance/tools/lock_check.py [--json]`

Every command runs with the network blocked (a user namespace without interfaces, `unshare -rn`), so nothing can
be fetched; the packages come from the search directory `conformance/packages`.

1. **Pinning**: `fortisc lock` on a copy of `conformance/lock/lock.ok` writes a lockfile that pins the compiler, the
   standard library and the Rust toolchain by digest, and every package the manifests reach by identity and digest.
   `fortisc lock --check` finds it holding, and `fortisc check` accepts the locked package.
2. **Only the lockfile**: with a pinned package missing from the search directory, or a toolchain digest changed in
   the lockfile (compiler, standard library, Rust), the lock is refused and so is `fortisc check`; a lockfile that
   cannot be read is refused too.
3. **Refusal fixtures**: `conformance/lock/lock.manifest` pins `acme.util@1.0.0` where the manifest names
   `acme.util@2.0.0` (E-NAM-020 naming the dependency), and `conformance/lock/lock.digest` pins a digest the package
   `acme.billing@1.4.0` does not have (E-NAM-021 naming the package).
4. **Two identities**: the lockfile of `lock.ok`, which reuses DEL-05's `acme.app` dependencies, pins two versions of
   `acme.util` as two identities, and the locked package checks.
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
FORTISC = os.path.join(ROOT, "compiler", "target", "release", "fortisc")
SEARCH = os.path.join(ROOT, "conformance", "packages")
LOCKS = os.path.join(ROOT, "conformance", "lock")


def offline():
    try:
        ok = subprocess.run(["unshare", "-rn", "true"], capture_output=True).returncode == 0
    except FileNotFoundError:
        ok = False
    return ["unshare", "-rn"] if ok else []


NET = offline()


def run(*args):
    if args[0] == "check":
        args = ("check", "--format", "text") + args[1:]
    r = subprocess.run(NET + [FORTISC] + list(args), capture_output=True, text=True, cwd=ROOT, timeout=600)
    return r.returncode, r.stdout + r.stderr


def codes(text):
    return re.findall(r"error (E-[A-Z]+-\d+) \[[^\]]+\](?: \([^)]*\))?: (.*)", text)


def main(argv):
    rows, problems = [], 0

    def expect(name, ok, detail):
        nonlocal problems
        rows.append({"check": name, "ok": ok, "detail": detail[:400]})
        problems += not ok
        print(f"{'PASS' if ok else 'FAIL'} {name}" + ("" if ok else f": {detail[:300]}"))

    expect("network blocked", bool(NET) and subprocess.run(NET + ["sh", "-c", "exec 3<>/dev/tcp/1.1.1.1/80"], capture_output=True).returncode != 0, "unshare -rn is unavailable" if not NET else "a connection succeeded")
    work = tempfile.mkdtemp(prefix="fortis-lock-")
    try:
        search = os.path.join(work, "packages")
        shutil.copytree(SEARCH, search)
        pkg = os.path.join(search, "lock.ok")      # `fortisc check` searches the package's parent directory
        shutil.copytree(os.path.join(LOCKS, "lock.ok"), pkg)
        os.remove(os.path.join(pkg, "fortis.lock"))
        code, out = run("lock", "--search", search, pkg)
        text = open(os.path.join(pkg, "fortis.lock"), encoding="utf-8").read() if code == 0 else ""
        pins = {k: re.search(rf"^{k}: (.+)$", text, re.M) for k in ("compiler", "stdlib", "rust", "root")}
        packages = re.findall(r"^  (\S+@\S+): sha256:([0-9a-f]{64})$", text, re.M)
        expect("the lockfile pins the compiler, the standard library and the Rust toolchain by digest",
               all(pins[k] and "sha256:" in pins[k].group(1) for k in ("compiler", "stdlib", "rust")), text)
        expect("the lockfile pins every package the manifests reach, by identity and digest",
               sorted(p for p, _ in packages) == ["acme.billing@1.4.0", "acme.util@1.0.0", "acme.util@2.0.0"], text)
        expect("two pinned versions are two identities", sum(1 for p, _ in packages if p.startswith("acme.util@")) == 2, text)
        code, out = run("lock", "--check", "--search", search, pkg)
        expect("a fresh lockfile holds, toolchain included", code == 0 and "the lockfile holds" in out, out)
        code, out = run("check", pkg)
        expect("the locked package checks offline with two acme.util identities", code == 0 and not codes(out), out)
        for key, what in (("compiler", "the compiler"), ("stdlib", "the standard library"), ("rust", "the Rust toolchain")):
            changed = re.sub(rf"^({key}: .*sha256:)[0-9a-f]+", r"\g<1>" + "f" * 64, text, flags=re.M)
            open(os.path.join(pkg, "fortis.lock"), "w", encoding="utf-8").write(changed)
            code, out = run("lock", "--check", "--search", search, pkg)
            c = codes(out)
            expect(f"a changed {key} digest is refused (E-NAM-021 naming {what})", code == 1 and any(k == "E-NAM-021" and what in m for k, m in c), out)
        # the Rust toolchain pin is verified by `lock --check` and `build`, not by `check`, which runs nothing (X-133)
        code, out = run("check", pkg)
        expect("`fortisc check` runs no subprocess, so it leaves the Rust toolchain pin to build", code == 0, out)
        changed = re.sub(r"^(compiler: .*sha256:)[0-9a-f]+", r"\g<1>" + "f" * 64, text, flags=re.M)
        open(os.path.join(pkg, "fortis.lock"), "w", encoding="utf-8").write(changed)
        code, out = run("check", pkg)
        expect("`fortisc check` refuses a package whose lockfile diverges (the compiler digest)", code != 0 and any(k == "E-NAM-021" for k, _ in codes(out)), out)
        open(os.path.join(pkg, "fortis.lock"), "w", encoding="utf-8").write(text)
        shutil.rmtree(os.path.join(search, "acme.util@1.0.0"))
        code, out = run("lock", "--check", "--search", search, pkg)
        expect("a pinned package missing from the search directory is refused", code == 1 and any(k == "E-NAM-001" and "acme.util@1.0.0" in m for k, m in codes(out)), out)
        open(os.path.join(pkg, "fortis.lock"), "w", encoding="utf-8").write("schema: something else\n")
        code, out = run("check", pkg)
        expect("a lockfile that cannot be read is refused", code != 0 and any(k == "E-NAM-020" for k, _ in codes(out)), out)
    finally:
        shutil.rmtree(work, ignore_errors=True)
    for name, code_want, what in (("lock.manifest", "E-NAM-020", "dependency acme.util"), ("lock.digest", "E-NAM-021", "package acme.billing@1.4.0")):
        code, out = run("lock", "--check", "--no-toolchain", "--search", SEARCH, os.path.join(LOCKS, name))
        c = codes(out)
        expect(f"fixture {name}: refused with {code_want} naming {what}", code == 1 and len(c) == 1 and c[0][0] == code_want and what in c[0][1], out)
    code, out = run("lock", "--check", "--no-toolchain", "--search", SEARCH, os.path.join(LOCKS, "lock.ok"))
    expect("fixture lock.ok holds", code == 0, out)
    print(f"\n{len(rows)} checks, {sum(r['ok'] for r in rows)} pass" + (" (network blocked)" if NET else ""))
    if "--json" in argv:
        os.makedirs(os.path.join(ROOT, "build"), exist_ok=True)
        json.dump({"rule": "PROG-ARC-04", "network_blocked": bool(NET), "checks": rows}, open(os.path.join(ROOT, "build", "lock.json"), "w"), indent=1)
    print("lock check: " + ("OK" if problems == 0 else "FAILED"))
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
