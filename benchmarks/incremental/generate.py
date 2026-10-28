#!/usr/bin/env python3
"""The SP-12 application workload: one package of ordinary modules that import each other.

    python3 -I benchmarks/incremental/generate.py --lines 50000 --out build/incremental/app-50k

The output is deterministic: the same arguments give the same bytes. The package `work.app` has modules in five
layers (`src/l0` to `src/l4`); a module imports from three modules of lower layers and uses their functions and
classes. Each module holds ten groups of the ordinary code of the PROG-NFR-04 check workload (an error type, a
refined type, a value class with methods, an enum and an exhaustive match, `try` functions with `or fail` and
`catch`, a loop, a map, a closure with a capture list, interpolation, a `pure` function and a test), with names that
carry the module and group numbers, plus a linking function per group that calls into an imported module.

Hubs: layer-0 modules export `hub` functions with a known number of callers in other modules (HUBS). A signature
edit of a hub affects exactly its callers, which `manifest.json` lists, with every module's imports and units.
"""
import argparse
import hashlib
import json
import os
import random

LAYERS = 5
# hub name -> number of callers, and how many callers each calling module holds
# (the edited function and its callers are the affected functions: hubA has 100, PROG-NFR-02's bound)
HUBS = [("hubA", 99, 4), ("hubB", 50, 2), ("hubC", 10, 1), ("hubD", 1, 1)]

GROUP = '''export error Missing{g} {{
  code: "w{m}.missing{i}"
  payload {{ key: Int }}
}}

export type Quantity{g} = Int<0..1000000>

export class Item{g}{implements} {{
  export id: Int
  export name: String
  export price: Int
  export pure func total(self, count: Int) Int => Int.wrappingMul(self.price, count)
  export pure func label(self) String => "${{self.name}} #${{self.id}}"{cost}
}}

export enum Status{g} {{
  Active
  Paused(reason: String)
  Closed(code: Int)
}}

pure func describe{g}(status: Status{g}) String {{
  return match status {{
    Active => "active"
    Paused(reason) => "paused: ${{reason}}"
    Closed(code) => "closed ${{code}}"
  }}
}}

try func find{g}(items: List[Item{g}], key: Int) Int {{
  for item in items {{
    if item.id == key {{
      return item.price
    }}
  }}
  fail Missing{g}(key: key)
}}

try func priceOf{g}(items: List[Item{g}], key: Int) Int {{
  let found = find{g}(items, key) or fail
  return Int.wrappingAdd(found, 1)
}}

recovers func safePrice{g}(items: List[Item{g}], key: Int) Int {{
  let price = priceOf{g}(items, key) catch e {{
    recover 0
  }}
  return price
}}

func stock{g}(names: List[String]) Map[String, Int] {{
  let mut counts: Map[String, Int] = Map.default()
  let mut n = 0
  for name in names {{
    n = Int.wrappingAdd(n, 1)
    editable counts.set(copy name, n)
  }}
  return move counts
}}

func summary{g}(items: List[Item{g}], factor: Int) Int {{
  let mut sum = 0
  let scaled = items.map(fn[factor](item: Item{g}) Int => Int.wrappingMul(item.price, factor))
  for value in scaled {{
    sum = Int.wrappingAdd(sum, value)
  }}
  let mut i = 0
  while i < 3 {{
    sum = Int.wrappingAdd(sum, i)
    i = Int.wrappingAdd(i, 1)
  }}
  return sum
}}

func settle{g}(n: Int) Int {{
  let w = Must.create()
  Must.discharge(move w)
  return n
}}

func step{g}(n: Int) Int {{
  return Int.wrappingAdd(n, 1)
}}

noalloc func fast{g}(n: Int) Int {{
  return step{g}(n)
}}

export pure func clampTo{g}(value: Int, limit: Int) Int {{
  if value > limit {{
    return limit
  }}
  return value
}}
{link}
test "group {i} of module {m}" {{
  let items = List.of(Item{g}(id: 1, name: "a", price: 10), Item{g}(id: 2, name: "b", price: 20))
  expect safePrice{g}(items, 2) == 21
  expect safePrice{g}(items, 9) == 0
  expect clampTo{g}(50, 40) == 40
  expect describe{g}(Status{g}.Closed(code: 3)) == "closed 3"
}}
'''

LINK = '''
{aspect}export func link{g}(items: List[Item{g}], other: {q}Item{o}, value: Int) Int {{
  let base = summary{g}(items, 2)
  let extra = other.total(3)
  return Int.wrappingAdd(Int.wrappingAdd(base, extra), {q}clampTo{o}(value, 100))
}}

export func shown{g}(value: Int) String => "v=${{{q}clampTo{o}(value, 9)}}"
'''

# every module exports an interface, a contract and an aspect, which group 0 of a module importing it uses
SHARED = '''export interface Costed{m} {{
  func cost(self) Int
}}

export contract Lean{m} {{
  pure
}}

export aspect checked{m}(limit: Int) {{
  before {{
    if limit > 1000 {{
      discard Int.wrappingAdd(limit, 1)
    }}
  }}
}}

'''

USES = '''
Lean{o} func lean{g}(n: Int) Int {{
  return List.of(n).count()
}}
'''

HUB = '''
export pure func {h}(value: Int<0..1000000>) Int {{
  return Int.wrappingAdd(value, {n})
}}
'''

CALLER = '''
pure func call{h}x{j}(n: Int<0..1000>) Int {{
  return {h}(n)
}}
'''


def gname(m, i):
    return f"{m}u{i}"


def plan(count):
    """The modules' layers and imports, and the hubs' callers, for a package of `count` modules."""
    layer = [m * LAYERS // count for m in range(count)]
    rng = random.Random(1201)
    imports = {}
    for m in range(count):
        lower = [x for x in range(count) if layer[x] < layer[m]]
        imports[m] = sorted(rng.sample(lower, 3)) if len(lower) >= 3 else []
    layer0 = [m for m in range(count) if layer[m] == 0]
    upper = [m for m in range(count) if layer[m] > 0]
    hubs = {}
    for k, (h, n, each) in enumerate(HUBS):
        home = layer0[k % len(layer0)]
        # `each` callers in each of n / each modules (more each in a package too small for that)
        mods = sorted(rng.sample(upper, min(len(upper), -(-n // each))))
        callers = [(mods[j % len(mods)], f"call{h}x{j}") for j in range(n)]
        hubs[h] = {"home": home, "callers": callers}
    return layer, imports, hubs


def module_key(m, layer):
    return f"l{layer[m]}.m{m:05d}"


def alias_of(m):
    return f"m{m:05d}"


def module(m, groups, layer, imports, hubs):
    """Module `m`. Its first and third imports are member imports; the second is an alias import, through which its
    groups name the module's class and function qualified (`m00007.clampTo7u1`)."""
    imported = imports[m]
    names = {}
    aliased = imported[1] if len(imported) > 1 else None
    for i in range(groups):
        if imported:
            o = imported[i % len(imported)]
            if o != aliased:
                names.setdefault(o, set()).update({f"Item{gname(o, i)}", f"clampTo{gname(o, i)}"})
    if imported:
        names.setdefault(imported[0], set()).update({f"Costed{imported[0]}", f"Lean{imported[0]}", f"checked{imported[0]}"})
    mine = [(h, c) for h, info in hubs.items() for (cm, c) in info["callers"] if cm == m]
    for h, info in hubs.items():
        if any(x == h for x, _ in mine):
            names.setdefault(info["home"], set()).add(h)
    head = f"// Generated ordinary code for the SP-12 application workload: module {m}.\n\n"
    for o in sorted(names):
        head += f"import work.app.{module_key(o, layer)} {{ {', '.join(sorted(names[o]))} }}\n"
    if aliased is not None:
        head += f"import work.app.{module_key(aliased, layer)}\n"
    if names:
        head += "\n"
    head += SHARED.format(m=m)
    body = []
    for i in range(groups):
        g = gname(m, i)
        link, implements, cost = "", "", ""
        if imported:
            o = imported[i % len(imported)]
            q = f"{alias_of(o)}." if o == aliased else ""
            aspect = f"@checked{o}(limit: 5)\n" if i == 0 else ""
            link = LINK.format(g=g, o=gname(o, i), q=q, aspect=aspect)
            if i == 0:
                link += USES.format(g=g, o=o)
                implements = f" implements Costed{o}"
                cost = "\n  export func cost(self) Int => self.price"
        body.append(GROUP.format(g=g, m=m, i=i, link=link, implements=implements, cost=cost))
    tail = "".join(HUB.format(h=h, n=n) for (h, n, _) in HUBS if hubs[h]["home"] == m)
    tail += "".join(CALLER.format(h=h, j=c.split("x")[-1]) for h, c in mine)
    return head + "\n".join(body) + tail


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--lines", type=int, required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--groups", type=int, default=10)
    a = ap.parse_args()
    # the fewest modules whose non-blank lines reach the target (a module has about 95 per group)
    count = max(LAYERS * 2, a.lines // (110 * a.groups))
    while True:
        layer, imports, hubs = plan(count)
        texts = [module(m, a.groups, layer, imports, hubs) for m in range(count)]
        if sum(1 for t in texts for x in t.splitlines() if x.strip()) >= a.lines:
            break
        count += 1
    src = os.path.join(a.out, "src")
    if os.path.isdir(src):
        for d, _, fs in os.walk(src):
            for f in fs:
                if f.endswith(".ft"):
                    os.remove(os.path.join(d, f))
    for l in range(LAYERS):
        os.makedirs(os.path.join(src, f"l{l}"), exist_ok=True)
    with open(os.path.join(a.out, "project.yaml"), "w", encoding="utf-8") as f:
        f.write("schema: fortis.project/v1\nname: work.app\nversion: 1.0.0\ndependencies:\n  fortis: \"1.0\"\n")
    files, total = [], 0
    for m, text in enumerate(texts):
        rel = os.path.join("src", f"l{layer[m]}", f"m{m:05d}.ft")
        with open(os.path.join(a.out, rel), "w", encoding="utf-8") as f:
            f.write(text)
        total += sum(1 for x in text.splitlines() if x.strip())
        files.append((rel, hashlib.sha256(text.encode()).hexdigest()))
    manifest = {
        "modules": [{"file": rel, "key": module_key(m, layer), "layer": layer[m], "imports": [module_key(o, layer) for o in imports[m]]} for m, (rel, _) in enumerate(files)],
        "hubs": {h: {"home": os.path.join("src", f"l{layer[i['home']]}", f"m{i['home']:05d}.ft"), "callers": [[os.path.join("src", f"l{layer[cm]}", f"m{cm:05d}.ft"), c] for cm, c in i["callers"]]} for h, i in hubs.items()},
        "lines": total,
    }
    with open(os.path.join(a.out, "manifest.json"), "w", encoding="utf-8") as f:
        json.dump(manifest, f, indent=1, sort_keys=True)
    digest = hashlib.sha256("".join(f"{h}  {r}\n" for r, h in files).encode()).hexdigest()
    print(f"{count} modules, {total} non-blank lines, sources sha256 {digest}")


if __name__ == "__main__":
    main()
