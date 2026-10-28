"""DEL-43 criterion 5 (frontend part): the PLAT-01 rules, type checking independent of the build configuration, the
three notions apart, and `check` with network and subprocesses blocked.

`python3 -I conformance/tools/features_check.py [--json]`

1. **PLAT-01-01**: the published compatibility set (`library/fortis/compatibility.yaml`) validates (schema, set
   version, every package a standard name at the set's version); a manifest naming a standard package at another
   version, or one outside the set, is refused (E-NAM-031); at the set's version it is accepted.
2. **PLAT-01-02**: every standard-library module parses with the core grammar and uses only the core's reserved
   declaration heads (LR 16); a package cannot add grammar, tokens or effects: a new head, a new token and an effect
   declaration are each a syntax error (E-SYN).
3. **PLAT-01-03 and PROG-ARC-03**: for every target and every standard module, `check --target` reports E-NAM-030
   exactly when the target's feature set lacks the module's (build_check.py covers the native side).
4. **PROG-VM-04**: every corpus module type-checks to the same records under every target (apart from E-NAM-030
   for a module the target lacks), and under the debug and release builds of the checker (frontend_check.py
   `--binary`).
5. **PROG-ARC-06**: a feature-set error (E-NAM-030, field `feature_set`), an effect error (E-EFF-001) and an
   authority error (E-EFF-010) are distinct codes; the typed IR keeps each function's effects and authorities in
   separate fields, and the build manifest also names the target's feature set (build_check.py).
6. **PROG-FND-02**: `fortisc check` over the corpus inside a new network namespace, under a seccomp filter that
   refuses process creation and network sockets, gives exactly the records it gives unrestricted; the same filter
   stops a process that tries to start a subprocess or open a network socket.
"""
import ctypes
import json
import os
import re
import struct
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
sys.path.insert(0, HERE)
FORTISC = os.path.join(ROOT, "compiler", "target", "release", "fortisc")
HEADS = {"application", "explore", "context", "provide", "inject", "model", "view", "component", "observation"}

MODULES = {"fortis.prelude": "core", "fortis.text": "core", "fortis.collections": "core", "fortis.numeric": "core", "fortis.time": "core", "fortis.money": "core",
           "fortis.codec": "alloc", "fortis.host": "host", "fortis.runtime": "host", "fortis.runtime.executor": "async", "fortis.di": "host", "fortis.data": "host",
           "fortis.telemetry": "host", "fortis.security": "host", "fortis.config": "host", "fortis.packages": "host", "fortis.http": "web", "fortis.web": "web",
           "fortis.web.spa": "browser", "fortis.web.pwa": "browser", "fortis.desktop": "desktop"}
TARGETS = {"x86_64-unknown-linux-gnu": {"core", "alloc", "host", "async", "web"}, "aarch64-apple-darwin": {"core", "alloc", "host", "async", "web"},
           "x86_64-pc-windows-msvc": {"core", "alloc", "host", "async", "web"}, "wasm32-wasip1": {"core", "alloc"}, "thumbv7em-none-eabihf": {"core"},
           "wasm32-unknown-unknown": {"core", "alloc", "browser"}}


def records(args, cwd=ROOT, **kw):
    r = subprocess.run([FORTISC] + args, capture_output=True, text=True, cwd=cwd, timeout=3600, **kw)
    return r.returncode, [json.loads(l) for l in r.stdout.splitlines() if l.startswith("{")], r


# ---- the sandbox of PROG-FND-02: a new user and network namespace, and a seccomp filter --------------------------
libc = ctypes.CDLL(None, use_errno=True)
AUDIT_ARCH_X86_64, CLONE_THREAD = 0xC000003E, 0x00010000
ALLOW, ERRNO = 0x7FFF0000, 0x00050000
NR = {"socket": 41, "clone": 56, "fork": 57, "vfork": 58, "clone3": 435}


def bpf(code, jt, jf, k):
    return struct.pack("HBBI", code, jt, jf, k)


def filter_program():
    """Refuse fork, vfork, clone without CLONE_THREAD (EPERM), clone3 (ENOSYS: threads fall back to clone) and any
    socket that is not a Unix socket (EACCES); allow everything else."""
    LD, JEQ, JSET, RET = 0x20, 0x15, 0x45, 0x06
    prog = [
        bpf(LD, 0, 0, 4),                          # 0  architecture
        bpf(JEQ, 1, 0, AUDIT_ARCH_X86_64),         # 1
        bpf(RET, 0, 0, ERRNO | 1),                 # 2  another architecture: refused
        bpf(LD, 0, 0, 0),                          # 3  system call number
        bpf(JEQ, 0, 1, NR["fork"]),                # 4
        bpf(RET, 0, 0, ERRNO | 1),                 # 5  fork: EPERM
        bpf(JEQ, 0, 1, NR["vfork"]),               # 6
        bpf(RET, 0, 0, ERRNO | 1),                 # 7  vfork: EPERM
        bpf(JEQ, 0, 1, NR["clone3"]),              # 8
        bpf(RET, 0, 0, ERRNO | 38),                # 9  clone3: ENOSYS (a thread falls back to clone)
        bpf(JEQ, 0, 4, NR["clone"]),               # 10 clone: to 11, else to 15
        bpf(LD, 0, 0, 16),                         # 11 its flags
        bpf(JSET, 0, 1, CLONE_THREAD),             # 12
        bpf(RET, 0, 0, ALLOW),                     # 13 a thread
        bpf(RET, 0, 0, ERRNO | 1),                 # 14 a process: EPERM
        bpf(JEQ, 0, 4, NR["socket"]),              # 15 socket: to 16, else to 20
        bpf(LD, 0, 0, 16),                         # 16 its domain
        bpf(JEQ, 0, 1, 1),                         # 17 AF_UNIX
        bpf(RET, 0, 0, ALLOW),                     # 18 a Unix socket
        bpf(RET, 0, 0, ERRNO | 13),                # 19 a network socket: EACCES
        bpf(RET, 0, 0, ALLOW),                     # 20 anything else
    ]
    return prog


def sandbox():
    """preexec_fn: enter new namespaces (no network), then install the filter."""
    if libc.unshare(0x10000000 | 0x40000000) != 0:      # CLONE_NEWUSER | CLONE_NEWNET
        os._exit(97)
    prog = filter_program()
    buf = ctypes.create_string_buffer(b"".join(prog))
    class SockFprog(ctypes.Structure):
        _fields_ = [("len", ctypes.c_ushort), ("filter", ctypes.c_void_p)]
    fprog = SockFprog(len(prog), ctypes.cast(buf, ctypes.c_void_p))
    if libc.prctl(38, 1, 0, 0, 0) != 0:                    # PR_SET_NO_NEW_PRIVS
        os._exit(98)
    if libc.prctl(22, 2, ctypes.byref(fprog), 0, 0) != 0:  # PR_SET_SECCOMP, SECCOMP_MODE_FILTER
        os._exit(99)


def main(argv):
    rows, problems = [], 0

    def expect(name, ok, detail=""):
        nonlocal problems
        rows.append({"check": name, "ok": bool(ok), "detail": str(detail)[:700]})
        problems += not ok
        print(f"{'PASS' if ok else 'FAIL'} {name}" + ("" if ok else f": {str(detail)[:400]}"))

    work = tempfile.mkdtemp(prefix="fortis-features-")
    # 1. PLAT-01-01
    text = open(os.path.join(ROOT, "library", "fortis", "compatibility.yaml"), encoding="utf-8").read()
    pkgs = dict(re.findall(r"^  ([a-z.]+): \"([^\"]+)\"$", text, re.M))
    setv = re.search(r'^set: "([^"]+)"', text, re.M)
    expect("the compatibility set validates: schema, set version, every package a standard name at the set's version",
           "schema: fortis.compatibility/v1" in text and setv and pkgs and all((p == "fortis" or p.startswith("fortis.")) and v == setv.group(1) for p, v in pkgs.items()), pkgs)
    code, recs, _ = records(["check", "--format", "json", "conformance/features/mixed.version"])
    expect("a standard package at a version outside the set is refused (E-NAM-031)", code == 1 and [r["code"] for r in recs] == ["E-NAM-031"] and "2.0" in recs[0]["message"])

    def manifest_case(name, deps):
        d = os.path.join(work, name)
        os.makedirs(os.path.join(d, "src"), exist_ok=True)
        open(os.path.join(d, "project.yaml"), "w").write(f"schema: fortis.project/v1\nname: {name}\nversion: 1.0.0\nexports: [app]\ndependencies:\n{deps}")
        open(os.path.join(d, "src", "app.ft"), "w").write("export pure func answer() Int => 42\n")
        return records(["check", "--format", "json", d])
    code, recs, _ = manifest_case("not.in.set", '  fortis: "1.0"\n  fortis.unknown: "1.0"\n')
    expect("a standard package that is not in the set is refused (E-NAM-031)", code == 1 and [r["code"] for r in recs] == ["E-NAM-031"])
    code, recs, _ = manifest_case("in.set", '  fortis: "1.0"\n  fortis.http: "1.0"\n')
    expect("standard packages at the set's version are accepted", code == 0 and not recs, recs)
    # 2. PLAT-01-02
    lib = [os.path.join(dp, f) for dp, _, fs in os.walk(os.path.join(ROOT, "library")) for f in fs if f.endswith(".ft")]
    out = subprocess.run([FORTISC, "syntax"] + lib, capture_output=True, text=True, cwd=ROOT).stdout
    bad = [l for l in out.splitlines() if "\tok\t" not in l]
    used_heads = {h for f in lib for h in re.findall(r"^\s*(?:export\s+)?([a-z]+)\s+[A-Z]\w*\s*\{", open(f, encoding="utf-8").read(), re.M)} - {"class", "enum", "interface", "mixin", "error", "contract", "aspect", "type"}
    expect(f"every standard-library module ({len(lib)}) parses with the core grammar and uses only reserved heads", not bad and used_heads <= HEADS, bad[:3] or used_heads)
    for name, src in (("a new declaration head", "widget Panel {\n  size: Int\n}\n"), ("a new token", "export pure func f(a: Int) Int => a @@ 1\n"), ("an effect declaration", "effect telemetry\n")):
        p = os.path.join(work, "ext.ft")
        open(p, "w").write(src)
        code, recs, _ = records(["check", "--format", "json", p])
        expect(f"a package cannot add grammar: {name} is a syntax error", code == 1 and recs and recs[0]["code"].startswith("E-SYN"), recs)
    # 3. PLAT-01-03
    wrong = []
    for module, need in MODULES.items():
        p = os.path.join(work, "imp.ft")
        open(p, "w").write(f"import {module}\n")
        for target, have in TARGETS.items():
            code, recs, _ = records(["check", "--format", "json", "--target", target, p])
            got = any(r["code"] == "E-NAM-030" for r in recs)
            if got != (need not in have):
                wrong.append((module, target, got))
    expect(f"check --target reports E-NAM-030 exactly when the target lacks the module ({len(MODULES)} modules x {len(TARGETS)} targets)", not wrong, wrong[:5])
    # 4. PROG-VM-04
    corpus = sorted(os.path.join(d, f) for d in ("conformance/programs", "conformance/corpus", "examples") for f in os.listdir(os.path.join(ROOT, d)) if f.endswith(".ft"))
    _, base, _ = records(["check", "--format", "json"] + corpus)
    differs = []
    for target in TARGETS:
        _, recs, _ = records(["check", "--format", "json", "--target", target] + corpus)
        if [r for r in recs if r["code"] != "E-NAM-030"] != base:
            differs.append(target)
    expect(f"{len(corpus)} corpus modules type-check to the same records under every target ({len(TARGETS)})", not differs, differs)
    subprocess.run(["cargo", "build", "-q", "-p", "fortisc"], cwd=os.path.join(ROOT, "compiler"), capture_output=True, timeout=3600)
    dbg = subprocess.run([sys.executable, "-I", os.path.join(HERE, "frontend_check.py"), "--binary", "compiler/target/debug/fortisc"], capture_output=True, text=True, cwd=ROOT)
    line = next((l for l in dbg.stdout.splitlines() if l.startswith("builds:")), "")
    expect("the debug and release builds of the checker give identical records (frontend_check --binary)", "0 differ" in line, line or dbg.stdout[-300:])
    # 5. PROG-ARC-06
    arc = os.path.join(work, "arc.ft")
    open(arc, "w").write("import fortis.host { Console }\n\npure func a(console: Console) Void {\n  console.print(\"x\")\n}\n\nfunc b(console: Console) Void {\n  console.print(\"y\")\n}\n")
    _, recs_t, _ = records(["check", "--format", "json", "--target", "thumbv7em-none-eabihf", arc])
    eff = os.path.join(work, "eff.ft")
    open(eff, "w").write("import fortis.host { Host }\n\nfunc helper() Void {\n  let caps = Host.capabilities()\n  caps.console.print(\"z\")\n}\n")
    _, recs_e, _ = records(["check", "--format", "json", eff])
    _, recs_p, _ = records(["check", "--format", "json", arc])
    codes = {r["code"]: r for r in recs_t + recs_e + recs_p}
    expect("feature set, effect and authority errors are distinct codes (E-NAM-030, E-EFF-001, E-EFF-010)",
           {"E-NAM-030", "E-EFF-001", "E-EFF-010"} <= set(codes) and "feature_set" in codes["E-NAM-030"]["fields"] and codes["E-NAM-030"]["class"] != codes["E-EFF-001"]["class"], sorted(codes))
    mir = subprocess.run([FORTISC, "mir", "--json", "examples/04_collections.ft"], capture_output=True, text=True, cwd=ROOT).stdout.splitlines()
    fns = json.loads(mir[0])["functions"] if mir else []
    expect("the typed IR keeps each function's effects and authorities in separate fields", fns and all({"effects", "authorities"} <= set(f.get("effects", {})) for f in fns), len(fns))
    # 6. PROG-FND-02
    plain = subprocess.run([FORTISC, "check", "--format", "json"] + corpus, capture_output=True, text=True, cwd=ROOT)
    boxed = subprocess.run([FORTISC, "check", "--format", "json"] + corpus, capture_output=True, text=True, cwd=ROOT, preexec_fn=sandbox)
    expect(f"check over {len(corpus)} modules with network and subprocesses blocked gives exactly its unrestricted records ({len(plain.stdout.splitlines())})",
           boxed.returncode == plain.returncode and boxed.stdout == plain.stdout and plain.stdout, (boxed.returncode, boxed.stderr[-300:]))
    locked = subprocess.run([FORTISC, "check", "--format", "json", "conformance/lock/lock.ok"], capture_output=True, text=True, cwd=ROOT, preexec_fn=sandbox)
    expect("check of a locked package works in the sandbox (no toolchain subprocess)", locked.returncode in (0, 1) and "Rust toolchain" not in locked.stdout, locked.stdout[-300:] + locked.stderr[-200:])
    spawn = subprocess.run([sys.executable, "-I", "-c", "import subprocess; subprocess.run(['true'])"], capture_output=True, text=True, preexec_fn=sandbox)
    sock = subprocess.run([sys.executable, "-I", "-c", "import socket; socket.socket(socket.AF_INET, socket.SOCK_STREAM)"], capture_output=True, text=True, preexec_fn=sandbox)
    unix = subprocess.run([sys.executable, "-I", "-c", "import socket, threading; socket.socket(socket.AF_UNIX); t = threading.Thread(target=lambda: None); t.start(); t.join()"], capture_output=True, text=True, preexec_fn=sandbox)
    expect("the sandbox stops a subprocess and a network socket, and allows threads and Unix sockets",
           spawn.returncode != 0 and sock.returncode != 0 and unix.returncode == 0, (spawn.stderr[-150:], sock.stderr[-150:], unix.stderr[-150:]))
    if "--json" in argv:
        os.makedirs(os.path.join(ROOT, "build"), exist_ok=True)
        json.dump({"rules": ["PLAT-01-01", "PLAT-01-02", "PLAT-01-03", "PROG-ARC-03", "PROG-VM-04", "PROG-ARC-06", "PROG-FND-02"], "checks": rows}, open(os.path.join(ROOT, "build", "features.json"), "w"), indent=1)
    print(f"\n{len(rows)} checks, {sum(r['ok'] for r in rows)} pass")
    print("features check: " + ("OK" if problems == 0 else "FAILED"))
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
