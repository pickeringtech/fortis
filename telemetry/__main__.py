"""The reference pipeline's command line (SP-09):

  python3 -m telemetry registry REGISTRY.json PROGRAM.ft...        register each program's observation schemas
  python3 -m telemetry check REGISTRY.json PLAN.yaml               validate a plan (diagnostics as JSON lines)
  python3 -m telemetry compile REGISTRY.json PLAN.yaml KEY-ID KEY-HEX CHANNEL [PRODUCER]
  python3 -m telemetry ingest STATE REGISTRY.json PLAN.yaml BATCH CHANNEL...   deliver channels in batches; export
  python3 -m telemetry serve STATE REGISTRY.json PLAN.yaml          batches as JSON lines on stdin, `ack N` on stdout
  python3 -m telemetry replay ARCHIVE DEST REGISTRY.json PLAN.yaml [--live]
  python3 -m telemetry export STATE REGISTRY.json PLAN.yaml
"""
from __future__ import annotations

import json
import os
import sys

from telemetry import plan as planmod
from telemetry.pipeline import ArchiveCorrupt, Pipeline, export_text
from telemetry.registry import Registry, RegistryError


def _registry(path: str) -> Registry:
    return Registry.load(path) if os.path.exists(path) else Registry()


def main(argv) -> int:
    if not argv:
        sys.stderr.write(__doc__)
        return 2
    cmd, a = argv[0], argv[1:]
    try:
        if cmd == "registry":
            r = _registry(a[0])
            for program in a[1:]:                          # each file a single-module program of its own
                for ident in r.add_program([program]):
                    print(ident)
            r.save(a[0])
        elif cmd == "check":
            planmod.load(open(a[1], encoding="utf-8").read(), _registry(a[0]))
        elif cmd == "compile":
            r = _registry(a[0])
            p = planmod.load(open(a[1], encoding="utf-8").read(), r)
            sys.stdout.write(planmod.compile_policy(p, r, a[2], a[3], a[4], a[5] if len(a) > 5 else None))
        elif cmd == "ingest":
            pipe = Pipeline(a[0], _registry(a[1]), open(a[2], encoding="utf-8").read())
            size = int(a[3])
            lines = [ln for ch in a[4:] for ln in open(ch, "rb").read().splitlines()]
            for i in range(0, len(lines), size):
                pipe.process(lines[i:i + size])
            sys.stdout.write(export_text(pipe.export()))
        elif cmd == "serve":
            pipe = Pipeline(a[0], _registry(a[1]), open(a[2], encoding="utf-8").read())
            for raw in sys.stdin:
                b = json.loads(raw)
                if pipe.process([ln.encode() for ln in b["records"]]):
                    sys.stdout.write(f"ack {b['batch']}\n")
                    sys.stdout.flush()
        elif cmd == "replay":
            text = open(a[3], encoding="utf-8").read()
            live = "--live" in a
            pipe = Pipeline(a[1], _registry(a[2]), text, archive_override=None if live else False)
            pipe.replay(a[0])
            sys.stdout.write(export_text(pipe.export()))
        elif cmd == "export":
            sys.stdout.write(export_text(Pipeline(a[0], _registry(a[1]), open(a[2], encoding="utf-8").read()).export()))
        else:
            sys.stderr.write(__doc__)
            return 2
    except planmod.PlanInvalid as e:
        for d in e.diags:
            print(json.dumps(d.to_json(), sort_keys=True))
        return 1
    except ArchiveCorrupt as e:
        sys.stderr.write(f"replay refused: {e}\n")
        return 1
    except RegistryError as e:
        sys.stderr.write(f"registry: {e}\n")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
