"""Diagnostic records (Delivery Programme 7.1, 7.3, 7.4; PROG-DIA-01/07/08).

A record is a dict with the common fields and a `fields` object carrying its class's mandatory
fields; the text rendering is derived from the record and carries nothing the record does not.
`validate` implements reference/schemas/diagnostic.schema.json by hand (the conformance tools
have no third-party dependencies); the schema file is the published contract.
"""
from __future__ import annotations

import json
import sys
from typing import Dict, List, Optional

# Programme 7.1, by identity: (class name, code prefixes, mandatory fields, correction kind)
CLASSES: List[dict] = [
    {"name": "Type", "prefixes": ["E-TYP", "E-MIX-001"], "fields": ["expected_type", "actual_type", "primary_span"], "correction": "behaviour-preserving or behaviour-changing fix"},
    {"name": "Ownership", "prefixes": ["E-OWN"], "fields": ["place", "conflict", "spans", "loan_region"], "correction": "fix (marker, block restructuring, copy)"},
    {"name": "Obligation", "prefixes": ["E-OBL", "E-ERR-001"], "fields": ["obligation_origin", "unconsumed_path"], "correction": "fix (or fail, or recover, report, match)"},
    {"name": "Effect", "prefixes": ["E-EFF"], "fields": ["required_contract", "violating_effect", "witness_path"], "correction": "contract-weakening or behaviour-changing fix"},
    {"name": "Refinement unproved", "prefixes": ["E-REF"], "fields": ["required_range", "proved_interval", "widening_point"], "correction": "explicit checked alternative"},
    {"name": "Unsupported shape", "prefixes": ["E-OWN-090"], "fields": ["lowering_row"], "correction": None},
    {"name": "Resource limit", "prefixes": ["E-LIM"], "fields": ["budget", "operation"], "correction": None},
    {"name": "Internal defect", "prefixes": ["E-INT"], "fields": ["reproduction"], "correction": None},
    {"name": "Syntax", "prefixes": ["E-SYN"], "fields": ["expected_tokens", "found_token"], "correction": "fix where the grammar allows one"},
    {"name": "Names and modules", "prefixes": ["E-NAM"], "fields": ["name", "candidates"], "correction": "fix (import, rename)"},
    {"name": "Patterns", "prefixes": ["E-MAT"], "fields": ["missing_variants", "arm_set"], "correction": "fix (add arms)"},
    {"name": "Closures", "prefixes": ["E-FUN"], "fields": ["violation", "capture_span", "use_span"], "correction": "fix (capture entry, stronger kind)"},
    {"name": "Defer and async", "prefixes": ["E-DEF", "E-ASY"], "fields": ["offending_statement", "rule_reference"], "correction": "fix (move statement out of defer, add await site)"},
    {"name": "Observation", "prefixes": ["E-OBS"], "fields": ["frame_field", "exit_path"], "correction": "fix (add or remove capture)"},
    {"name": "Dependency injection", "prefixes": ["E-DI"], "fields": ["consumer", "slot", "candidates"], "correction": "fix (target, replaces, lifetime)"},
    {"name": "Telemetry", "prefixes": ["E-TEL"], "fields": ["plan_line", "projection", "field"], "correction": "fix (bind_at, a new metric name, a new schema version)"},
]
COMMON = ["class", "code", "message", "rule", "file", "line", "column", "severity", "status", "fields"]
SEVERITIES = {"error", "warning"}
STATUSES = {"violated", "unproved", "unknown"}   # PROG-DIA-02: a proved violation, an inability to prove, an unknown trust boundary


def class_of(code: str) -> Optional[dict]:
    exact = [c for c in CLASSES if code in c["prefixes"]]
    if exact:
        return exact[0]
    for c in CLASSES:
        if any(code.startswith(p + "-") for p in c["prefixes"] if p.count("-") == 1):
            return c
    return None


def make(code: str, message: str, rule: str, file: str, line: int, column: int, fields: Dict, *,
         severity: str = "error", status: str = "violated", correction: Optional[dict] = None) -> dict:
    cls = class_of(code)
    rec = {"class": cls["name"] if cls else None, "code": code, "message": message, "rule": rule,
           "file": file, "line": line, "column": column, "severity": severity, "status": status, "fields": fields}
    if correction is not None:
        rec["correction"] = correction
    return rec


def validate(rec: dict) -> List[str]:
    """Errors against the schema: common fields, class by identity, class mandatory fields, correction shape."""
    errs = []
    for f in COMMON:
        if f not in rec:
            errs.append(f"missing {f}")
    if errs:
        return errs
    cls = next((c for c in CLASSES if c["name"] == rec["class"]), None)
    if cls is None:
        errs.append(f"unknown class {rec['class']!r}")
        return errs
    if class_of(rec["code"]) is not cls:
        errs.append(f"code {rec['code']} does not belong to class {cls['name']}")
    for f in cls["fields"]:
        if f not in rec["fields"]:
            errs.append(f"class {cls['name']} requires field {f}")
    if rec["severity"] not in SEVERITIES:
        errs.append("severity must be error or warning")
    if rec["status"] not in STATUSES:
        errs.append("status must be violated, unproved or unknown")
    if not isinstance(rec["line"], int) or not isinstance(rec["column"], int) or rec["line"] < 1 or rec["column"] < 1:
        errs.append("line and column are 1-based integers")
    if "correction" in rec:
        if cls["correction"] is None:
            errs.append(f"class {cls['name']} defines no correction")
        c = rec["correction"]
        if not isinstance(c, dict) or "edits" not in c or not c["edits"]:
            errs.append("correction needs a non-empty edits list")
        else:
            for e in c["edits"]:
                for f in ("file", "line", "column", "end_line", "end_column", "replacement"):
                    if f not in e:
                        errs.append(f"edit lacks {f}")
    return errs


def render_text(rec: dict) -> str:
    head = f"{rec['file']}:{rec['line']}:{rec['column']}: {rec['severity']} {rec['code']} [{rec['rule']}] ({rec['class']}, {rec['status']}): {rec['message']}"
    lines = [head]
    # field values and replacements as JSON, so the text carries each value exactly (the production renderer,
    # fortis_diag::Record::render_text, writes the same text; PROG-DIA-05)
    for k, v in rec["fields"].items():
        lines.append(f"    {k}: {json.dumps(v, ensure_ascii=False)}")
    if rec.get("correction"):
        lines.append(f"    correction: {rec['correction'].get('kind', '')}".rstrip())
        for e in rec["correction"]["edits"]:
            lines.append(f"      {e['file']}:{e['line']}:{e['column']}-{e['end_line']}:{e['end_column']} -> {json.dumps(e['replacement'], ensure_ascii=False)}")
    return "\n".join(lines)


def to_json_line(rec: dict) -> str:
    return json.dumps(rec, sort_keys=True, ensure_ascii=False)


def emit(records: List[dict], stream=None, fmt: Optional[str] = None) -> None:
    """Text on a terminal, JSON lines on a pipe, `fmt` overrides (Programme 7.4)."""
    stream = stream or sys.stdout
    if fmt is None:
        fmt = "text" if stream.isatty() else "json"
    for r in records:
        stream.write((render_text(r) if fmt == "text" else to_json_line(r)) + "\n")
