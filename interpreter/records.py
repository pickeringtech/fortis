"""Runtime records (PROG-DIA-07): every runtime failure report, observation frame, observation event and
test result exists as a schema-valid record; the human text is rendered from it. `SCHEMAS` is the single
definition; `validate` checks a record structurally (required keys, value types) before it is emitted."""
from __future__ import annotations

import json
import os
from typing import Any, Dict, List, Optional


def shown_path(file: str, package_dir: Optional[str] = None) -> str:
    """How a runtime record names a module's file, as fortisc's `shown_path` does for the VM and native code (D-302):
    a module of a package relative to the package's directory, and a standalone module relative to the current
    directory when it lies below it (the corpus runs from the repository root), else by its file name. Never
    absolute, because a native executable carries it."""
    f = os.path.realpath(file)
    if package_dir is not None:
        return os.path.relpath(f, os.path.realpath(package_dir))
    here = os.path.realpath(os.getcwd())
    return os.path.relpath(f, here) if f.startswith(here + os.sep) else os.path.basename(f)


SCHEMAS: Dict[str, Dict[str, type]] = {
    # a failing `try main` (exactly one), each `report` (exactly one per execution of the statement)
    "runtime-failure": {"kind": str, "code": str, "message": str, "payload": dict, "origin": dict, "cause": list, "annotations": list, "reported_by": str, "rule": str},
    # the observation frame of one activation of a function carrying @observe(Schema) (13)
    "observation-frame": {"kind": str, "schema": str, "function": str, "fields": dict, "outcome": str, "truncated": list, "rule": str},
    # one `record Name(...)` execution (13)
    "observation-event": {"kind": str, "schema": str, "fields": dict, "rule": str},
    # one test or property case (14): status pass | fail | error, evidence from the failing `expect`
    "test-result": {"kind": str, "test": str, "case": str, "status": str, "evidence": dict, "rule": str},
    # a property's or fuzz target's failing input, minimised, with what replays it (Programme 8.2, 8.3; PROG-TST-02)
    "property-failure": {"kind": str, "test": str, "mode": str, "seed": str, "case": str, "original": dict, "minimal": dict, "shrinks": int,
                         "evidence": dict, "replay": str, "rule": str},
    # the end of a test run: the cases that passed and failed (PROG-TST-01)
    "test-summary": {"kind": str, "passed": int, "failed": int, "rule": str},
    # a never-polled future discarded, or a task cancelled before it started (12.3)
    "task-lifecycle": {"kind": str, "function": str, "state": str, "rule": str},
    # a detached task completed: what its supervisor observes (12.6)
    "task-completion": {"kind": str, "task": str, "outcome": str, "rule": str},
    # a defect terminated the main task (10.8): the program exits 3 and no completion value exists
    "defect": {"kind": str, "class": str, "task": str, "rule": str},
    # one `log.info|warn|error(message)` execution (13, LANG-13-04): level info | warn | error, the writing function
    "log": {"kind": str, "level": str, "message": str, "function": str, "rule": str},
    # the reporter's buffer overflowed (PROG-NFR-13): one visible record, the count of dropped records grows
    "buffer-overflow": {"kind": str, "bound": int, "dropped": int, "rule": str},
}


class RecordError(Exception):
    pass


def validate(rec: Dict[str, Any]) -> Dict[str, Any]:
    kind = rec.get("kind")
    schema = SCHEMAS.get(kind)
    if schema is None:
        raise RecordError(f"unknown record kind {kind!r}")
    for k, t in schema.items():
        if k not in rec:
            raise RecordError(f"{kind} record lacks `{k}`")
        if not isinstance(rec[k], t):
            raise RecordError(f"{kind} record field `{k}` is {type(rec[k]).__name__}, expected {t.__name__}")
    extra = set(rec) - set(schema)
    if extra:
        raise RecordError(f"{kind} record has unknown fields {sorted(extra)}")
    return rec


def render(rec: Dict[str, Any]) -> str:
    """The text a person sees, carrying nothing the record does not."""
    k = rec["kind"]
    if k == "runtime-failure":
        where = rec["origin"].get("function", "")
        pay = ", ".join(f"{a}: {b}" for a, b in rec["payload"].items())
        return f"failure {rec['code']} in {where}" + (f" ({pay})" if pay else "") + (f": {rec['message']}" if rec["message"] else "")
    if k == "observation-frame":
        shown = ", ".join(f"{a}={b}" for a, b in rec["fields"].items())
        return f"frame {rec['schema']} of {rec['function']} [{rec['outcome']}]" + (f": {shown}" if shown else "")
    if k == "observation-event":
        shown = ", ".join(f"{a}={b}" for a, b in rec["fields"].items())
        return f"event {rec['schema']}" + (f": {shown}" if shown else "")
    if k == "log":
        return f"{rec['level']}: {rec['message']}"
    if k == "defect":
        return f"defect {rec['class']} terminated task {rec['task']}"
    if k == "buffer-overflow":
        return f"observation buffer overflow: bound {rec['bound']}, {rec['dropped']} record(s) dropped"
    if k == "task-lifecycle":
        return f"task {rec['function']}: {rec['state']}"
    if k == "task-completion":
        return f"task {rec['task']} completed: {rec['outcome']}"
    if k == "test-result":
        case = f" / {rec['case']}" if rec["case"] else ""
        ev = rec["evidence"]
        detail = (f": expected {ev['expected']}, actual {ev['actual']}" + (f" (line {ev['line']})" if "line" in ev else "")) if "actual" in ev else ""
        return f"{rec['status'].upper()} {rec['test']}{case}{detail}"
    if k == "property-failure":
        return (f"  minimal failing input ({rec['mode']} seed {rec['seed']}, {rec['shrinks']} shrinks from {json.dumps(rec['original'])}): "
                f"{json.dumps(rec['minimal'])}\n  replay: {rec['replay']}")
    return str(rec)
