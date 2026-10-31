# Fortis: AI-written code people can review

Fortis is a statically checked, compiled language for software written mostly by AI agents. **People can read, review and trust it because every decision is written in the source.** Ownership, changes, failures and effects are explicit.

Latest: Model a domain (v0.1.0). Language 1.0 is still being built and is not released.

## Examples

### Failures you can see (`mean.ft`)

Source: `README.md`. Expected output: `mean of 4 readings: 25`.

```fortis
import fortis.host { Console }

export error Empty {
  code: "demo.empty"
}

pure try func mean(readings: ListView[Int<0..1000>]) Int {
  let n = readings.count()
  if n <= 0 {
    fail Empty()
  }
  let mut total = 0
  for r in readings {
    total = total + r or fail
  }
  return total / n
}

func main(console: Console) {
  let mut readings: List[Int<0..1000>] = List.of(12, 30, 18)
  readings.push(40)
  let m = mean(readings) catch e {
    report e
    recover 0
  }
  console.print("mean of ${readings.count()} readings: ${m}")
}
```

Notes:

1. **Checked purity** — `pure` rules out effects such as I/O and clock access. The checker verifies it. (lines 7)
2. **Declared failure** — `try` declares that `mean` can fail. Its caller must handle the outcome. (lines 7)
3. **Bounded input** — `Int<0..1000>` carries a range. `ListView` gives read-only access. (lines 7)
4. **Visible propagation** — `total + r` is checked: an overflow is a failure, and `or fail` passes it to the caller. (lines 14)
5. **Changes declared once** — `let mut` says that `readings` will change, so the `push` that changes it needs no other marker. (lines 20–21)
6. **Caller’s recovery** — `catch` handles the failure, reports it and recovers with `0`. (lines 22–24)
7. **Authority as a parameter** — `main` declares the console it needs, and the runtime supplies it. Printing uses that capability. (lines 19, 26)

### Cleanup without a collector (`resource.ft`)

Source: `docs/guide/07-ownership.md#17`. Expected output: `working
release 2
release 1
done`.

```fortis
import fortis.host { Console }

class Resource {
  export id: Int
  export console: Shared[Console]
  cleanup(editable self) {
    self.console.print("release ${self.id}")
  }
}

func work(console: Shared[Console]) {
  let first = Resource(id: 1, console: console.retain())
  let second = Resource(id: 2, console: console.retain())
  console.print("working")
}

func main(out: Console) {
  let console = Shared.create(move out)
  work(console)
  console.print("done")
}
```

Notes:

1. **Cleanup belongs to the type** — `cleanup` runs exactly once, when a `Resource` is destroyed. (lines 6–8)
2. **Counted, explicit sharing** — `Shared[Console]` is reference-counted. `retain()` takes another reference, and says so. (lines 5, 12–13)
3. **Destroyed in reverse order** — When `work` returns, `second` is destroyed before `first`, so the output reads `release 2` then `release 1`. No garbage collector decides when. (lines 12–13, 15)
4. **Owned, then shared** — `main` owns the console it declares, and moves it into a `Shared` handle for the code it calls. (lines 17–18)
5. **Lent, not given away** — `work(console)` borrows the handle. `main` still owns it afterwards, and prints `done`. (lines 19–20)

### Patterns that take ownership (`parcel.ft`)

Source: `docs/guide/09-classes.md#28`. Expected output: `books: 2500 g
false`.

```fortis
import fortis.host { Console }

class Parcel {
  export label: String
  export grams: Int
}

pure func heavy(p: owned Parcel) Bool {
  return match move p {
    Parcel(label: _, grams: g) => g > 1000
  }
}

func main(console: Console) {
  let parcel = Parcel(label: "books", grams: 2500)
  let Parcel(label: label, grams: grams) = move parcel
  console.print("${label}: ${grams} g")
  console.print("${heavy(Parcel(label: "feather", grams: 1))}")
}
```

Notes:

1. **Ownership in the signature** — `owned Parcel` means `heavy` takes the parcel. The caller gives it up. (lines 8)
2. **Pure, and checked** — `pure`: no effects at all. The checker verifies the claim. (lines 8)
3. **Consume and take apart** — `match move p` consumes the parcel. The pattern binds the field it needs, and `_` discards the label. (lines 9–10)
4. **Destructuring a move** — `let Parcel(…) = move parcel` takes the fields out. `parcel` can't be used after this line. (lines 16)
5. **Fresh values need no marker** — A constructor's result is a fresh value, so it goes straight into `heavy` without `move`. (lines 18)

### Recover, report or pass on (`half.ft`)

Source: `docs/guide/10-outcomes.md#4`. Expected output: `0 4
half of 6 is 3
3 has no half`.

```fortis
import fortis.host { Console }

export error NotEven {
  code: "demo.not_even"
  payload { value: Int }
}

pure try func half(n: Int) Int {
  if n % 2 != 0 {
    fail NotEven(value: n)
  }
  return n / 2
}

pure recovers func halfOrZero(n: Int) Int => half(n) or recover 0

recovers func describe(n: Int) String {
  return match half(n) {
    success(v) => "half of ${n} is ${v}"
    failure(f) => {
      report f
      give "${n} has no half"
    }
  }
}

recovers func main(console: Console) {
  console.print("${halfOrZero(7)} ${halfOrZero(8)}")
  console.print(describe(6))
  console.print(describe(3))
}
```

Notes:

1. **Errors are declared** — An error has a stable code and a typed payload. (lines 3–6)
2. **Failing is in the signature** — `try` says `half` can fail, and `fail` is where it does. (lines 8, 10)
3. **Recover in place** — `or recover 0` handles the failure on the spot. (lines 15)
4. **Or match on the outcome** — `match` sees success and failure as two explicit cases. (lines 18–20)
5. **Nothing is lost** — `report f` records the failure before carrying on with a value. (lines 21–22)
6. **Recovery is visible** — `recovers` is transitive: a function that absorbs a failure says so, and so does every function that reaches one, up to `main`. (lines 15, 17, 27)

## Try it

From a checkout of the repository, with Python 3 and a Rust toolchain:

```sh
cargo build --release --manifest-path compiler/Cargo.toml
bin/fortis check mean.ft
bin/fortis run mean.ft
```

The same program runs on the definitional interpreter (`bin/fortis run`), the development VM (`compiler/target/release/fortisc vm`) and native code (`bin/fortis build`), and all three must agree.

## Roadmap

- **v0.0.1**: write, check, run, build, format and test programs. Modules, functions, classes, enums, ownership and failures.
- **v0.0.2**: code that says only what matters. Fewer markers, capabilities as parameters and built-in logging.
- **v0.1.0 (latest)**: model a domain. Refinements, newtypes, conversions, interfaces, generics, contracts, text and collections.
- **v0.2**: trust effects and test properly. Checked effects and capabilities, and property tests.
- **v0.3**: build a web service. An HTTP server with health, readiness and metrics built in, SQLite and typed queries.
- **v0.4**: build web apps. HTML, forms, sessions, authentication, Postgres and migrations.
- **v0.5**: run it in production. Telemetry, connection pools, timeouts, rate limits and containers.
- **v0.6**: organise and extend code. Packages with dependencies, checked architecture and extensions.
- **v0.7–0.9**: async in your own code, a complete standard library, and tooling for large code bases.
- **v1.0**: stability. Editions, migration and a stability promise. Performance judged against Rust.

## Licence

Developed by Pickering Technologies Ltd. Released under the MIT License.
