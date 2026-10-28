"""DEL-38 criterion 3 (PROG-NFR-14 on the VM): a synchronous program starts no task thread; only a launch does.

`fortisc vm --probe` reports, for each program, the OS threads the VM started for tasks (exact) and the peak
thread count a watcher sampled from /proc/self/task while the program ran (a task's thread may be too short-lived
to sample, so the exact count decides), with the baseline and the launches. The two programs are interpreter/probe_threads.py's: a synchronous one,
and one that launches a single pool task.

  python3 -I conformance/tools/vm_probe.py"""
from __future__ import annotations

import os
import re
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
FORTISC = os.path.join(ROOT, "compiler", "target", "release", "fortisc")
sys.path.insert(0, ROOT)

from interpreter.probe_threads import POOL, SYNC  # noqa: E402


def probe(src: str):
    d = tempfile.mkdtemp(prefix="fortis-vm-probe-")
    path = os.path.join(d, "probe.ft")
    open(path, "w", encoding="utf-8").write(src)
    out = subprocess.run([FORTISC, "vm", "--probe", path], capture_output=True, text=True, timeout=600).stdout
    m = re.search(r"probe: baseline (\d+) peak (\d+) launches (\d+) threads (\d+)", out)
    observed = re.search(r"^observed: (.*)$", out, re.M)
    return (int(m.group(1)), int(m.group(2)), int(m.group(3)), int(m.group(4))) if m else None, observed.group(1) if observed else None


def main():
    (sb, sp, sl, st), so = probe(SYNC)
    (pb, pp, pl, pt), po = probe(POOL)
    print(f"synchronous program: {st} task threads started; sampled peak {sp} (baseline {sb}); launches {sl}; output {so!r}")
    print(f"pool program: {pt} task threads started; sampled peak {pp} (baseline {pb}); launches {pl}; output {po!r}")
    ok = st == 0 and sp == sb and sl == 0 and so == "sync" and pl == 1 and pt == 1 and po == "pool 1"
    print("vm probe: " + ("OK (no task thread without a launch)" if ok else "FAILED"))
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
