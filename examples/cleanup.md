# Cleanup without a collector

The complete program `resource.ft`, from the `docs/guide/07-ownership.md#17` example of the Fortis documentation.

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

Expected output:

```
working
release 2
release 1
done
```

Review notes:

1. **Cleanup belongs to the type** — `cleanup` runs exactly once, when a `Resource` is destroyed. (lines 6–8)
2. **Counted, explicit sharing** — `Shared[Console]` is reference-counted. `retain()` takes another reference, and says so. (lines 5, 12–13)
3. **Destroyed in reverse order** — When `work` returns, `second` is destroyed before `first`, so the output reads `release 2` then `release 1`. No garbage collector decides when. (lines 12–13, 15)
4. **Owned, then shared** — `main` owns the console it declares, and moves it into a `Shared` handle for the code it calls. (lines 17–18)
5. **Lent, not given away** — `work(console)` borrows the handle. `main` still owns it afterwards, and prints `done`. (lines 19–20)
