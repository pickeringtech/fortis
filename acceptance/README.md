# Acceptance applications

Programme 14's applications, each a package with a specification that `acceptance/run.py` checks.

| Application | Directory | Backend | Chunk |
|---|---|---|---|
| A — Core library (a stock ledger) | `a/` | interpreter, VM, native | DEL-30 (all three backends: DEL-44) |
| A2 — Command-line tool (a word tally) | `a2/` | interpreter, VM, native | DEL-44 |

Each application runs on every backend its spec lists (`backends`): the interpreter (`bin/fortis run`), the VM
(`fortisc vm`) and natively (`fortisc build`, then the executable), each run checked for its output, exit status,
failures reported once by code, and a start within `start_within_s` (judged only with `FORTIS_ACCEPTANCE_TIMING=1`, the v1.0 release check's quiet-machine run; otherwise recorded); the pools each backend started are read from
its executor trace (`pool_threads`). A spec's `runs` adds further data sets (A2's missing input).

Run one with:

```
python3 -I acceptance/run.py a
```

An application directory holds `project.yaml` and `src/` (the package), `data/` (the files its development
FileSystem starts with), `expected/` (hand-verified output), `negatives/` (one module variant per diagnostic
class, each naming its base module, class and change) and `spec.json` (expected records, test totals, the runtime
inventory, each stated feature with its source patterns, tests and run lines, and mutants that the tests must
kill). The harness writes `run-log.json` with three separate fields (Programme 6.3): `evidence_integrity` (sha256
of every input, toolchain file and output), `execution_attestation` (each command with where, when and its exit
status, the commit and whether the worktree was clean) and `semantic_correctness` (every check and its result).

Application A uses the language core and `fortis.host` only:

- `ids` declares nominal IDs (`SkuId`, `BinId`) and refined arithmetic. A plain operator is used where the
  interval domain proves the result, and a checked operator (`sum + v or fail`) or `validate` elsewhere.
- `feed` is a `noalloc` parser over `StringView` (fields are views `from` the line) and `BytesView` (bytes read in
  place).
- `ledger` holds owned items, copies a name with an explicit `copy`, and returns views `from self`. It uses
  closures of all three kinds and a batch of linear outcomes in a `List` that the caller must consume.
- `main` reads the feeds through the `FileSystem` capability, reports each malformed line once, and observes the
  load with an `observation` schema. Running it starts no task, pool, DI registry or exporter.

Application A2 uses the core only: `async try func main` on the core executor; the `FileSystem`, `Environment` and
`SecretAccess` authorities as parameters of `main`, which the runtime supplies; one `launchBlocking` task that reads the manifest's files on the
blocking pool; a `scope` of cooperative tasks, one per file, counting words; a synchronous `defer` that reports the
end of every run; and a missing input whose `fs.not_found` travels to main and is reported once at the entry
boundary. Its executor traces show one blocking launch and no parallel pool, and it starts no DI registry or exporter.

Each application also has a resolved contract manifest, `contracts.json` (PROG-FND-05), written by
`conformance/tools/release_record.py`. Every declaration that carries a guarantee (`pure`, `noalloc`, `try`,
`async`, a named contract, a refinement, a view `from` an origin, an `owned` parameter, an observation) is listed
with the checker that establishes the guarantee, its scope, and its trust assumptions: the baseline, plus the
transitive trusted operations taken from the native build manifest.
