"""The telemetry plan (Platform 6.4; SP-09 design 2, 3.6, 3.7, 4): read from YAML, validated against the schema
registry with every error at its plan line, and compiled into the adapter's policy.

A plan never reaches the checker: it cannot change whether source type-checks (PLAT-06-01)."""
from __future__ import annotations

import hashlib
import json
import re
from decimal import Decimal, InvalidOperation
from typing import Dict, List, Optional, Tuple

from telemetry import yamlite
from telemetry.registry import FieldInfo, Registry

ACTIONS = ("permit", "redact", "hash", "drop")
INSTRUMENTS = ("counter", "histogram", "updown")
OUTCOMES = ("success", "failure", "cancelled", "defect", "not_started")
UNITS = {"1", "s", "ms", "us", "ns", "By", "KiBy", "MiBy", "%"}
# Platform 6.1: a plan MUST NOT add a required field, choose capture timing or reference object fields
FORBIDDEN = {"required": "adds a required field", "capture": "chooses capture timing", "capture_at": "chooses capture timing",
             "timing": "chooses capture timing", "object": "names an object field", "object_field": "names an object field"}
OPS = ("equals", "in", "not_in", "range", "present")


class Diag:
    def __init__(self, code: str, line: int, message: str, correction: Optional[dict] = None):
        self.code, self.line, self.message = code, line, message
        # the class's correction (Programme 7.1): edits of the plan's text, in the form fortisc records carry
        self.correction = correction

    def to_json(self) -> dict:
        d = {"code": self.code, "line": self.line, "message": self.message}
        if self.correction:
            d["correction"] = self.correction
        return d

    def __repr__(self):
        return f"{self.code} at line {self.line}: {self.message}"


class PlanInvalid(Exception):
    def __init__(self, diags: List[Diag]):
        super().__init__("; ".join(map(repr, diags)))
        self.diags = diags


class Pred:
    """A predicate over a permitted record: ("all"|"any", [Pred]), ("not", Pred) or ("field", name, op, value)."""

    def __init__(self, kind: str, *args):
        self.kind, self.args = kind, args

    def fields(self) -> List[str]:
        if self.kind == "field":
            return [self.args[0]]
        if self.kind == "not":
            return self.args[0].fields()
        return [f for p in self.args[0] for f in p.fields()]

    def plain(self):
        if self.kind == "field":
            return {"field": self.args[0], self.args[1]: self.args[2]}
        if self.kind == "not":
            return {"not": self.args[0].plain()}
        return {self.kind: [p.plain() for p in self.args[0]]}

    def holds(self, rec: dict, kinds: Dict[str, str]) -> bool:
        if self.kind == "all":
            return all(p.holds(rec, kinds) for p in self.args[0])
        if self.kind == "any":
            return any(p.holds(rec, kinds) for p in self.args[0])
        if self.kind == "not":
            return not self.args[0].holds(rec, kinds)
        name, op, value = self.args
        text = rec.get("outcome") if name == "outcome" else rec["fields"].get(name)
        if op == "present":
            return (text is not None) == value
        if text is None:
            return False                                   # a comparison on an absent field is false (design 3.6)
        kind = "enum" if name == "outcome" else kinds.get(name, "opaque")
        if op == "equals":
            return _eq(text, value, kind)
        if op == "in":
            return any(_eq(text, v, kind) for v in value)
        if op == "not_in":
            return not any(_eq(text, v, kind) for v in value)
        lo, hi = value
        return Decimal(str(lo)) <= Decimal(text) <= Decimal(str(hi))


def _eq(text: str, value, kind: str) -> bool:
    if kind == "numeric":
        return Decimal(text) == Decimal(str(value))
    if kind == "bool":
        return text == ("true" if value else "false")
    return text == value


class Metric:
    def __init__(self, name: str, instrument: str, line: int):
        self.name, self.instrument, self.line = name, instrument, line
        self.value: Optional[str] = None
        self.unit: Optional[str] = None
        self.buckets: List[Decimal] = []
        self.key: Optional[str] = None
        self.start: Optional[Pred] = None
        self.end: Optional[Pred] = None


class Projection:
    def __init__(self, name: str, line: int):
        self.name, self.line = name, line
        self.schema = ""
        self.pinned: Optional[int] = None
        self.latest = 0
        self.admitted: List[int] = []
        self.where: Optional[Pred] = None
        self.dimensions: List[str] = []
        self.metrics: List[Metric] = []
        self.limit = 1000
        self.export = ""
        self.bind_at: Optional[str] = None
        self.definition: dict = {}
        self.epoch = ""


class Plan:
    def __init__(self):
        self.permitted_fields = "all"
        # (schema, version or None for every version) -> field -> action; a version's own entry wins
        self.transforms: Dict[Tuple[str, Optional[int]], Dict[str, str]] = {}
        self.archive = {"enabled": True, "segment_records": 64, "capacity_records": None, "on_full": "block"}
        self.projections: Dict[str, Projection] = {}
        self.metric_lines: Dict[str, int] = {}
        self.transform_lines: Dict[Tuple[str, Optional[int], str], int] = {}
        self.permitted_line = 1

    def privacy_line(self, schema: str, field: str, version: Optional[int] = None) -> int:
        """Where the plan decides a field's action: its version's transform, its schema's, else `permitted_fields`."""
        for key in ((schema, version, field), (schema, None, field)):
            if key in self.transform_lines:
                return self.transform_lines[key]
        return self.permitted_line

    def action(self, schema: str, field: str, version: Optional[int] = None) -> str:
        for key in ((schema, version), (schema, None)):
            a = self.transforms.get(key, {}).get(field)
            if a is not None:
                return a
        return "permit" if self.permitted_fields == "all" else "drop"


def load(text: str, registry: Registry) -> Plan:
    """The plan, or PlanInvalid with every diagnostic at its line."""
    try:
        root = yamlite.loads(text)
    except yamlite.PlanSyntax as e:
        raise PlanInvalid([Diag("E-TEL-004", e.line, f"not a telemetry plan: {e}")])
    v = _Validator(registry)
    v.lines = text.split("\n")
    plan = v.plan(root)
    if v.diags:
        raise PlanInvalid(sorted(v.diags, key=lambda d: (d.line, d.code)))
    return plan


class _Validator:
    def __init__(self, registry: Registry):
        self.lines: List[str] = []
        self.r = registry
        self.diags: List[Diag] = []

    def err(self, code: str, line: int, message: str, correction: Optional[dict] = None):
        self.diags.append(Diag(code, line, message, correction))

    def insert_after(self, line: int, text: str, kind: str) -> dict:
        """A correction inserting `text` as a line after `line`, indented as `line` is."""
        src = self.lines[line - 1] if 0 < line <= len(self.lines) else ""
        indent = src[: len(src) - len(src.lstrip(" "))]
        return {"edits": [{"file": "", "line": line + 1, "column": 1, "end_line": line + 1, "end_column": 1, "replacement": f"{indent}{text}\n"}], "kind": kind}

    def entries(self, node: yamlite.Node, what: str) -> Tuple[str, ...]:
        """The keys of a mapping whose keys are names (projections, schemas, fields); `mapping` reports another shape."""
        return tuple(node.value) if isinstance(node.value, dict) else ()

    def mapping(self, node: yamlite.Node, allowed: Tuple[str, ...], what: str) -> Dict[str, yamlite.Node]:
        if not isinstance(node.value, dict):
            self.err("E-TEL-004", node.line, f"{what} must be a mapping")
            return {}
        for k in node.value:
            if k in FORBIDDEN:
                self.err("E-TEL-004", node.key_line(k), f"`{k}` {FORBIDDEN[k]}: a plan may not (Platform 6.1)")
            elif k not in allowed:
                self.err("E-TEL-004", node.key_line(k), f"`{k}` is not a key of {what}")
        return {k: v for k, v in node.value.items() if k in allowed}

    def plan(self, root: yamlite.Node) -> Plan:
        p = Plan()
        self._plan_actions = p                             # privacy is read first: field uses depend on it
        top = self.mapping(root, ("schema", "telemetry"), "the plan")
        if "telemetry" not in top:
            self.err("E-TEL-004", root.line, "the plan has no `telemetry` mapping")
            return p
        t = self.mapping(top["telemetry"], ("privacy", "archive", "projections"), "`telemetry`")
        if "privacy" in t:
            self.privacy(t["privacy"], p)
        if "archive" in t:
            self.archive(t["archive"], p)
        if "projections" in t:
            for name, node in self.mapping(t["projections"], self.entries(t["projections"], "`projections`"), "`projections`").items():
                pr = self.projection(name, t["projections"].key_line(name), node, p)
                if pr is not None:
                    p.projections[name] = pr
        return p

    def privacy(self, node: yamlite.Node, p: Plan):
        m = self.mapping(node, ("permitted_fields", "transforms"), "`privacy`")
        p.permitted_line = node.key_line("permitted_fields") if "permitted_fields" in m else node.line
        if "permitted_fields" in m:
            if m["permitted_fields"].value not in ("all", "none"):
                self.err("E-TEL-004", m["permitted_fields"].line, "`permitted_fields` is `all` or `none`")
            else:
                p.permitted_fields = m["permitted_fields"].value
        if "transforms" in m:
            tm = m["transforms"]
            for target, fnode in self.mapping(tm, self.entries(tm, "`transforms`"), "`transforms`").items():
                # `Schema` applies to every version; `Schema.vN` to that version only, ahead of the schema's entry
                mt = re.fullmatch(r"([A-Za-z_][A-Za-z0-9_]*)(?:\.v([0-9]+))?", target)
                schema, version = (mt.group(1), int(mt.group(2)) if mt.group(2) else None) if mt else (target, None)
                if mt is None or self.r.latest(schema) is None:
                    self.err("E-TEL-004", tm.key_line(target), f"a transform names `{target}`, which no registered program declares")
                    continue
                if version is not None and version not in self.r.versions(schema):
                    self.err("E-TEL-004", tm.key_line(target), f"a transform names `{target}`, a version not registered (versions: {self.r.versions(schema)})")
                    continue
                declared = {f for v in ([version] if version is not None else self.r.versions(schema)) for f in self.r.fields(schema, v)}
                for field, anode in self.mapping(fnode, self.entries(fnode, f"the transforms of `{schema}`"), f"the transforms of `{schema}`").items():
                    if field not in declared:
                        self.err("E-TEL-004", fnode.key_line(field), f"a transform names `{target}.{field}`, which `{target}` does not declare")
                    elif anode.value not in ACTIONS:
                        self.err("E-TEL-004", anode.line, f"`{anode.value}` is not a transform (one of {', '.join(ACTIONS)})")
                    else:
                        p.transforms.setdefault((schema, version), {})[field] = anode.value
                        p.transform_lines[(schema, version, field)] = fnode.key_line(field)

    def archive(self, node: yamlite.Node, p: Plan):
        m = self.mapping(node, ("enabled", "segment_records", "capacity_records", "on_full"), "`archive`")
        if "enabled" in m:
            if not isinstance(m["enabled"].value, bool):
                self.err("E-TEL-004", m["enabled"].line, "`enabled` is true or false")
            else:
                p.archive["enabled"] = m["enabled"].value
        for k in ("segment_records", "capacity_records"):
            if k in m:
                x = m[k].value
                if x is None and k == "capacity_records":
                    continue
                if not isinstance(x, int) or isinstance(x, bool) or x < 1:
                    self.err("E-TEL-004", m[k].line, f"`{k}` is a positive integer")
                else:
                    p.archive[k] = x
        if "on_full" in m:
            if m["on_full"].value not in ("block", "drop"):
                self.err("E-TEL-004", m["on_full"].line, "`on_full` is `block` or `drop`")
            else:
                p.archive["on_full"] = m["on_full"].value

    def projection(self, name: str, line: int, node: yamlite.Node, p: Plan) -> Optional[Projection]:
        pr = Projection(name, line)
        m = self.mapping(node, ("source", "bind_at", "where", "dimensions", "metrics", "cardinality", "export"), f"projection `{name}`")
        before = len(self.diags)
        src = m.get("source")
        mt = re.fullmatch(r"([A-Za-z_][A-Za-z0-9_]*)(?:\.v([0-9]+))?", str(src.value)) if src is not None else None
        if mt is None:
            self.err("E-TEL-004", src.line if src else line, f"projection `{name}` needs `source: Schema` or `Schema.vN`")
            return None
        pr.schema = mt.group(1)
        pr.source_line = src.line
        if self.r.latest(pr.schema) is None:
            self.err("E-TEL-002", src.line, f"`{pr.schema}` is not declared by any registered program")
            return None
        if mt.group(2) is not None:
            pr.pinned = int(mt.group(2))
            if pr.pinned not in self.r.versions(pr.schema):
                self.err("E-TEL-002", src.line, f"`{pr.schema}.v{pr.pinned}` is not registered (versions: {self.r.versions(pr.schema)})")
                return None
        pr.latest = pr.pinned if pr.pinned is not None else self.r.latest(pr.schema)
        fields = self.r.fields(pr.schema, pr.latest)
        if "bind_at" in m:
            if m["bind_at"].value != "lifecycle_start":
                self.err("E-TEL-004", m["bind_at"].line, "`bind_at` is `lifecycle_start`")
            else:
                pr.bind_at = "lifecycle_start"
        if "where" in m:
            pr.where = self.pred(m["where"], pr, fields)
        if "dimensions" in m:
            dn = m["dimensions"]
            if not isinstance(dn.value, list):
                self.err("E-TEL-004", dn.line, "`dimensions` is a list of fields")
            else:
                for d in dn.value:
                    if self.field_use(d.value, d.line, pr, fields, "dimension"):
                        pr.dimensions.append(d.value)
        if "metrics" not in m or not isinstance(m["metrics"].value, list) or not m["metrics"].value:
            self.err("E-TEL-004", m["metrics"].line if "metrics" in m else line, f"projection `{name}` needs a list of `metrics`")
        else:
            for mn in m["metrics"].value:
                met = self.metric(mn, pr, fields, p)
                if met is not None:
                    pr.metrics.append(met)
        if "cardinality" in m:
            cm = self.mapping(m["cardinality"], ("limit", "overflow"), "`cardinality`")
            if "limit" in cm:
                x = cm["limit"].value
                if not isinstance(x, int) or isinstance(x, bool) or x < 1:
                    self.err("E-TEL-004", cm["limit"].line, "`limit` is a positive integer")
                else:
                    pr.limit = x
            if "overflow" in cm and cm["overflow"].value != "aggregate":
                self.err("E-TEL-004", cm["overflow"].line, "`overflow` is `aggregate`")
        if "export" in m:
            pr.export = str(m["export"].value)
        if len(self.diags) > before:
            return None
        used = sorted({f for f in self.used(pr) if f != "outcome"})
        if pr.pinned is not None:
            pr.admitted = [pr.pinned]
        else:
            # every version in which each used field exists with the latest version's type and action (design 3.6)
            pr.admitted = [v for v in self.r.versions(pr.schema)
                           if all(f in self.r.fields(pr.schema, v) and self.r.fields(pr.schema, v)[f].type == fields[f].type
                                  and p.action(pr.schema, f, v) == p.action(pr.schema, f, pr.latest) for f in used)]
        pr.definition = {
            "source": {"schema": pr.schema, "pinned": pr.pinned, "fields": {f: [fields[f].type, p.action(pr.schema, f, pr.latest)] for f in used}},
            "bind_at": pr.bind_at,
            "where": pr.where.plain() if pr.where else None,
            "dimensions": pr.dimensions,
            "metrics": [_metric_plain(x) for x in pr.metrics],
            "cardinality": {"limit": pr.limit, "overflow": "aggregate"},
        }
        pr.epoch = hashlib.sha256(json.dumps(pr.definition, sort_keys=True, default=str).encode()).hexdigest()[:16]
        return pr

    @staticmethod
    def used(pr: Projection) -> List[str]:
        out = list(pr.dimensions) + (pr.where.fields() if pr.where else [])
        for x in pr.metrics:
            out += [f for f in (x.value, x.key) if f] + (x.start.fields() if x.start else []) + (x.end.fields() if x.end else [])
        return out

    def field_use(self, field, line: int, pr: Projection, fields: Dict[str, FieldInfo], use: str, op: Optional[str] = None) -> bool:
        """Whether `field` may be used as `use` (a dimension, a predicate operand with `op`, a value or a key)."""
        if field == "outcome" and use in ("dimension", "predicate"):
            return True
        if not isinstance(field, str) or field not in fields:
            self.err("E-TEL-002", line, f"`{pr.schema}.v{pr.latest}` declares no field `{field}`")
            return False
        action = self._plan_actions.action(pr.schema, field, pr.latest)
        allowed = {"permit": True, "hash": use == "key" or op == "present", "redact": op == "present", "drop": False}[action]
        if not allowed:
            self.err("E-TEL-005", line, f"`{field}` is {action}{'ped' if action == 'drop' else 'ed'} by the privacy policy, so it cannot be used as a {use if op is None else op + ' operand'} (design 3.6)")
            return False
        return True

    def pred(self, node: yamlite.Node, pr: Projection, fields: Dict[str, FieldInfo]) -> Optional[Pred]:
        if not isinstance(node.value, dict):
            self.err("E-TEL-004", node.line, "a predicate is a mapping")
            return None
        keys = set(node.value)
        for comb in ("all", "any"):
            if comb in keys:
                if keys != {comb} or not isinstance(node.value[comb].value, list):
                    self.err("E-TEL-004", node.line, f"`{comb}` takes a list of predicates and nothing else")
                    return None
                subs = [self.pred(x, pr, fields) for x in node.value[comb].value]
                return None if None in subs else Pred(comb, subs)
        if "not" in keys:
            if keys != {"not"}:
                self.err("E-TEL-004", node.line, "`not` takes one predicate and nothing else")
                return None
            sub = self.pred(node.value["not"], pr, fields)
            return None if sub is None else Pred("not", sub)
        ops = [k for k in keys if k in OPS]
        bad = keys - set(OPS) - {"field"}
        for k in bad:
            self.err("E-TEL-004", node.key_line(k), f"`{k}` {FORBIDDEN[k]}: a plan may not (Platform 6.1)" if k in FORBIDDEN else f"`{k}` is not a predicate key")
        if "field" not in keys or len(ops) != 1 or bad:
            if not bad:
                self.err("E-TEL-004", node.line, "a predicate is `field` with one of " + ", ".join(OPS))
            return None
        fname, op, vnode = node.value["field"].value, ops[0], node.value[ops[0]]
        if not self.field_use(fname, node.value["field"].line, pr, fields, "predicate", op):
            return None
        kind = "outcome" if fname == "outcome" else fields[fname].kind()
        value = vnode.plain()
        if op == "present":
            if not isinstance(value, bool):
                self.err("E-TEL-001", vnode.line, "`present` is true or false")
                return None
        elif op == "range":
            if kind != "numeric":
                self.err("E-TEL-001", vnode.line, f"`range` needs a numeric field; `{fname}` is {fields[fname].type if fname in fields else 'an outcome'}")
                return None
            if not (isinstance(value, list) and len(value) == 2 and all(_num(x) for x in value) and Decimal(str(value[0])) <= Decimal(str(value[1]))):
                self.err("E-TEL-001", vnode.line, "`range` is [low, high], two numbers with low <= high")
                return None
        else:
            values = value if op in ("in", "not_in") else [value]
            if op in ("in", "not_in") and not isinstance(value, list):
                self.err("E-TEL-001", vnode.line, f"`{op}` takes a list")
                return None
            for x in values:
                why = self.type_mismatch(x, kind, fname, fields)
                if why:
                    self.err("E-TEL-001", vnode.line, why)
                    return None
        return Pred("field", fname, op, value)

    @staticmethod
    def type_mismatch(x, kind: str, fname: str, fields: Dict[str, FieldInfo]) -> Optional[str]:
        if kind == "outcome":
            return None if x in OUTCOMES else f"`{x}` is not an outcome class ({', '.join(OUTCOMES)})"
        if kind == "enum":
            vs = fields[fname].variants
            return None if isinstance(x, str) and x in vs else f"`{x}` is not a variant of `{fields[fname].base()}` ({', '.join(vs)})"
        if kind == "numeric":
            return None if _num(x) else f"`{x}` is not a number, and `{fname}` is {fields[fname].type}"
        if kind == "string":
            return None if isinstance(x, str) else f"`{x}` is not a string, and `{fname}` is {fields[fname].type}"
        if kind == "bool":
            return None if isinstance(x, bool) else f"`{x}` is not true or false, and `{fname}` is Bool"
        return f"`{fname}` ({fields[fname].type}) can only be tested with `present`"

    def metric(self, node: yamlite.Node, pr: Projection, fields: Dict[str, FieldInfo], p: Plan) -> Optional[Metric]:
        m = self.mapping(node, ("name", "instrument", "value", "unit", "buckets", "lifecycle", "description"), "a metric")
        if "name" not in m or not isinstance(m["name"].value, str) or "instrument" not in m:
            self.err("E-TEL-004", node.line, "a metric needs `name` and `instrument`")
            return None
        if m["instrument"].value not in INSTRUMENTS:
            self.err("E-TEL-004", m["instrument"].line, f"`{m['instrument'].value}` is not an instrument ({', '.join(INSTRUMENTS)})")
            return None
        met = Metric(m["name"].value, m["instrument"].value, m["name"].line)
        options = {"counter": (), "histogram": ("value", "unit", "buckets"), "updown": ("lifecycle",)}[met.instrument]
        for k in ("value", "unit", "buckets", "lifecycle"):
            if k in m and k not in options:
                self.err("E-TEL-004", node.key_line(k), f"a {met.instrument} takes no `{k}`")
                return None
        if met.name in p.metric_lines:
            self.err("E-TEL-006", met.line, f"metric `{met.name}` is already defined at line {p.metric_lines[met.name]}")
            return None
        p.metric_lines[met.name] = met.line
        if met.instrument == "histogram":
            v = m.get("value")
            if v is None:
                self.err("E-TEL-004", node.line, "a histogram needs `value`")
                return None
            if not self.field_use(v.value, v.line, pr, fields, "value"):
                return None
            if fields[v.value].kind() != "numeric":
                self.err("E-TEL-001", v.line, f"a histogram's value must be numeric; `{v.value}` is {fields[v.value].type}")
                return None
            met.value = v.value
            if "unit" in m:
                u = m["unit"].value
                if not (isinstance(u, str) and (u in UNITS or re.fullmatch(r"\{[a-z_]+\}", u))):
                    self.err("E-TEL-001", m["unit"].line, f"`{u}` is not a unit (one of {', '.join(sorted(UNITS))}, or an annotation such as {{cent}})")
                    return None
                met.unit = u
            bk = m["buckets"].plain() if "buckets" in m else [10, 100, 1000]
            if not (isinstance(bk, list) and bk and all(_num(b) for b in bk) and [Decimal(str(b)) for b in bk] == sorted({Decimal(str(b)) for b in bk})):
                self.err("E-TEL-004", m["buckets"].line if "buckets" in m else node.line, "`buckets` is a list of increasing numbers")
                return None
            met.buckets = [Decimal(str(b)) for b in bk]
        elif met.instrument == "updown":
            if pr.bind_at != "lifecycle_start":
                fix = self.insert_after(pr.source_line, "bind_at: lifecycle_start", "bind the projection at lifecycle start") if pr.bind_at is None and getattr(pr, "source_line", None) else None
                self.err("E-TEL-003", met.line, f"up-down metric `{met.name}` needs its projection bound at lifecycle start (`bind_at: lifecycle_start`, Platform 6.4)", fix)
                return None
            lc = m.get("lifecycle")
            lm = self.mapping(lc, ("key", "start", "end"), "`lifecycle`") if lc is not None else {}
            if set(lm) != {"key", "start", "end"}:
                self.err("E-TEL-003", node.key_line("lifecycle") if lc is not None else met.line, f"up-down metric `{met.name}` needs `lifecycle` with `key`, `start` and `end`")
                return None
            if not self.field_use(lm["key"].value, lm["key"].line, pr, fields, "key"):
                return None
            met.key = lm["key"].value
            met.start = self.pred(lm["start"], pr, fields)
            met.end = self.pred(lm["end"], pr, fields)
            if met.start is None or met.end is None:
                return None
        return met


def _num(x) -> bool:
    if isinstance(x, bool) or not isinstance(x, (int, float, str, Decimal)):
        return False
    try:
        Decimal(str(x))
        return not isinstance(x, str)
    except InvalidOperation:
        return False


def _metric_plain(m: Metric) -> dict:
    return {"name": m.name, "instrument": m.instrument, "value": m.value, "unit": m.unit, "buckets": [str(b) for b in m.buckets],
            "key": m.key, "start": m.start.plain() if m.start else None, "end": m.end.plain() if m.end else None}


def compile_policy(plan: Plan, registry: Registry, key_id: str, key_hex: str, channel: str, producer: Optional[str] = None) -> str:
    """The adapter's policy (design 2): the action for every field of every registered version."""
    lines = ["fortis-telemetry-policy 1", f"channel {channel}", f"key {key_id} {key_hex}"]
    if producer:
        lines.append(f"producer {producer}")
    for name in sorted(registry.schemas):
        for v in registry.versions(name):
            for f, info in sorted(registry.fields(name, v).items()):
                lines.append(f"field {name} {v} {f} {plan.action(name, f, v)} {info.type}")
    return "\n".join(lines) + "\n"
