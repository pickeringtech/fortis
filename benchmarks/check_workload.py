"""The frontend check workloads of PROG-NFR-04 (DEL-40): ordinary Fortis modules of a given total size.

`python3 -I benchmarks/check_workload.py --lines 50000 --out build/bench/check-50k`

The output is deterministic: the same arguments always produce the same bytes, whose sha256 manifest the
preregistration records. Each module holds ten units of ordinary code, written the way the examples and the
acceptance application are. A unit has an error type, a refined type, a value class with methods, an enum with an
exhaustive match, a `try` function using `or fail` and `catch`, a loop over a list, a map lookup, a closure with a
capture list, text interpolation, a `pure` function and a test. Names carry the module and unit numbers, so every
module is independent and every one is accepted by the checker. Lines are counted as non-blank lines.
"""
import argparse
import hashlib
import os

UNIT = '''export error Missing{i} {{
  code: "w{m}.missing{i}"
  payload {{ key: Int }}
}}

export type Quantity{i} = Int<0..1000000>

export class Item{i} {{
  export id: Int
  export name: String
  export price: Int
  export pure func total(self, count: Int) Int => Int.wrappingMul(self.price, count)
  export pure func label(self) String => "${{self.name}} #${{self.id}}"
}}

export enum Status{i} {{
  Active
  Paused(reason: String)
  Closed(code: Int)
}}

pure func describe{i}(status: Status{i}) String {{
  return match status {{
    Active => "active"
    Paused(reason) => "paused: ${{reason}}"
    Closed(code) => "closed ${{code}}"
  }}
}}

try func find{i}(items: List[Item{i}], key: Int) Int {{
  for item in items {{
    if item.id == key {{
      return item.price
    }}
  }}
  fail Missing{i}(key: key)
}}

try func priceOf{i}(items: List[Item{i}], key: Int) Int {{
  let found = find{i}(items, key) or fail
  return Int.wrappingAdd(found, 1)
}}

recovers func safePrice{i}(items: List[Item{i}], key: Int) Int {{
  let price = priceOf{i}(items, key) catch e {{
    recover 0
  }}
  return price
}}

func stock{i}(names: List[String]) Map[String, Int] {{
  let mut counts: Map[String, Int] = Map.default()
  let mut n = 0
  for name in names {{
    n = Int.wrappingAdd(n, 1)
    editable counts.set(copy name, n)
  }}
  return move counts
}}

func summary{i}(items: List[Item{i}], factor: Int) Int {{
  let mut sum = 0
  let scaled = items.map(fn[factor](item: Item{i}) Int => Int.wrappingMul(item.price, factor))
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

pure func clampTo{i}(value: Int, limit: Int) Int {{
  if value > limit {{
    return limit
  }}
  return value
}}

test "unit {i} of module {m}" {{
  let items = List.of(Item{i}(id: 1, name: "a", price: 10), Item{i}(id: 2, name: "b", price: 20))
  expect safePrice{i}(items, 2) == 21
  expect safePrice{i}(items, 9) == 0
  expect clampTo{i}(50, 40) == 40
  expect describe{i}(Status{i}.Closed(code: 3)) == "closed 3"
}}
'''


def module(m: int, units: int) -> str:
    return f"// Generated ordinary code for the PROG-NFR-04 check workload: module {m}.\n\n" + "\n".join(UNIT.format(m=m, i=i) for i in range(units))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--lines", type=int, required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--units", type=int, default=10)
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)
    for f in os.listdir(a.out):
        if f.endswith(".ft"):
            os.remove(os.path.join(a.out, f))
    total, m, manifest = 0, 0, []
    while total < a.lines:
        text = module(m, a.units)
        name = f"m{m:05d}.ft"
        open(os.path.join(a.out, name), "w", encoding="utf-8").write(text)
        total += sum(1 for l in text.splitlines() if l.strip())
        manifest.append(f"{hashlib.sha256(text.encode()).hexdigest()}  {name}")
        m += 1
    digest = hashlib.sha256(("\n".join(manifest) + "\n").encode()).hexdigest()
    print(f"{m} modules, {total} non-blank lines, manifest sha256 {digest}")


if __name__ == "__main__":
    main()
