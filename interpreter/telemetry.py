"""The Telemetry kit's in-process adapter (Platform 6.3; Language Reference 15; SP-09 design 3.1), as
fortis-runtime's `telemetry.rs` implements it for the VM and native, byte for byte: the privacy policy applied to
captured values before any of them leaves the process, the record's identity and schema version, and the canonical
permitted record written to the producer's channel.

`FORTIS_TELEMETRY` selects the mode: unset, the diagnostic plane withholds every field and nothing is written;
`permit-all`, every field is permitted unchanged on the record stream; otherwise the path of a compiled policy."""
from __future__ import annotations

import hashlib
import hmac
import json
import os
import sys
from typing import Dict, List, Optional, Tuple

WITHHOLD, PERMIT_ALL, POLICY = "withhold", "permit-all", "policy"


class Policy:
    def __init__(self, channel: str, key_id: str, key: bytes, producer: str, fields: Dict[Tuple[str, int, str], Tuple[str, str]]):
        self.key_id, self.key, self.producer, self.fields = key_id, key, producer, fields
        self.seq = 0
        self.channel = open(channel, "ab", buffering=0)

    def canonical(self, schema: str, version: int, function: Optional[str], outcome: Optional[str],
                  fields: List[Tuple[str, str]], truncated: List[str]) -> str:
        kept: Dict[str, str] = {}
        withheld = 0
        for name, text in fields:
            rule = self.fields.get((schema, version, name))
            if rule is None:
                withheld += 1
            elif rule[0] == "permit":
                kept[name] = text
            elif rule[0] == "redact":
                kept[name] = "[redacted]"
            elif rule[0] == "hash":
                kept[name] = keyed_hash(self.key_id, self.key, rule[1], text)
        self.seq += 1
        # that a value was cut to its bound says something about it: only a permitted field's cut is told
        told = [t for t in truncated if self.fields.get((schema, version, t), ("drop",))[0] == "permit"]
        rec = {"fields": kept, "id": {"producer": self.producer, "seq": self.seq},
               "kind": "observation-frame" if function is not None else "observation-event",
               "schema": schema, "truncated": told, "version": version, "withheld": withheld}
        if function is not None:
            rec["function"] = function
        if outcome is not None:
            rec["outcome"] = outcome
        return json.dumps(rec, sort_keys=True) + "\n"


def keyed_hash(key_id: str, key: bytes, type_: str, text: str) -> str:
    msg = b"fortis.telemetry.hash.v1\x00" + type_.encode() + b"\x00" + text.encode("utf-8")
    return f"h1:{key_id}:" + hmac.new(key, msg, hashlib.sha256).hexdigest()[:32]


class PolicyError(Exception):
    pass


def parse(text: str) -> Policy:
    lines = [(i, ln) for i, ln in enumerate(text.splitlines()) if ln.strip() and not ln.lstrip().startswith("#")]
    if not lines or lines[0][1].strip() != "fortis-telemetry-policy 1":
        raise PolicyError("line 1: not a fortis-telemetry-policy 1 file")
    channel = key = producer = None
    fields: Dict[Tuple[str, int, str], Tuple[str, str]] = {}
    for i, ln in lines[1:]:
        ws = ln.split()
        bad = PolicyError(f"line {i + 1}: malformed `{ws[0]}` line")
        try:
            if ws[0] == "channel" and len(ws) == 2:
                channel = ws[1]
            elif ws[0] == "key" and len(ws) == 3 and len(ws[2]) == 64:
                key = (ws[1], bytes.fromhex(ws[2]))
            elif ws[0] == "producer" and len(ws) == 2 and len(ws[1]) == 32:
                bytes.fromhex(ws[1])
                producer = ws[1].lower()
            elif ws[0] == "field" and len(ws) == 6 and ws[4] in ("permit", "redact", "hash", "drop"):
                fields[(ws[1], int(ws[2]), ws[3])] = (ws[4], ws[5])
            else:
                raise bad
        except ValueError:
            raise bad
    if channel is None:
        raise PolicyError("no `channel` line")
    if key is None:
        raise PolicyError("no `key` line")
    try:
        return Policy(channel, key[0], key[1], producer or os.urandom(16).hex(), fields)
    except OSError as e:
        raise PolicyError(f"channel unwritable ({type(e).__name__})")


_mode: Optional[Tuple[str, Optional[Policy]]] = None


def mode() -> Tuple[str, Optional[Policy]]:
    global _mode
    if _mode is None:
        v = os.environ.get("FORTIS_TELEMETRY", "")
        if not v:
            _mode = (WITHHOLD, None)
        elif v == "permit-all":
            _mode = (PERMIT_ALL, None)
        else:
            try:
                with open(v, encoding="utf-8") as f:
                    _mode = (POLICY, parse(f.read()))
            except (OSError, PolicyError) as e:
                why = e if isinstance(e, PolicyError) else "unreadable"
                sys.stderr.write(f"telemetry: policy {v}: {why}; every captured field is withheld\n")
                _mode = (WITHHOLD, None)
    return _mode


def diagnostic_values() -> bool:
    """Whether the diagnostic plane writes captured values: only under `permit-all`."""
    return mode()[0] == PERMIT_ALL


def observe(schema: str, version: int, function: Optional[str], outcome: Optional[str],
            fields: List[Tuple[str, str]], truncated: List[str]) -> None:
    m, p = mode()
    if m == POLICY and p is not None:
        p.channel.write(p.canonical(schema, version, function, outcome, fields, truncated).encode("ascii"))
