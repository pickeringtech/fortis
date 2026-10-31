"""Effect summaries (DEL-19: Language Reference 11.1–11.4, 10.4/10.5 for `recovers`).

A `Summary` holds the eight effects a body may have, each labelled by its provenance (`proved` from
the body's own statements, `trusted` when imported from a trusted library signature), and the
write footprint: the function's own `editable` parameters (and `editable self`) it writes through.
`library_summary` derives a trusted callee's summary from its modifiers, from its receiver and parameters (a
capability's method, or an operation that takes an Authority, performs I/O: 11.6), and from the effect data its
standard package ships, reference/library/effects/<package>.json (D-112): the one source, read by both checkers,
for the effects a signature cannot show (`alloc`, `block`, `io`, `mutate`, `nondet`)."""
from __future__ import annotations

import json
import os
from dataclasses import dataclass, field
from functools import cache
from typing import Dict, List, Optional, Set, Tuple

EFFECTS = ("alloc", "io", "mutate", "nondet", "suspend", "block", "fail", "recovers")
PURE_FORBIDS = ("io", "mutate", "nondet", "suspend", "block")

ROOT = os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
EFFECT_DATA = os.path.join(ROOT, "reference", "library", "effects")
COMPATIBILITY = os.path.join(ROOT, "library", "fortis", "compatibility.yaml")
# the effects a standard package's data may give an operation, in the order a summary adds them; `fail`, `suspend` and
# `recovers` change a function's type, so only its modifiers give them (11.2)
DECLARABLE = ("nondet", "block", "mutate", "alloc", "io")

# a modifier and the effect it permits (11.2): `forbids` of that effect beside the modifier is a contradiction (11.5)
PERMITS = (("async", "suspend"), ("try", "fail"), ("recovers", "recovers"))


def contract_contradictions(parts, skip_own: bool) -> List[Tuple[str, str, str, str, str]]:
    """The contradictions among a declaration's claims (11.5, E-EFF-050), shared by the contract check and the function
    check. `parts` lists (origin, items): an origin is a contract's name, or "" for a function's written modifiers;
    items are a contract's `modifiers`, `forbids`, `requires` and `excludes`. Each contradiction is (kind, first origin,
    second origin, claim, effect): `pure` beside a required capability ("pure", the capability, "io"), a modifier beside
    `forbids` of the effect it permits ("forbids", the modifier, the effect), and one authority both required and
    excluded ("excludes", the capability, "io"). With `skip_own`, a pair whose two sides are one contract's is left out,
    since that contract's own check reports it. Each is listed once, the written modifiers' side first."""
    out, seen = [], set()
    for oa, a in parts:
        for ob, b in parts:
            if skip_own and oa and oa == ob:
                continue
            found = []
            if "pure" in a.get("modifiers", ()):
                found += [("pure", cap, "io") for cap in sorted(b.get("requires", ()))]
            found += [("forbids", m, e) for m, e in PERMITS if m in a.get("modifiers", ()) and e in b.get("forbids", ())]
            found += [("excludes", cap, "io") for cap in sorted(set(a.get("requires", ())) & set(b.get("excludes", ())))]
            for kind, claim, effect in found:
                if (kind, ob, claim) not in seen:
                    seen.add((kind, ob, claim))
                    out.append((kind, oa, ob, claim, effect))
    return out


def standard_packages(path: str = COMPATIBILITY) -> List[str]:
    """The packages of the published compatibility set (PLAT-01-01)."""
    out, inside = [], False
    for line in open(path, encoding="utf-8"):
        if line.startswith("packages:"):
            inside = True
            continue
        if inside and line.startswith("  ") and ":" in line:
            out.append(line.strip().split(":", 1)[0].strip().strip('"'))
        elif inside and line.strip() and not line.startswith(" "):
            inside = False
    return out


def owning_package(module: str, packages: List[str]) -> Optional[str]:
    """The standard package a module belongs to: the longest package name that is the module or prefixes it."""
    hits = [p for p in packages if module == p or module.startswith(p + ".")]
    return max(hits, key=len) if hits else None


def load_effect_data(directory: str = EFFECT_DATA, packages: Optional[List[str]] = None) -> Dict[Tuple[str, Optional[str], str], Tuple[str, ...]]:
    """Every standard package's effect data (D-112): {(module, owner or None, name): effects}. One file per package,
    `<package>.json`, holding rows for that package's modules only; anything malformed raises."""
    packages = standard_packages() if packages is None else packages
    rows: Dict[Tuple[str, Optional[str], str], Tuple[str, ...]] = {}
    for f in sorted(os.listdir(directory)):
        if not f.endswith(".json"):
            continue
        d = json.load(open(os.path.join(directory, f), encoding="utf-8"))
        where = f"effect data {f}"
        if not isinstance(d, dict) or d.get("schema") != "fortis.effects/v1" or set(d) - {"schema", "package", "about", "operations"}:
            raise ValueError(f"{where}: not a fortis.effects/v1 document")
        pkg = d.get("package")
        if pkg not in packages or f != f"{pkg}.json":
            raise ValueError(f"{where}: names package {pkg!r}, which is not a standard package of the compatibility set or not the file's name")
        if not isinstance(d.get("operations"), list):
            raise ValueError(f"{where}: no operations list")
        for r in d["operations"]:
            if not isinstance(r, dict) or set(r) - {"module", "owner", "name", "effects", "why"} or not {"module", "owner", "name", "effects"} <= set(r):
                raise ValueError(f"{where}: a row needs module, owner, name and effects (and may give why): {r!r}")
            mod, owner, nm, effs = r["module"], r["owner"], r["name"], r["effects"]
            if not isinstance(mod, str) or owning_package(mod, packages) != pkg:
                raise ValueError(f"{where}: module {mod!r} is not one of package {pkg}'s")
            if not (owner is None or isinstance(owner, str) and owner) or not isinstance(nm, str) or not nm or not isinstance(r.get("why", ""), str):
                raise ValueError(f"{where}: malformed owner, name or why in {r!r}")
            if not isinstance(effs, list) or not effs or len(set(effs)) != len(effs) or any(e not in DECLARABLE for e in effs):
                raise ValueError(f"{where}: {mod} {owner}.{nm}: effects must be a non-empty list of distinct {', '.join(DECLARABLE)}")
            key = (mod, owner, nm)
            if key in rows:
                raise ValueError(f"{where}: {mod} {owner}.{nm} is listed twice")
            rows[key] = tuple(e for e in DECLARABLE if e in effs)
    return rows


@cache
def effect_data():
    """Load trusted-operation metadata only when a library operation needs it.

    Summary algebra and in-memory checker units need no library files. As in the Rust checker,
    one analysis process shares the same validated metadata after its first library lookup.
    """
    return load_effect_data()


AUTHORITIES = {"FileSystem", "Network", "Clock", "Random", "Environment", "Process", "Supervisor", "Console", "SecretAccess", "RawSql"}


@dataclass
class Summary:
    effects: Dict[str, str] = field(default_factory=dict)     # effect -> "proved" | "trusted"
    footprint: Set[str] = field(default_factory=set)          # own editable parameters written through
    sites: Dict[str, object] = field(default_factory=dict)    # effect -> first token that introduced it
    authorities: Set[str] = field(default_factory=set)        # authority types the function requires (11.5, 11.6)
    needs_auth: bool = False                                  # io reached through a capability (not `report`): needs an authority parameter (11.6)
    # effects that are not certain (PROG-DIA-02, X-132): "possible" when only a callback's type permits the effect,
    # "unknown" when it comes through a foreign `trusted` function that does not claim its absence
    uncertain: Dict[str, str] = field(default_factory=dict)

    def add(self, effect: str, label: str = "proved", tok=None):
        self.uncertain.pop(effect, None)
        self._add(effect, label, tok)

    def _add(self, effect: str, label: str, tok):
        if effect not in self.effects or (self.effects[effect] == "trusted" and label == "proved"):
            self.effects[effect] = label
        if tok is not None and effect not in self.sites:
            self.sites[effect] = tok

    def add_uncertain(self, effect: str, label: str, kind: str, tok=None):
        """An effect that is not certain; a certain occurrence of the same effect, before or after, wins."""
        certain = effect in self.effects and effect not in self.uncertain
        self._add(effect, label, tok)
        if not certain:
            if kind == "possible" or effect not in self.uncertain:
                self.uncertain[effect] = kind

    def status(self, effect: str) -> str:
        """The status of a claim excluding `effect` (Programme 7.1): violated, unproved or unknown."""
        k = self.uncertain.get(effect)
        return "violated" if k is None else ("unproved" if k == "possible" else "unknown")

    def imports(self, other: "Summary", tok=None, skip: Set[str] = frozenset()):
        for e, lab in other.effects.items():
            if e not in skip:
                if e in other.uncertain:
                    self.add_uncertain(e, lab, other.uncertain[e], tok)
                else:
                    self.add(e, lab, tok)
        self.authorities |= other.authorities
        self.needs_auth = self.needs_auth or other.needs_auth

    def show(self) -> str:
        return "{" + ", ".join(f"{e}{'' if lab == 'proved' else ' (trusted)'}" for e, lab in sorted(self.effects.items())) + "}" + (f" footprint {sorted(self.footprint)}" if self.footprint else "")


def summaries_text(summaries) -> str:
    """Every function's summary in one stable text form (DEL-33: compared byte for byte with the production frontend)."""
    out = []
    for label in sorted(summaries):
        s = summaries[label]
        out.append(f"{label}: {s.show()} authorities {sorted(s.authorities)} needs_auth {s.needs_auth}")
    return "\n".join(out) + ("\n" if out else "")


def library_summary(module: Optional[str], owner: Optional[str], name: str, modifiers, implements_authority: bool, receiver: Optional[str],
                    authority_params=()) -> Summary:
    """The summary of a trusted (bodiless) library signature: `name` of `owner` (None for a free function) declared in
    `module`, with the Authority types its parameters take."""
    s = Summary()
    mods = set(modifiers)
    if "try" in mods: s.add("fail", "trusted")
    if "async" in mods: s.add("suspend", "trusted")
    if "recovers" in mods: s.add("recovers", "trusted")
    for e in effect_data().get((module, owner, name), ()) if module is not None else ():
        s.add(e, "trusted")
        if e == "io":
            s.needs_auth = True
    # a method of a capability or authority, or an operation that takes one, performs I/O through it (11.6; D-112)
    if "pure" not in mods and ((implements_authority and receiver is not None) or authority_params):
        s.add("io", "trusted")
        s.needs_auth = True
        s.authorities |= set(authority_params)
    if receiver == "editable self" and "pure" not in mods:
        s.footprint.add("self")
    return s
