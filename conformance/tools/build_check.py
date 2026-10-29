"""DEL-43 criteria 4 and 5 (native part): offline, reproducible native builds with a manifest, and target feature
sets enforced before native compilation.

`python3 -I conformance/tools/build_check.py [--json] [--no-clones]`

Every build runs with the network blocked (`unshare -rn`).

1. **Reproducible from clean clones** (PROG-NFR-22): the committed tree is cloned twice into different directories
   (a tree that is not a git checkout, such as a plain download, is copied twice, less its build output);
   each clone builds its own compiler (paths remapped) and then `examples/apps/orders` against one `fortis.lock`
   written by the first clone's compiler. Both clones must verify the lockfile (so their compilers are identical)
   and produce byte-identical executables; their manifests may differ only in the fields the manifest itself
   lists as nondeterministic, and the ELF sections that differ are disclosed (none are expected). The full tier
   runs the clones; `--no-clones` (the quick tier) leaves them out but still builds one package copied to two
   directories, which must give byte-identical executables that hold neither directory (D-302).
2. **No build scripts, vendored crates** (PROG-CMP-06): a package that ships a build script is refused before Cargo
   runs (E-NAM-032, and no crate is generated); the crate graph of a build holds only the toolchain's path crates.
3. **Compile and link apart; no rewrites** (PROG-NFR-07): a build reports compile and link time separately (the link
   timed by `fortisc` acting as the linker); building an unchanged program rewrites no generated file (their
   modification times are unchanged) and links nothing.
4. **Changes exposed** (PROG-NFR-15): rebuilding after a change that adds an allocation reports the code-size delta
   and the function whose allocation changed.
5. **Feature sets** (PROG-ARC-03, PLAT-01-03): a module the target lacks fails `check --target` and `build --target`
   with E-NAM-030 before any native step; the same program builds for the host.
6. **Kits** (PLAT-01-04, PLAT-01-05): a program importing standard modules it never uses compiles to the same
   executable as one that imports nothing, and runs identically on the VM.
7. **Trust and the three notions** (PROG-ARC-05, PROG-ARC-06): the manifest lists, per function, the trusted
   operations it reaches, and keeps the target's feature set, each function's effects and its authorities in
   separate fields.
"""
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
FORTISC = os.path.join(ROOT, "compiler", "target", "release", "fortisc")
BUILD = os.path.join(ROOT, "conformance", "build")


def offline():
    try:
        return ["unshare", "-rn"] if subprocess.run(["unshare", "-rn", "true"], capture_output=True).returncode == 0 else []
    except FileNotFoundError:
        return []


NET = offline()


def run(args, cwd=ROOT, env=None, timeout=3600):
    r = subprocess.run(NET + args, capture_output=True, text=True, cwd=cwd, timeout=timeout, env=env)
    return r.returncode, r.stdout, r.stderr


def sha(path):
    return hashlib.sha256(open(path, "rb").read()).hexdigest()


def manifest(path):
    return json.load(open(path + ".build.json", encoding="utf-8"))


def main(argv):
    rows, problems = [], 0

    def expect(name, ok, detail=""):
        nonlocal problems
        rows.append({"check": name, "ok": bool(ok), "detail": detail if isinstance(detail, (dict, list)) else str(detail)[:800]})
        problems += not ok
        print(f"{'PASS' if ok else 'FAIL'} {name}" + ("" if ok else f": {str(detail)[:500]}"))

    expect("network blocked for every build", bool(NET) and subprocess.run(NET + ["sh", "-c", "exec 3<>/dev/tcp/1.1.1.1/80"], capture_output=True).returncode != 0)
    work = tempfile.mkdtemp(prefix="fortis-build-")
    try:
        out = os.path.join(work, "bin")
        os.makedirs(out)
        # 2. build scripts
        gen_dir = os.path.join(ROOT, "build", "native-build", "fb_with_build_script")
        shutil.rmtree(gen_dir, ignore_errors=True)
        code, so, se = run([FORTISC, "build", "--format", "json", os.path.join(BUILD, "with.build.script")])
        recs = [json.loads(l) for l in so.splitlines() if l.startswith("{") and '"code"' in l]
        expect("a package that ships a build script is refused before Cargo runs (E-NAM-032; no crate generated)",
               code == 1 and recs and all(r["code"] == "E-NAM-032" for r in recs) and not os.path.exists(gen_dir), recs)
        code, so, se = run([FORTISC, "check", "--format", "json", os.path.join(BUILD, "with.build.script")])
        expect("check accepts it: a build script is a native-build concern", code == 0, so + se)
        # 3. compile and link apart, no rewrites
        orders = os.path.join(out, "orders")
        gen = os.path.join(ROOT, "build", "native-build", "fb_orders")
        shutil.rmtree(gen, ignore_errors=True)     # a fresh crate: the first build compiles and links
        code, so, se = run([FORTISC, "build", "--format", "text", "--out", orders, "examples/apps/orders"])
        m1 = manifest(orders) if code == 0 else {}
        t = m1.get("timings", {})
        expect("a build reports compile and link time separately", code == 0 and float(t.get("compile_ms", 0)) > 0 and float(t.get("link_ms", 0)) > 0, (so + se)[-400:] or t)
        mtimes = {f: os.stat(os.path.join(gen, f)).st_mtime_ns for f in ("Cargo.toml", "src/main.rs")} if code == 0 else {}
        time.sleep(1.1)
        code, so, se = run([FORTISC, "build", "--format", "text", "--out", orders, "examples/apps/orders"])
        m2 = manifest(orders) if code == 0 else {}
        after = {f: os.stat(os.path.join(gen, f)).st_mtime_ns for f in ("Cargo.toml", "src/main.rs")} if code == 0 else {}
        expect("an unchanged program rewrites no generated file and links nothing",
               code == 0 and m2.get("rewritten") == [] and after == mtimes and float(m2.get("timings", {}).get("link_ms", 1)) == 0 and m2["binary"]["sha256"] == m1["binary"]["sha256"], m2.get("timings"))
        # 7. trust and the three notions
        trust = {t["operation"]: t for t in m1.get("trust", [])}
        main_ops = {a["operation"] for a in trust.get("shop.orders.main::function main", {}).get("assumptions", [])}
        expect("the manifest lists each function's trusted operations, transitively (main reaches FileSystem.readText and Console.print)",
               {"FileSystem.readText", "Console.print"} <= main_ops and all("assumptions" in t for t in trust.values()), sorted(main_ops))
        fns = m1.get("functions", {})
        expect("feature set, effects and authorities are separate fields of the manifest",
               m1.get("target", {}).get("feature_set") and fns and all(set(v) == {"effects", "authorities"} for v in fns.values()), m1.get("target"))
        crate_graph = subprocess.run(["cargo", "metadata", "--offline", "--format-version", "1"], capture_output=True, text=True, cwd=gen)
        pkgs = sorted(p["name"] for p in json.loads(crate_graph.stdout)["packages"]) if crate_graph.returncode == 0 else []
        expect("the crate graph holds only the generated crate and the toolchain's vendored runtime crates, none with a build script",
               pkgs == ["fb_orders", "fortis-native", "fortis-runtime"], pkgs)
        # 1, in one checkout: where a package lies does not reach its executable (D-302). Two copies build to the
        # same bytes, and neither executable holds the directory it was built from.
        places = [os.path.join(work, "here", "orders"), os.path.join(work, "a-second-place", "deeper", "orders")]
        copies = []
        for i, place in enumerate(places):
            shutil.copytree(os.path.join(ROOT, "examples", "apps", "orders"), place)
            exe = os.path.join(out, f"orders-{i}")
            code, so, se = run([FORTISC, "build", "--format", "text", "--out", exe, place])
            copies.append(open(exe, "rb").read() if code == 0 else (so + se)[-300:])
        ok = all(isinstance(b, bytes) for b in copies)
        expect("a package built from two directories gives byte-identical executables, and neither holds its directory (D-302)",
               ok and copies[0] == copies[1] and not any(work.encode() in b for b in copies), copies if not ok else "the executables differ or hold the directory")
        # 4. changes exposed
        grow_dir = os.path.join(work, "grow")
        os.makedirs(grow_dir)
        prog = open(os.path.join(BUILD, "plain.ft"), encoding="utf-8").read()
        open(os.path.join(grow_dir, "app.ft"), "w").write(prog)
        grow = os.path.join(out, "grow")
        c1, _, e1 = run([FORTISC, "build", "--format", "text", "--out", grow, "app.ft"], cwd=grow_dir)
        changed = prog.replace("  discard total\n", "  let label = \"total ${total}\"\n  discard label.scalarCount()\n")
        open(os.path.join(grow_dir, "app.ft"), "w").write(changed)
        c2, s2, e2 = run([FORTISC, "build", "--format", "text", "--out", grow, "app.ft"], cwd=grow_dir)
        ch = manifest(grow).get("changes", {}) if c2 == 0 else {}
        alloc = [a for a in ch.get("allocation", []) if a["function"].endswith("function main")]
        expect("a change that adds an allocation is reported with its code-size delta and the function whose allocation changed",
               c1 == 0 and c2 == 0 and ch.get("code_size", {}).get("bytes", 0) != 0 and alloc and alloc[0]["after"]["allocates"] and not alloc[0]["before"]["allocates"], (s2 + e2)[-300:] or ch)
        # 5. feature sets
        host = os.path.join(BUILD, "host.ft")
        for target in ("wasm32-wasip1", "thumbv7em-none-eabihf"):
            code, so, _ = run([FORTISC, "check", "--format", "json", "--target", target, host])
            crec = [json.loads(l) for l in so.splitlines() if l.startswith("{")]
            gen_host = os.path.join(ROOT, "build", "native-build", "fb_host")
            shutil.rmtree(gen_host, ignore_errors=True)
            bcode, bso, _ = run([FORTISC, "build", "--format", "json", "--target", target, host])
            brec = [json.loads(l) for l in bso.splitlines() if l.startswith("{") and '"code"' in l]
            expect(f"`fortis.host` is absent on {target}: check and build refuse it with E-NAM-030 before any native step",
                   code == 1 and [r["code"] for r in crec] == ["E-NAM-030"] and bcode == 1 and [r["code"] for r in brec] == ["E-NAM-030"] and not os.path.exists(gen_host), crec + brec)
        code, so, se = run([FORTISC, "build", "--format", "text", "--out", os.path.join(out, "host"), host])
        expect("the same program builds for the host target", code == 0, so + se)
        # 6. kits: unused imports are not linked and do no work
        bins = []
        for name in ("plain.ft", "imports.ft"):
            d = os.path.join(work, name.split(".")[0])
            os.makedirs(d)
            shutil.copy(os.path.join(BUILD, name), os.path.join(d, "app.ft"))
            exe = os.path.join(d, "app")
            code, so, se = run([FORTISC, "build", "--format", "text", "--out", exe, "app.ft"], cwd=d)
            bins.append(sha(exe) if code == 0 else (so + se))
        expect("a program importing modules it never uses builds to the same executable as one importing nothing (PLAT-01-05)", bins[0] == bins[1], bins)
        vm = [run([FORTISC, "vm", os.path.join(BUILD, n)])[1].replace(n, "") for n in ("plain.ft", "imports.ft")]
        expect("importing standard modules performs no runtime work: the VM observes the same run (PLAT-01-04)", vm[0] == vm[1] and "observed: " in vm[0], vm)
        # 1. reproducible from clean clones
        if "--no-clones" not in argv:
            # a git checkout is cloned at its commit; a plain download (no git) is copied, less its build output
            in_git = subprocess.run(["git", "rev-parse", "--is-inside-work-tree"], capture_output=True, text=True, cwd=ROOT).stdout.strip() == "true"
            commit = subprocess.run(["git", "rev-parse", "HEAD"], capture_output=True, text=True, cwd=ROOT).stdout.strip() if in_git else "(not a git checkout)"
            dirty = subprocess.run(["git", "status", "--porcelain", "--untracked-files=no"], capture_output=True, text=True, cwd=ROOT).stdout.strip() if in_git else ""
            clones = [os.path.join(work, "clone-a"), os.path.join(work, "the-second-clone")]
            built = []
            for c in clones:
                if in_git:
                    subprocess.run(["git", "clone", "-q", "--no-hardlinks", ROOT, c], check=True)
                    subprocess.run(["git", "checkout", "-q", commit], cwd=c, check=True)
                else:
                    sys.path.insert(0, HERE)
                    import hygiene_check
                    for rel in hygiene_check.walked_files():
                        os.makedirs(os.path.dirname(os.path.join(c, rel)), exist_ok=True)
                        shutil.copy2(os.path.join(ROOT, rel), os.path.join(c, rel), follow_symlinks=False)
                env = dict(os.environ, RUSTFLAGS=f"--remap-path-prefix={c}=/fortis --remap-path-prefix={os.path.expanduser('~')}/.cargo=/cargo", CARGO_TARGET_DIR=os.path.join(c, "compiler", "target"))
                t0 = time.time()
                code, so, se = run(["cargo", "build", "--release", "--offline", "-q", "-p", "fortisc"], cwd=os.path.join(c, "compiler"), env=env, timeout=7200)
                built.append((code, time.time() - t0, se[-300:]))
            fortisc = [os.path.join(c, "compiler", "target", "release", "fortisc") for c in clones]
            expect("each clean clone builds its compiler offline, and the two compilers are identical", all(b[0] == 0 for b in built) and sha(fortisc[0]) == sha(fortisc[1]),
                   [(b[0], round(b[1]), b[2]) for b in built])
            # one lockfile, written by the first clone's compiler, used by both
            code, so, se = run([fortisc[0], "lock", os.path.join(clones[0], "examples", "apps", "orders")], cwd=clones[0])
            lock = open(os.path.join(clones[0], "examples", "apps", "orders", "fortis.lock"), encoding="utf-8").read() if code == 0 else ""
            shutil.copy(os.path.join(clones[0], "examples", "apps", "orders", "fortis.lock"), os.path.join(clones[1], "examples", "apps", "orders", "fortis.lock")) if code == 0 else None
            exes, mans, outs = [], [], []
            for c, f in zip(clones, fortisc):
                exe = os.path.join(c, "build", "orders")
                code, so, se = run([f, "build", "--format", "text", "--out", exe, "examples/apps/orders"], cwd=c)
                outs.append((code, so + se))
                exes.append(exe)
                mans.append(manifest(exe) if code == 0 else {})
            ok_build = all(o[0] == 0 for o in outs)
            expect("both clones build examples/apps/orders offline from the one lockfile (verified, toolchain included)", ok_build and "compiler" in lock, [o[1][-300:] for o in outs])
            same = ok_build and sha(exes[0]) == sha(exes[1])
            secs = [json.loads(subprocess.run([FORTISC, "sections", e], capture_output=True, text=True).stdout or "{}") for e in exes] if ok_build else [{}, {}]
            differing = sorted(k for k in set(secs[0]) | set(secs[1]) if secs[0].get(k) != secs[1].get(k))
            nondet = set(mans[0].get("nondeterministic", []))
            strip = lambda m: {k: v for k, v in m.items() if k not in nondet and k != "program"}  # noqa: E731
            expect(f"the two executables are byte-identical ({len(secs[0])} ELF sections compared; differing sections disclosed: {differing or 'none'})", same and not differing, differing)
            expect(f"the manifests agree except their disclosed nondeterministic fields {sorted(nondet)}", ok_build and strip(mans[0]) == strip(mans[1]),
                   sorted(k for k in strip(mans[0]) if strip(mans[0]).get(k) != strip(mans[1]).get(k)))
            rows.append({"check": "clean-clone record", "ok": True, "detail": {"commit": commit, "uncommitted_tracked_changes": bool(dirty), "compiler_build_seconds": [round(b[1]) for b in built],
                                                                               "binary_sha256": sha(exes[0]) if same else None, "sections": len(secs[0]), "differing_sections": differing}})
    finally:
        shutil.rmtree(work, ignore_errors=True)
    if "--json" in argv:
        os.makedirs(os.path.join(ROOT, "build"), exist_ok=True)
        json.dump({"rules": ["PROG-NFR-22", "PROG-CMP-06", "PROG-NFR-07", "PROG-NFR-15", "PROG-ARC-03", "PROG-ARC-05", "PROG-ARC-06", "PLAT-01-03", "PLAT-01-04", "PLAT-01-05"],
                   "network_blocked": bool(NET), "checks": rows}, open(os.path.join(ROOT, "build", "build.json"), "w"), indent=1)
    n = sum(1 for r in rows if r["check"] != "clean-clone record")
    print(f"\n{n} checks, {sum(r['ok'] for r in rows if r['check'] != 'clean-clone record')} pass")
    print("build check: " + ("OK" if problems == 0 else "FAILED"))
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
