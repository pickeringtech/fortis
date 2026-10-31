# Contributing to Fortis

Thank you for wanting to help. Fortis is a language, its tools and its documentation, all in this repository, and
they are kept in step: a change to one is checked against the others. This page tells you how to build it, how to
check your change, and what a change needs before it can merge.

## What you need

- Rust, stable, with Cargo (`rustup` is the easy way to get it).
- Python 3.12 or later. The reference tools use only the standard library.
- Pillow (`python3 -m pip install Pillow`), only if you build the website.
- Linux, to run the whole gate: some of its checks sandbox the tools with Linux namespaces and seccomp, so they need
  unprivileged user namespaces to be allowed.

## Build

```bash
cd compiler && cargo build --release
```

That builds `fortisc`, the production toolchain, into `compiler/target/release/`. The reference toolchain needs no
build: `bin/fortis` runs the reference checker and the definitional interpreter.

```bash
bin/fortis check examples/01_hello.ft
compiler/target/release/fortisc check examples/01_hello.ft
```

## Check your change

Start with unit tests for each function you change, in both languages where both implement it. Python uses
`unittest` in `tests/python/<area>/test_*.py`: run `python3 -I conformance/tools/unit_check.py` (or add `--json`).
Rust tests live beside the code or in a crate's `tests/` directory: run `cargo test --manifest-path compiler/Cargo.toml`
(or select a crate with `-p`). Use shared case tables for checker rules and seeded properties for algebraic logic;
keep corpus programs for end-to-end behaviour and interactions. See [tests/README.md](tests/README.md).

Every change passes the gate. It runs the repository's checks, each with its own time limit, and gives one
verdict. A pull request must pass its quick tier:

```bash
python3 -I conformance/tools/gate.py --tier quick
```

The quick tier runs every program on the interpreter, the VM and natively and compares them, and runs every
documentation example, both acceptance applications and the pilot's tests; it leaves out a few compile-side
checks, and lists them. The full tier (no `--tier`) runs everything, and runs at every release tag. The first run
builds the compiler and takes a while; later runs are quicker, and the gate runs independent steps at once when
your machine has the cores. Add `--json build/gate.json` for a report, `--only STEP,STEP` to rerun the steps you
are working on, and `--from STEP` to resume. The unit step runs first and stops the gate if it fails; after that, every selected step runs so one run shows
everything else to fix.

## How a change gets in

1. Open the pull request as a draft (`gh pr create --draft`). CI does not run on drafts.
2. Run `python3 -I conformance/tools/gate.py --tier quick --changed origin/master` locally, and fix what it finds.
   It prints the classes, steps and reasons for the changes since the merge base, including staged and unstaged
   edits and new files. Add new files to git before the final run, because some checks read tracked files only.
3. When it passes, mark the pull request ready (`gh pr ready`). CI uses the same plan against the pull request's
   base. It runs what the change can affect, split across machines.
4. When CI and the review agree, the maintainers take it in.

The shared path table is `PATH_RULES` in `conformance/tools/ci_plan.py`. Named process Markdown runs the docs
checks (`plan`, `cite`, `terms`, `hygiene`, with `syntax` first), without building the compiler. Documentation
whose examples or rendered pages are checked also builds the toolchain and runs `docs`, `docset`, `website`,
`site`, `libref` and `limitations`, plus the showcase and ownership-shape checks (with their checker prerequisite).
Test files run Python units and the Rust tests, adding the docs checks when
Markdown cites them. A change spanning classes runs their union. Every unrecognised path runs the whole quick
tier, including new areas and tools kept under documentation directories. Both names of a renamed file count,
as do deleted files. An empty diff or a manual CI run uses the whole quick tier.

Rendered pages take precedence over process prose: this includes the Delivery Programme in the offline site
and the published release notes. The table names these exceptions; a unit test checks it against the tools'
document inventories. `quick` remains the one aggregate CI status: every planned job must pass and every
planned gate step must have exactly one owner across the shards. Without `--changed`, the local gate runs as
before. The release mop-up and every tag run everything.

The public repository, [pickeringtech/fortis](https://github.com/pickeringtech/fortis), receives one commit per
release: the release's whole tree, tagged with its version. The maintainers develop Fortis in a working repository
of their own, with the plans and reviews that sit behind each release. So an accepted pull request is not merged here. It is applied there, with you kept as its author, and it
arrives here in the next release; your pull request is closed with a link to that release.

## The bar for a change

- **Language changes start in the Language Reference.** If a change alters what Fortis means (its syntax, its
  rules or what a program does), change `docs/Fortis_Language_Reference.md` first, with the rule and its
  diagnostic codes, and then the checkers, the backends and the documentation to match. The reference is the
  source of truth; an implementation that disagrees with it is a bug in the implementation.
- **Every change passes the gate.** That includes the two checkers agreeing on every fixture, and the
  interpreter, the VM and native agreeing on what each program prints.
- **Every documentation example is checked.** A `fortis` block in the README, the guide (`docs/guide`) or the
  agent mode (`docs/agent`) is a test: both checkers judge it, and it runs on every backend against the output the
  page shows. If you add or change an example, the gate tells you whether it holds.
- **Test what you fix.** A bug fix comes with unit tests that failed before it and pass after it; add a program
  in `conformance/` when the bug concerns end-to-end behaviour or interactions.

## Pull requests

Keep a pull request to one change, and say what it changes and why. If it changes the language, say which rules
of the Language Reference it touches. Small pull requests are reviewed sooner.

If you are not sure whether something is a bug or how to approach a change, open an issue and ask. Questions are
welcome.

Security problems are different: please report them privately, as [SECURITY.md](SECURITY.md) explains.

## Licence

By contributing, you agree that your contribution is released under the [MIT License](LICENSE), like the rest of
Fortis.
