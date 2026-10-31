# Recover, report or pass on

The complete program `half.ft`, from the `docs/guide/10-outcomes.md#4` example of the Fortis documentation.

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

Expected output:

```
0 4
half of 6 is 3
3 has no half
```

Review notes:

1. **Errors are declared** — An error has a stable code and a typed payload. (lines 3–6)
2. **Failing is in the signature** — `try` says `half` can fail, and `fail` is where it does. (lines 8, 10)
3. **Recover in place** — `or recover 0` handles the failure on the spot. (lines 15)
4. **Or match on the outcome** — `match` sees success and failure as two explicit cases. (lines 18–20)
5. **Nothing is lost** — `report f` records the failure before carrying on with a value. (lines 21–22)
6. **Recovery is visible** — `recovers` is transitive: a function that absorbs a failure says so, and so does every function that reaches one, up to `main`. (lines 15, 17, 27)
