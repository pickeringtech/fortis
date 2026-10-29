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
are working on, and `--from STEP` to resume. The gate never stops early, so one run shows everything to fix.

## How a change gets in

1. Open the pull request as a draft (`gh pr create --draft`). CI does not run on drafts.
2. Run the quick tier locally, and fix what it finds.
3. When it passes, mark the pull request ready (`gh pr ready`). CI runs the same quick tier, split across
   machines, in about five minutes.
4. When CI and the review agree, the maintainers take it in.

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
- **Add a fixture for what you fix.** A bug fix comes with a program in `conformance/` that failed before it and
  passes after it.

## Pull requests

Keep a pull request to one change, and say what it changes and why. If it changes the language, say which rules
of the Language Reference it touches. Small pull requests are reviewed sooner.

If you are not sure whether something is a bug or how to approach a change, open an issue and ask. Questions are
welcome.

Security problems are different: please report them privately, as [SECURITY.md](SECURITY.md) explains.

## Licence

By contributing, you agree that your contribution is released under the [MIT License](LICENSE), like the rest of
Fortis.
