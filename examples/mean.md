# Failures you can see

The complete program `mean.ft`, from the `README.md` example of the Fortis documentation.

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

Expected output:

```
mean of 4 readings: 25
```

Review notes:

1. **Checked purity** — `pure` rules out effects such as I/O and clock access. The checker verifies it. (lines 7)
2. **Declared failure** — `try` declares that `mean` can fail. Its caller must handle the outcome. (lines 7)
3. **Bounded input** — `Int<0..1000>` carries a range. `ListView` gives read-only access. (lines 7)
4. **Visible propagation** — `total + r` is checked: an overflow is a failure, and `or fail` passes it to the caller. (lines 14)
5. **Changes declared once** — `let mut` says that `readings` will change, so the `push` that changes it needs no other marker. (lines 20–21)
6. **Caller’s recovery** — `catch` handles the failure, reports it and recovers with `0`. (lines 22–24)
7. **Authority as a parameter** — `main` declares the console it needs, and the runtime supplies it. Printing uses that capability. (lines 19, 26)
