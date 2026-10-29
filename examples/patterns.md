# Patterns that take ownership

The complete program `parcel.ft`, from the `docs/guide/09-classes.md#24` example of the Fortis documentation.

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

Expected output:

```
books: 2500 g
false
```

Review notes:

1. **Ownership in the signature** — `owned Parcel` means `heavy` takes the parcel. The caller gives it up. (lines 8)
2. **Pure, and checked** — `pure`: no effects at all. The checker verifies the claim. (lines 8)
3. **Consume and take apart** — `match move p` consumes the parcel. The pattern binds the field it needs, and `_` discards the label. (lines 9–10)
4. **Destructuring a move** — `let Parcel(…) = move parcel` takes the fields out. `parcel` can't be used after this line. (lines 16)
5. **Fresh values need no marker** — A constructor's result is a fresh value, so it goes straight into `heavy` without `move`. (lines 18)
