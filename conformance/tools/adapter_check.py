"""PROG-ARC-02 over the native host adapters (DEL-47): every capability's boundary manifest declares ownership,
effects, failures, allocation and cancellation for each operation, and the declarations agree with the sources.

`python3 -I conformance/tools/adapter_check.py [--json] [--no-native]`

For each of the ten capabilities of `library/fortis/host.ft`, the manifest
`compiler/crates/fortis-native/adapters/<Capability>.json` must:

1. cover every operation of the capability (its methods, plus `File`, `Path.parse`, `reveal` and `Scope.detach`
   where the capability owns them), each with the five facets present and non-empty (`failures` may be `[]`);
2. declare exactly the effects the checker derives for the signature (checker/effects.py `library_summary`);
3. declare exactly the failure codes named by the operation's contract comment, each of which the native adapter
   can produce (it appears in `fortis-native/src/host.rs`); a `try` operation declares at least one, and only a
   `try` operation declares any;
4. call an `async` operation a cancellation point, and no other operation one;
5. name a contract-violation fixture the toolchain refuses: statically (`fortisc check` reports the code) or natively
   (the native run of the fixture prints the code).

Every manifest under `conformance/adapters/reject/` is a corrupted manifest that the same validation must refuse
with the diagnostic its `expect` field names (the missing-contract rejection fixtures of PROG-ARC-02).
"""
import json
import os
import re
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
sys.path.insert(0, ROOT)
from checker.effects import AUTHORITIES as AUTHORITY_TYPES, library_summary  # noqa: E402

HOST_FT = os.path.join(ROOT, "library", "fortis", "host.ft")
HOST_RS = os.path.join(ROOT, "compiler", "crates", "fortis-native", "src", "host.rs")
MANIFESTS = os.path.join(ROOT, "compiler", "crates", "fortis-native", "adapters")
REJECT = os.path.join(ROOT, "conformance", "adapters", "reject")
FORTISC = os.path.join(ROOT, "compiler", "target", "release", "fortisc")
FACETS = ("ownership", "effects", "failures", "allocation", "cancellation")
CODE = re.compile(r"`([a-z]+\.[a-z_]+)`")
# operations a capability owns besides its own methods
OWNS = {"FileSystem": ["File.readAll", "File.writeAll", "File.close", "Path.parse"], "SecretAccess": ["reveal"], "Supervisor": ["Scope.detach"]}


def library_operations():
    """Every operation of host.ft with its modifiers, receiver and contract codes: {qualified name: info}, and the
    capability classes (those implementing Authority)."""
    ops, caps, owner, implements, comment = {}, [], None, False, ""
    for line in open(HOST_FT, encoding="utf-8"):
        s = line.strip()
        m = re.match(r"export class (\w+)(?:\[\w+\])?(\s+implements Authority)?", s)
        if m:
            owner, implements = m.group(1), bool(m.group(2))
            if implements:
                caps.append(owner)
            if s.endswith("{}"):
                owner, implements = None, False      # a one-line class declares no members
            continue
        if s == "}":
            owner, implements = None, False
            continue
        if s.startswith("// intrinsic:"):
            comment = s
            continue
        m = re.match(r"export trusted ((?:\w+ )*)func (\w+)(?:\[[^\]]*\])?\(([^)]*)\)", s)
        if m:
            mods = m.group(1).split()
            params = m.group(3)
            receiver = "editable self" if params.startswith("editable self") else ("self" if params.startswith("self") else None)
            name = f"{owner}.{m.group(2)}" if owner else m.group(2)
            # only a `try` operation's contract names its own failures (a non-`try` one may name another's)
            codes = CODE.findall(comment) if "try" in mods else []
            # the Authority types its parameters take (a capability of this module, by name)
            auth_params = sorted({t for t in re.findall(r":\s*(?:owned |editable )?(?:Shared\[)?(\w+)", params) if t in AUTHORITY_TYPES})
            ops[name] = {"module": "fortis.host", "owner": owner, "name": m.group(2), "modifiers": mods, "receiver": receiver, "authority": implements, "codes": codes,
                         "authority_params": auth_params}
            comment = ""
    # Scope.detach lives in the prelude; its summary has no effects and no failures (12.5)
    ops["Scope.detach"] = {"module": "fortis.prelude", "owner": "Scope", "name": "detach", "modifiers": [], "receiver": "self", "authority": False, "codes": []}
    return ops, caps


def derived_effects(info):
    s = library_summary(info.get("module", "fortis.host"), info["owner"], info["name"], info["modifiers"], info["authority"], info["receiver"],
                        info.get("authority_params", ()))
    return sorted(s.effects)


def validate(manifest, cap, ops, host_rs):
    """The diagnostics for one manifest (empty when it holds)."""
    out = []
    if manifest.get("capability") != cap:
        out.append(f"capability: the manifest names {manifest.get('capability')!r}, not {cap}")
    if manifest.get("rule") != "PROG-ARC-02":
        out.append("rule: the manifest must cite PROG-ARC-02")
    handle = manifest.get("handle")
    if not isinstance(handle, dict) or not all(handle.get(k) for k in ("representation", "ownership", "created_by")):
        out.append("handle: representation, ownership and created_by must be declared")
    declared = manifest.get("operations") if isinstance(manifest.get("operations"), dict) else {}
    expected = [n for n, i in ops.items() if i["owner"] == cap] + OWNS.get(cap, [])
    for name in expected:
        if name.split(".")[-1] not in declared and name not in declared:
            out.append(f"missing operation: {name}")
    for name, d in declared.items():
        qual = name if name in ops else f"{cap}.{name}"
        info = ops.get(qual)
        for facet in FACETS:
            if facet not in d:
                out.append(f"missing facet: {facet} ({name})")
            elif facet != "failures" and not d[facet]:
                if not (facet == "effects" and d[facet] == []):
                    out.append(f"empty facet: {facet} ({name})")
        if d.get("kind") == "runtime":
            # a boundary the runtime crosses without a library signature (the supervisor's wait): facets only
            continue
        if info is None:
            out.append(f"unknown operation: {name} is not a library operation of {cap}")
            continue
        if "effects" in d and sorted(d["effects"]) != derived_effects(info):
            out.append(f"effects: {name} declares {sorted(d['effects'])}, the checker derives {derived_effects(info)}")
        if "failures" in d:
            fails = d["failures"]
            if sorted(fails) != sorted(info["codes"]):
                out.append(f"failures: {name} declares {sorted(fails)}, its contract names {sorted(info['codes'])}")
            is_try = "try" in info["modifiers"]
            if is_try and not fails:
                out.append(f"failures: {name} is `try` but declares no failure")
            if fails and not is_try:
                out.append(f"failures: {name} is not `try` but declares {fails}")
            for c in fails:
                if f'"{c}"' not in host_rs:
                    out.append(f"failures: {name} declares {c}, which the native adapter never produces")
        if "cancellation" in d and isinstance(d["cancellation"], str):
            point = d["cancellation"].startswith("a cancellation point")
            if ("async" in info["modifiers"]) != point:
                out.append(f"cancellation: {name} is {'' if 'async' in info['modifiers'] else 'not '}async, so it {'is' if 'async' in info['modifiers'] else 'is not'} a cancellation point")
    v = manifest.get("violation")
    if not isinstance(v, dict) or not all(v.get(k) for k in ("fixture", "refused_by", "expect", "clause")):
        out.append("violation: fixture, refused_by, expect and clause must be declared")
    return out


def refused(v, native_out):
    """Whether the toolchain refuses the violation fixture as declared."""
    path = os.path.join(ROOT, v["fixture"])
    if not os.path.exists(path):
        return False, f"{v['fixture']} does not exist"
    if v["refused_by"] == "static":
        r = subprocess.run([FORTISC, "check", path], capture_output=True, text=True, cwd=ROOT, timeout=600)
        ok = r.returncode != 0 and v["expect"] in (r.stdout + r.stderr)
        return ok, f"fortisc check {'reports' if ok else 'does not report'} {v['expect']}"
    if v["refused_by"] == "native":
        observed = native_out.get(v["fixture"])
        if observed is None:
            return False, "not run natively"
        ok = v["expect"] in observed.split("|")
        return ok, f"the native run prints {observed!r}"
    return False, f"unknown refused_by {v['refused_by']!r}"


def main(argv):
    ops, caps = library_operations()
    host_rs = open(HOST_RS, encoding="utf-8").read()
    rows, problems = [], 0
    manifests = {}
    for cap in caps:
        p = os.path.join(MANIFESTS, f"{cap}.json")
        if not os.path.exists(p):
            rows.append({"capability": cap, "ok": False, "diagnostics": ["missing manifest"]})
            problems += 1
            continue
        manifests[cap] = json.load(open(p, encoding="utf-8"))
    # the native-refused fixtures, built and run once each
    native_out = {}
    natives = sorted({m["violation"]["fixture"] for m in manifests.values() if isinstance(m.get("violation"), dict) and m["violation"].get("refused_by") == "native"})
    if natives and "--no-native" not in argv:
        res = subprocess.run([FORTISC, "native"] + natives, capture_output=True, text=True, cwd=ROOT, timeout=3600).stdout
        cur = None
        for line in res.splitlines():
            if line.startswith("=== "):
                cur = line[4:]
            elif line.startswith("observed: ") and cur:
                native_out[cur] = line[len("observed: "):]
    for cap, m in manifests.items():
        diags = validate(m, cap, ops, host_rs)
        v = m.get("violation") or {}
        refusal = None
        if not diags and (v.get("refused_by") == "static" or "--no-native" not in argv):
            ok, refusal = refused(v, native_out)
            if not ok:
                diags.append(f"violation: {refusal}")
        rows.append({"capability": cap, "ok": not diags, "operations": len(m.get("operations", {})), "diagnostics": diags, "violation": v.get("fixture"), "refusal": refusal})
        problems += bool(diags)
    # missing-contract rejection fixtures
    rejects = []
    for f in sorted(os.listdir(REJECT)) if os.path.isdir(REJECT) else []:
        if not f.endswith(".json"):
            continue
        fx = json.load(open(os.path.join(REJECT, f), encoding="utf-8"))
        diags = validate(fx["manifest"], fx["manifest"].get("capability", "?"), ops, host_rs)
        ok = any(fx["expect"] in d for d in diags)
        rejects.append({"fixture": f, "expect": fx["expect"], "refused": ok, "diagnostics": diags})
        problems += not ok
    for r in rows:
        print(f"{'PASS' if r['ok'] else 'FAIL'} {r['capability']:<13} {r.get('operations', 0):>2} operations" + ("" if r["ok"] else "  " + "; ".join(r["diagnostics"])))
        if r.get("refusal"):
            print(f"     violation {r['violation']}: {r['refusal']}")
    for r in rejects:
        print(f"{'PASS' if r['refused'] else 'FAIL'} reject {r['fixture']}: {'refused' if r['refused'] else 'accepted'} ({r['expect']})")
    print(f"\n{len(rows)} manifests, {sum(r['ok'] for r in rows)} hold; {len(rejects)} rejection fixtures, {sum(r['refused'] for r in rejects)} refused")
    if "--json" in argv:
        out = os.path.join(ROOT, "conformance", "adapters", "results.json")
        os.makedirs(os.path.dirname(out), exist_ok=True)
        json.dump({"rule": "PROG-ARC-02", "manifests": rows, "rejections": rejects}, open(out, "w", encoding="utf-8"), indent=2)
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
