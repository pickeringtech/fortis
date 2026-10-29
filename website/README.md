# The Fortis website

The static site served at https://fortis.picktech.co.uk. Only `website/dist/` is published (the Forge server pulls
the orphan branch `website`, which holds the site alone); everything in dist is generated, so it is gitignored and
never edited by hand.

## Build

    python3 -I website/build.py

Writes `website/dist/`: the landing page (`src/index.html` wrapped in a full document, its example programs
extracted from the documentation at build time), the offline documentation site under `/docs/` (both modes and
search, from `conformance/tools/site.py`, which needs `compiler/target/release/fortisc`), the raw Markdown of
every documentation page beside its HTML, `/llms.txt`, `/llms-full.txt`, `/index.md`, `/examples.json` and one
Markdown file per example, `/robots.txt`, `/sitemap.xml` and `/manifest.webmanifest`.

The site publishes user-facing documentation only: the repository README, the human guide, the agent mode with
the `fortis-code` skill, the three specifications and the v0.0.1 release notes (also rendered to HTML). The
maintainers' planning documents (the Delivery Programme, the decision register, the delivery plan and the review
records) and `AGENTS.md` are never published. A link from a published page to an excluded document is
rewritten in the generated HTML and Markdown to the closest published page or, where none corresponds, to plain
text keeping the words; the sources in `docs/` are never edited, and every rewrite is printed in the build log
and kept in `build/website-rewrites.json`. In `llms.txt` the release ladder is summarised inline from the
delivery plan's table, never linked.

The example programs are never copied: each is extracted from the `fortis` fenced block its source names (the
same blocks the docset harness checks), and its SHA-256 must equal the one in `src/notes.json`. When a source
program changes the build fails, says the notes need reviewing, and ships nothing stale — review the notes,
update the hash, and build again.

## Check

    python3 -I website/build.py --check

Builds into a temporary directory and verifies it: every `href` and `src` of every generated HTML page resolves,
every link in `llms.txt` resolves, every example program matches its source and its hash, the JSON-LD and the
manifest parse, and dist holds no excluded path — a planning or internal document, or any documentation path
that is not a published page, fails the check. This is the merge gate's `website` step.

## Publish

    website/publish.sh

Builds, then commits `website/dist` to the orphan branch `website` (index.html at its root) through a temporary
git worktree and pushes. Idempotent: when the built site is unchanged it makes no commit. Never force-pushes,
except that the first push creates the branch.

## Sources

- `src/index.html` — the landing page, a document fragment; `__LOGO__` marks the logo and the `#examples` script
  is replaced with the programs extracted at build time.
- `src/notes.json` — per example: id, title, file name, source, expected output, review notes with their lines,
  and the SHA-256 of the program as extracted from its source.
- `assets/` — the logos and the derived icons (favicon, apple-touch-icon, manifest icons), made with Pillow.
