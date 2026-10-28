#!/usr/bin/env python3
"""SP-09-telemetry's apparatus check: each mutation injects one known fault into the pipeline or an adapter, the
harness (telemetry_check.py) runs against it, and the mutation is killed only when a check of the family named for
it fails. A harness that passes a mutant would not have measured what it claims. A mutated run whose harness writes
no results, or marks them incomplete, is an apparatus failure, never a kill.

  python3 conformance/tools/telemetry_mutations.py [--json OUT] [harness options...]

Files are patched in place and restored from a copy taken first, whatever happens."""
from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))

# (name, file, text, replacement, the check family that must fail)
MUTATIONS = [
    ("state-without-fsync", "telemetry/pipeline.py", "    disk.fsync(tmp)\n    disk.rename(tmp, path)", "    disk.rename(tmp, path)", "D3"),
    ("state-renames-not-synced", "telemetry/pipeline.py", "    disk.rename(tmp, path)\n    disk.fsync_dir(os.path.dirname(path))", "    disk.rename(tmp, path)", "D3"),
    ("directories-never-synced", "telemetry/pipeline.py", "        disk.makedirs(d)\n    disk.fsync_dir(parent or \"/\")", "        disk.makedirs(d)", "D3"),
    ("existing-directory-not-synced", "telemetry/pipeline.py", "        disk.makedirs(d)\n    disk.fsync_dir(parent or \"/\")", "        disk.makedirs(d)\n        disk.fsync_dir(parent or \"/\")", "D3"),
    ("archive-without-fsync", "telemetry/pipeline.py", "            self.disk.fsync(path)\n            if fresh:", "            if fresh:", "D3"),
    ("new-segment-entry-not-synced", "telemetry/pipeline.py", "            if fresh:\n                self.disk.fsync_dir(self.d)", "            if fresh:\n                pass", "D3"),
    ("torn-tail-kept", "telemetry/pipeline.py", "                self.disk.truncate(self.path(self.open_name), good)\n                self.disk.fsync(self.path(self.open_name))", "                pass", "D3"),
    ("no-deduplication", "telemetry/pipeline.py", "        if self.applied.has(*who):\n            return\n        self.applied.add(*who)\n        self.dirty = True", "        self.applied.add(*who)\n        self.dirty = True", "D2"),
    ("high-water-mark", "telemetry/pipeline.py", "        return s <= self.w.get(p, 0) or s in self.above.get(p, ())", "        return s <= max([self.w.get(p, 0)] + list(self.above.get(p, ())))", "D2"),
    ("end-first-closes", "telemetry/pipeline.py", "            if st is None:\n                self.s[\"lifecycles\"][lk] = [\"ended-first\", None]", "            if st is None:\n                self.s[\"lifecycles\"][lk] = [\"closed\", None]", "U1"),
    ("reuse-reported-as-duplicate", "telemetry/pipeline.py", "                name = \"duplicate_start\" if st[0] == \"open\" else \"reuse\"", "                name = \"duplicate_start\"", "U1"),
    ("overflow-counted-per-metric", "telemetry/pipeline.py", "                slot = self.lifecycle(m, rec, kinds, member, key, vals, slot)", "                self.lifecycle(m, rec, kinds, member, key, vals, slot)", "U1"),
    ("lifecycles-shared-between-metrics", "telemetry/pipeline.py", "        lk = m.name + \"|\" + rec[\"id\"][\"producer\"] + \"|\" + k", "        lk = rec[\"id\"][\"producer\"] + \"|\" + k", "U1"),
    ("limit-ignored", "telemetry/pipeline.py", "        if len(self.series) < self.pr.limit:", "        if True:", "C1"),
    ("missing-sample-takes-no-series", "telemetry/pipeline.py", "            if self.per_record:", "            if self.per_record and all(rec[\"fields\"].get(m.value) is not None for m in self.pr.metrics if m.instrument == \"histogram\"):", "L1"),
    ("drop-not-final", "telemetry/pipeline.py", "        if dropped.has(*who):\n            return \"dropped\"\n", "", "A1"),
    ("coverage-always-complete", "telemetry/pipeline.py", "                    info[\"coverage\"] = \"since_activation\"", "                    info[\"coverage\"] = \"complete\"", "E1"),
    ("no-catch-up", "telemetry/pipeline.py", "            if self.archive is not None:\n                for ln in read_archive(self.disk, self.archive.d):", "            if False:\n                for ln in read_archive(self.disk, self.archive.d):", "E1"),
    ("privacy-written-before-checks", "telemetry/pipeline.py", "        return dict(had, **now), diags", "        _atomic(self.disk, os.path.join(self.state, \"privacy.json\"), dict(had, **now))\n        return dict(had, **now), diags", "E1"),
    ("version-actions-ignored", "telemetry/plan.py", "        for key in ((schema, version), (schema, None)):\n            a = self.transforms", "        for key in ((schema, None),):\n            a = self.transforms", "E1"),
    ("replay-not-accepted", "telemetry/pipeline.py", "            self.accepted.add(*identity_of(rec))            # what reached this destination, for later coverage\n", "", "D4"),
    ("privacy-change-accepted", "telemetry/pipeline.py", "        diags = privacy_diags + self.check_epochs()", "        diags = self.check_epochs()", "E1"),
    ("huge-integer-converted", "telemetry/pipeline.py", "            return len(v) <= 21 and INTEGER.fullmatch(v)", "            return INTEGER.fullmatch(v)", "P4"),
    ("integer-range-unchecked", "telemetry/pipeline.py", "            return len(v) <= 21 and INTEGER.fullmatch(v) is not None and BOUNDS[b][0] <= int(v) <= BOUNDS[b][1]", "            return len(v) <= 21 and INTEGER.fullmatch(v) is not None", "P4"),
    ("quarantine-keeps-content", "telemetry/pipeline.py", "                    self.ledger[\"quarantine\"][code] = self.ledger[\"quarantine\"].get(code, 0) + 1",
     "                    self.ledger[\"quarantine\"][code] = self.ledger[\"quarantine\"].get(code, 0) + 1\n                    self.ledger.setdefault(\"samples\", []).append(ln.decode(errors=\"replace\"))", "P1"),
    ("transient-raw-write", "telemetry/pipeline.py", "                who = rec\n", "                who = rec\n                self.disk.write_new(os.path.join(self.state, \"rejected.tmp\"), ln)\n                self.disk.remove(os.path.join(self.state, \"rejected.tmp\"))\n", "P1"),
    ("rejection-printed", "telemetry/pipeline.py", "                who = rec\n", "                who = rec\n                print(ln)\n", "P1"),
    ("diagnostic-plane-leaks", "interpreter/telemetry.py", "    return mode()[0] == PERMIT_ALL", "    return True", "P1"),
    ("redact-leaks", "interpreter/telemetry.py", "                kept[name] = \"[redacted]\"", "                kept[name] = text", "P1"),
    ("adapter-writes-values-to-stderr", "interpreter/telemetry.py", "    if m == POLICY and p is not None:\n", "    if m == POLICY and p is not None:\n        sys.stderr.write(repr(fields) + \"\\n\")\n", "P1"),
    ("diagnostic-cut-told-under-policy", "interpreter/eval.py", "\"truncated\": truncated if values else []", "\"truncated\": truncated", "P2"),
    # added after the closing review (post-review run only)
    ("duplicate-keys-accepted", "telemetry/pipeline.py", "        rec = json.loads(line, object_pairs_hook=_unique)", "        rec = json.loads(line)", "P4"),
    ("recovery-not-settled", "telemetry/pipeline.py", "    if disk.read(path) is not None:\n        disk.fsync(path)", "    if False:\n        disk.fsync(path)", "D3"),
    ("ancestors-not-synced", "telemetry/pipeline.py", "    _mkdir(disk, parent)\n    if not disk.is_dir(d):", "    if not disk.is_dir(parent):\n        _mkdir(disk, parent)\n    if not disk.is_dir(d):", "D3"),
    ("no-drain-on-start", "telemetry/pipeline.py", "            if self.archive is not None:\n                for ln in read_archive(self.disk, self.archive.d):\n                    ep.apply(json.loads(ln))\n                ep.commit()\n            if self.history",
     "            if self.archive is not None and self.history[\"active\"].get(name) != pr.epoch:\n                for ln in read_archive(self.disk, self.archive.d):\n                    ep.apply(json.loads(ln))\n                ep.commit()\n            if self.history", "D3"),
    ("decimals-as-floats", "telemetry/yamlite.py", "        return Node(Decimal(t), line)", "        return Node(float(t), line)", "L1"),
    ("allocated-sorted-by-value", "telemetry/pipeline.py", "\"allocated\": [json.loads(k) for k in sorted(self.series)]", "\"allocated\": sorted(json.loads(k) for k in self.series)", "L1"),
    ("transformed-cut-told", "interpreter/telemetry.py", "        told = [t for t in truncated if self.fields.get((schema, version, t), (\"drop\",))[0] == \"permit\"]", "        told = list(truncated)", "P2"),
]


def main(argv):
    out_json = None
    if "--json" in argv:
        i = argv.index("--json")
        out_json = argv[i + 1]
        argv = argv[:i] + argv[i + 2:]
    rows = []
    for name, rel, old, new, family in MUTATIONS:
        path = os.path.join(ROOT, rel)
        original = open(path, encoding="utf-8").read()
        if original.count(old) != 1:
            rows.append({"mutation": name, "applied": False, "killed": False})
            print(f"NOT APPLIED {name}: the text to mutate is not found exactly once", flush=True)
            continue
        backup = tempfile.NamedTemporaryFile(delete=False, suffix=".bak")
        backup.write(original.encode())
        backup.close()
        tmp = tempfile.mktemp(suffix=".json")
        try:
            with open(path, "w", encoding="utf-8") as f:
                f.write(original.replace(old, new))
            subprocess.run([sys.executable, os.path.join(HERE, "telemetry_check.py"), "--json", tmp] + argv, cwd=ROOT,
                           capture_output=True, text=True, timeout=3600)
            summary = json.load(open(tmp)) if os.path.exists(tmp) else None
            results = summary["results"] if summary and summary.get("complete") else None
        finally:
            shutil.copyfile(backup.name, path)
            os.unlink(backup.name)
            if os.path.exists(tmp):
                os.unlink(tmp)
        if results is None:
            # no results, or results the harness itself marks incomplete: an apparatus failure, never a kill (an
            # exception inside a section is that family's failed check `<FAMILY>.exception`, which does count)
            results_row = {"mutation": name, "applied": True, "family": family, "killed": False, "apparatus_failure": True, "failed_checks": []}
            print(f"APPARATUS FAILURE {name}: the harness produced no complete results", flush=True)
        else:
            failed = [r["check"] for r in results if not r["pass"]]
            killed = any(c.startswith(family + ".") for c in failed)
            results_row = {"mutation": name, "applied": True, "family": family, "killed": killed, "failed_checks": failed}
            print(f"{'KILLED' if killed else 'SURVIVED'} {name} ({family}): {', '.join(failed[:6])}", flush=True)
        rows.append(results_row)
    summary = {"mutations": len(rows), "expected": len(MUTATIONS), "killed": sum(r["killed"] for r in rows),
               "apparatus_failures": sum(1 for r in rows if r.get("apparatus_failure") or not r.get("applied")), "results": rows}
    if out_json:
        with open(out_json, "w", encoding="utf-8") as f:
            json.dump(summary, f, indent=1, sort_keys=True)
    print(f"{summary['killed']} of {summary['mutations']} mutations killed")
    return 0 if summary["killed"] == summary["mutations"] == len(MUTATIONS) else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
