#!/usr/bin/env python3
"""The Fortis website: builds `website/dist/`, the static site served at https://fortis.picktech.co.uk.

    python3 -I website/build.py [--check]

The landing page (`website/src/index.html`, a document fragment) is wrapped in a full document and its example
programs are embedded from the documentation at build time, never from a copy: each program is extracted from the
same `fortis` fenced block the docset harness checks (`#n` counting the fortis blocks of the page, the first
marker line `// accept` and whole-line comments removed), and its SHA-256 must equal the one recorded in
`website/src/notes.json`. When a source program changes, the build fails and says the notes need reviewing, and
ships nothing stale.

The site publishes user-facing documentation only: the repository README, every page of the human guide
(`docs/guide`) and the agent mode (`docs/agent`, with the `fortis-code` skill the agent documentation points
to), the three specifications (`docs/Fortis_Specification.md`, `docs/Fortis_Language_Reference.md`,
`docs/Fortis_Platform_Specification.md`) and the release notes (`docs/releases/v0.0.1.md`, `docs/releases/v0.0.2.md`, `docs/releases/v0.1.0.md`), each rendered
to HTML with its raw Markdown beside it. The maintainers' planning documents (the Delivery Programme, the decision
register, the delivery plan and the review records) and `AGENTS.md` are never published. A link from a published
page to an excluded document does not dangle: it is rewritten in the generated HTML and Markdown to the closest
published page (LINK_REWRITES) or, where no published page corresponds, to plain text keeping the link's words;
the sources in `docs/` are never edited, and every rewrite is printed in the build log and recorded in
`build/website-rewrites.json`.

Everything else in dist is generated too: the offline documentation site (`conformance/tools/site.py`'s own
renderer, both modes and search), the agent-facing files of llmstxt.org (`/llms.txt`, `/llms-full.txt`; the
release ladder is summarised inline in llms.txt from the delivery plan's table, never linked, and left out, with a
line saying so, in a checkout without that planning file), `/index.md`,
`/examples.json` and one Markdown file per example, `/robots.txt`, `/sitemap.xml`, `/manifest.webmanifest` and
the Open Graph image.

`--check` builds into a temporary directory and verifies it: every `href` and `src` of every generated HTML page
resolves inside it, every link of llms.txt resolves, every example program matches its source and its recorded
hash, the JSON-LD and the manifest parse, and dist holds no excluded path (a planning or internal document fails
the check wherever it appears).
"""
from __future__ import annotations

import hashlib
import html
import importlib.util
import json
import os
import re
import shutil
import sys
import tempfile
from collections import Counter

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, ".."))
SRC = os.path.join(HERE, "src")
ASSETS = os.path.join(HERE, "assets")
DIST = os.path.join(HERE, "dist")
SITE = "https://fortis.picktech.co.uk"
NAVY = (20, 41, 65)                      # #142941, the brand colour of the landing page's ink
DESCRIPTION = ("Fortis is a programming language for software written mostly by AI agents and reviewed by "
               "people. Ownership, changes, failures and effects are explicit in the source.")

_spec = importlib.util.spec_from_file_location("fortis_docsite", os.path.join(ROOT, "conformance", "tools", "site.py"))
docsite = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(docsite)
sys.path.insert(0, os.path.join(ROOT, "conformance", "tools"))
from docset_check import fences  # noqa: E402

# the site publishes user-facing documentation only: the documentation set (the README, the human guide, the
# agent mode and the `fortis-code` skill, from the citation check's inventory), these specifications and the
# release notes; the Delivery Programme, the decision register, the delivery plan and the review records are the
# maintainers' planning documents, and are never published
PUBLISHED_SPECS = ["docs/Fortis_Specification.md", "docs/Fortis_Language_Reference.md",
                   "docs/Fortis_Platform_Specification.md"]
# documentation pages published beside the documentation set, rendered with the documentation site's renderer
EXTRA_RENDER = ["docs/releases/v0.0.1.md", "docs/releases/v0.0.2.md", "docs/releases/v0.1.0.md"]
EXTERNAL_OK = ("https://fonts.googleapis.com/",)

# repository paths the site never publishes: the planning and internal documents, and the .html forms the
# documentation site's renderer would give them
EXCLUDED_FILES = {"AGENTS.md", "CLAUDE.md", "docs",
                  "docs/Fortis_Delivery_Programme.md", "docs/Fortis_Delivery_Programme.html",
                  "docs/Fortis_Specification_Review.md", "docs/Fortis_Specification_Review.html"}
EXCLUDED_PREFIXES = ("docs/delivery/", "docs/reviews/")
# excluded link target -> the published page that replaces it (the closest published page); every other excluded
# target becomes plain text, keeping the link's words
LINK_REWRITES = {
    # the delivery status board is published as the current release notes (the landing page's Status agrees)
    "docs/delivery/status.md": "docs/releases/v0.1.0.md",
}
# the href a link to an excluded document gets when no published page replaces it: WebsitePage.render replaces
# the whole anchor with its label, so the words stay and no link dangles
PLAIN_HREF = "\x02plain\x02"
REWRITES = []       # the build log of link rewrites: (where, page, link, action)


def fail(message):
    print(f"FAIL {message}")
    sys.exit(1)


# ---------------------------------------------------------------------------------------------------------------
# the example programs, from the documentation
# ---------------------------------------------------------------------------------------------------------------

def extract_program(source):
    """The program of one example: the `fortis` fenced block its source names (`#n` counting the fortis blocks of
    the page, the first block when no `#n` is given), with the first marker line `// accept` and the whole-line
    comments removed, exactly as the published program reads."""
    page, _, frag = source.partition("#")
    path = os.path.join(ROOT, page)
    if not os.path.exists(path):
        fail(f"the example source {page} does not exist")
    blocks = [b for b in fences(open(path, encoding="utf-8").read()) if b["lang"] == "fortis"]
    if not blocks:
        fail(f"{page} has no fortis block")
    b = blocks[int(frag) - 1] if frag else blocks[0]
    lines = b["lines"]
    if not lines or not lines[0].strip().startswith("// accept"):
        fail(f"{source}: the block's first line is not `// accept`")
    return "\n".join(l for l in lines[1:] if not l.strip().startswith("//"))


def load_examples(checking):
    """[(notes entry, program)] with every program extracted from its source and checked against its hash."""
    notes = json.load(open(os.path.join(SRC, "notes.json"), encoding="utf-8"))
    out = []
    for entry in notes:
        program = extract_program(entry["source"])
        sha = hashlib.sha256(program.encode("utf-8")).hexdigest()
        if sha != entry["sha256"]:
            print(f"FAIL example {entry['id']}: the program extracted from {entry['source']} has SHA-256 {sha}, "
                  f"not the {entry['sha256']} recorded in website/src/notes.json. The example's notes (and this "
                  f"hash) need reviewing before the site can build; nothing stale is shipped.")
            if not checking and os.path.isdir(DIST):
                shutil.rmtree(DIST)
            sys.exit(1)
        out.append((entry, program))
    return out


def rendered_pages():
    """The documentation pages the website publishes: each is rendered to HTML and copied as raw Markdown."""
    return docsite.doc_pages() + [p for p in PUBLISHED_SPECS + EXTRA_RENDER
                                  if os.path.exists(os.path.join(ROOT, p))]


PUBLISHED = frozenset(rendered_pages()) | {p[:-3] + ".html" for p in rendered_pages()}


def excluded_path(rel):
    """Whether a repository path is excluded from the site: a planning or internal document. Anything under
    docs/ that is not a published documentation page is internal; repository files a published page links to
    (library sources, schemas, the LICENSE, tools, directories) are copied as the documentation site always
    has."""
    rel = rel.rstrip("/")
    if rel in EXCLUDED_FILES or rel.startswith(EXCLUDED_PREFIXES):
        return True
    return rel.startswith("docs/") and rel not in PUBLISHED


def record_rewrite(where, page, link, action):
    """One entry of the build log of link rewrites."""
    REWRITES.append((where, page, link, action))


def page_url(page, markdown=False):
    """Where a repository documentation page is published under /docs/."""
    if markdown or page not in rendered_pages():
        return "/docs/" + page
    return "/docs/" + page[:-3] + ".html"


class WebsiteLinker(docsite.Linker):
    """The documentation site's Linker, restricted to the published pages: a link to an excluded document is
    rewritten to the closest published page (LINK_REWRITES) or, where no published page corresponds, to plain
    text keeping the link's words. Every rewrite is recorded for the build log; the sources are never edited."""

    def href(self, page, target):
        if re.match(r"^[a-z]+:", target):
            return target
        path, _, anchor = target.partition("#")
        if not path:
            return f"#{anchor}"
        full = os.path.normpath(os.path.join(os.path.dirname(page), path))
        if not excluded_path(full):
            return super().href(page, target)
        repl = LINK_REWRITES.get(full)
        if repl:
            record_rewrite("html", page, f"({target})", repl)
            return self.rel(page, repl[:-3] + ".html")        # the excluded document's own anchor goes with it
        record_rewrite("html", page, f"({target})", "plain text")
        return PLAIN_HREF

    def autolink(self, page, text, keep):
        def cite(m):
            doc = m.group(1)
            path = docsite.SPECS[doc]
            parts = re.split(r"(,\s*|\s+and\s+)", m.group(2))
            out = []
            for k, part in enumerate(parts):
                if k % 2:
                    out.append(part)
                    continue
                sec = self.sections[doc].get(part) if doc in self.sections else None    # a document not in the checkout
                label = f"{doc} {part}" if k == 0 else part
                if sec and page != path:
                    if excluded_path(path):
                        # a citation of the Delivery Programme: the words stay, the link goes
                        record_rewrite("html", page, f'citation "{label}"', "plain text")
                        out.append(label)
                    else:
                        out.append(keep(f'<a class="cite" href="{self.rel(page, path[:-3] + ".html", sec[2])}">{label}</a>'))
                else:
                    out.append(label)
            return "".join(out)

        text = docsite.CITE.sub(cite, text)

        def rule(m):
            rid = m.group(1)
            doc = {"LANG": "docs/Fortis_Language_Reference.md", "PROG": "docs/Fortis_Delivery_Programme.md",
                   "PLAT": "docs/Fortis_Platform_Specification.md"}[rid.split("-")[0]]
            if excluded_path(doc):
                # a PROG-… rule ID names a row of the Delivery Programme: the ID stays as words
                record_rewrite("html", page, f"rule ID {rid}", "plain text")
                return rid
            if doc == page:
                return keep(f'<a class="rule" href="#{rid}">{rid}</a>')
            return keep(f'<a class="rule" href="{self.rel(page, doc[:-3] + ".html", rid)}">{rid}</a>')

        text = docsite.RULE_ID.sub(rule, text)
        if self.code_page:
            def code(m):
                c = m.group(0)
                if c not in self.code_ids:
                    return c
                if page == self.code_page:
                    return keep(f'<a class="code" href="#{c}">{c}</a>')
                return keep(f'<a class="code" href="{self.rel(page, self.code_page[:-3] + ".html", c)}">{c}</a>')
            text = docsite.CODE_ID.sub(code, text)
        return text


class WebsitePage(docsite.Page):
    """The documentation site's Page with the plain-text rewrites applied: an anchor whose href is the sentinel
    becomes its label, so a link to an excluded document with no published replacement keeps its words and no
    link dangles."""

    def render(self, text):
        body = super().render(text)
        return re.sub(rf'<a href="{PLAIN_HREF}">((?:[^<]|<(?!/a>))*)</a>', lambda m: m.group(1), body)


def render_docs(out):
    """The offline documentation site under `out/docs/`, from the documentation site's own renderer over the
    published pages only: the planning and internal documents are not rendered, not copied and not linked."""
    docs_out = os.path.join(out, "docs")
    saved = (docsite.SPEC_PAGES, docsite.pages, docsite.Linker, docsite.Page)
    docsite.SPEC_PAGES = list(PUBLISHED_SPECS)
    docsite.pages = rendered_pages
    docsite.Linker = WebsiteLinker
    docsite.Page = WebsitePage
    try:
        built, sections, blocks, highlighted = docsite.render_site(docs_out)
    finally:
        docsite.SPEC_PAGES, docsite.pages, docsite.Linker, docsite.Page = saved
    missing = [p for p in rendered_pages() if not os.path.exists(os.path.join(docs_out, p[:-3] + ".html"))]
    for p in missing:
        print(f"FAIL {p}: a published page was not rendered")
    problems, n = docsite.check_links(docs_out)
    # the specification's own references to outside sources are not this site's links (as site.py treats them)
    spec_external = [p for p in problems if p[2] == "external link" and p[0].startswith("docs/Fortis_")]
    problems = [p for p in problems if p not in spec_external]
    for page, href, why in problems:
        print(f"FAIL {page}: {href}: {why}")
    if missing or problems:
        fail(f"the documentation site did not build ({len(missing)} pages missing, {len(problems)} broken links)")
    if blocks != highlighted:
        fail(f"{blocks - highlighted} Fortis blocks were not highlighted by fortisc view "
             f"(the documentation site needs compiler/target/release/fortisc)")
    print(f"website: documentation site: {len(built)} pages, {sections} searchable sections, {blocks} Fortis "
          f"blocks ({highlighted} highlighted), {n} links checked, {len(spec_external)} external links in the "
          f"specification (not followed)")


def rewrite_markdown_links(page, text):
    """The Markdown of a published page as published beside its HTML: every link to an excluded document is
    rewritten exactly as in the HTML (to the closest published page's Markdown, or to plain text keeping the
    words); the source in the repository is never edited."""
    base = os.path.dirname(page)

    def link(m):
        label, target = m.group(1), m.group(2)
        if re.match(r"^[a-z]+:", target):
            return m.group(0)
        path, _, anchor = target.partition("#")
        if not path:
            return m.group(0)
        full = os.path.normpath(os.path.join(base, path))
        if not excluded_path(full):
            return m.group(0)
        repl = LINK_REWRITES.get(full)
        if repl:
            new = os.path.relpath(repl, base or ".").replace(os.sep, "/")
            record_rewrite("md", page, f"[{label}]({target})", new)
            return f"[{label}]({new})"
        record_rewrite("md", page, f"[{label}]({target})", "plain text")
        return label

    out, fence = [], None
    for line in text.split("\n"):
        s = line.strip()
        if fence:
            if s == fence:
                fence = None
            out.append(line)
            continue
        fm = re.match(r"^(`{3,}|~{3,})", s)
        if fm:
            fence = fm.group(1)
            out.append(line)
            continue
        parts = re.split(r"(`+[^`]*`+)", line)       # only outside fenced and inline code, where `f[Int](x)` is code
        for k in range(0, len(parts), 2):
            parts[k] = re.sub(r"\[([^\]]+)\]\(([^)\s]+)\)", link, parts[k])
        out.append("".join(parts))
    return "\n".join(out)


def rewrite_log():
    """Print the build log of link rewrites, and keep it in build/website-rewrites.json."""
    if not REWRITES:
        return
    counts = Counter(REWRITES)
    print(f"website: {sum(counts.values())} link rewrites: published pages linked documents the site excludes; "
          f"the sources are not edited, every rewrite is here:")
    for (where, page, link, action), n in sorted(counts.items()):
        print(f"  rewrite [{where}] {page}: {link} -> {action}" + (f" (x{n})" if n > 1 else ""))
    os.makedirs(os.path.join(ROOT, "build"), exist_ok=True)
    json.dump([{"where": w, "page": p, "link": l, "action": a, "count": n}
               for (w, p, l, a), n in sorted(counts.items())],
              open(os.path.join(ROOT, "build", "website-rewrites.json"), "w", encoding="utf-8"), indent=1)


# ---------------------------------------------------------------------------------------------------------------
# the landing page
# ---------------------------------------------------------------------------------------------------------------

def landing_html(examples):
    fragment = open(os.path.join(SRC, "index.html"), encoding="utf-8").read()
    lines = []
    for l in fragment.split("\n"):
        if re.match(r"^<title>", l) or re.match(r"^<meta name=\"(?:viewport|description)\"", l):
            continue                      # the wrapping document carries these
        lines.append(l)
    body = "\n".join(lines)

    def link(m):
        page, _, anchor = m.group(1).partition("#")
        if anchor:
            fail(f"the landing page links {m.group(1)} with an anchor the built page does not define")
        if page in rendered_pages():
            return f'href="{page_url(page)}"'
        if excluded_path(page):
            repl = LINK_REWRITES.get(page)
            if repl:
                record_rewrite("landing", "website/src/index.html", f'href="{m.group(1)}"', page_url(repl))
                return f'href="{page_url(repl)}"'
            fail(f"the landing page links {page}, which the site excludes and no published page replaces; "
                 f"edit website/src/index.html")
        fail(f"the landing page links {page}, which the site does not publish")
    body = re.sub(r'href="([^"]+\.md)(?:#[^"]*)?"', link, body)
    body = body.replace("__LOGO__", "/assets/logo-256.png")

    embedded = [{"id": e["id"], "title": e["title"], "file": e["file"], "source": page_url(e["source"].partition("#")[0]),
                 "program": p, "output": e["output"], "notes": None if e["id"] == "mean" else e["notes"]}
                for e, p in examples]
    body, n = re.subn(r'(<script type="application/json" id="examples">).*?(</script>)',
                      lambda m: m.group(1) + json.dumps(embedded, indent=1) + m.group(2), body, flags=re.S)
    if n != 1:
        fail("the landing page's #examples script was not found")
    mean = next(p for e, p in examples if e["id"] == "mean")
    body, n = re.subn(r'(<script type="text/plain" id="src">).*?(</script>)',
                      lambda m: m.group(1) + mean + m.group(2), body, flags=re.S)
    if n != 1:
        fail("the landing page's #src script was not found")

    ld = {"@context": "https://schema.org", "@type": "ComputerLanguage", "name": "Fortis", "url": SITE + "/",
          "description": DESCRIPTION, "version": "0.1.0", "license": "https://spdx.org/licenses/MIT.html",
          "publisher": {"@type": "Organization", "name": "Pickering Technologies Ltd"},
          "codeRepository": "https://github.com/pickeringtech/fortis"}
    head = f"""<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">
<title>Fortis: AI-written code people can review</title>
<meta name="description" content="{html.escape(DESCRIPTION, quote=True)}">
<link rel="canonical" href="{SITE}/">
<meta property="og:type" content="website">
<meta property="og:site_name" content="Fortis">
<meta property="og:title" content="Fortis: AI-written code people can review">
<meta property="og:description" content="{html.escape(DESCRIPTION, quote=True)}">
<meta property="og:url" content="{SITE}/">
<meta property="og:image" content="{SITE}/assets/og.png">
<meta property="og:image:width" content="1200">
<meta property="og:image:height" content="630">
<meta name="twitter:card" content="summary_large_image">
<meta name="twitter:title" content="Fortis: AI-written code people can review">
<meta name="twitter:description" content="{html.escape(DESCRIPTION, quote=True)}">
<meta name="twitter:image" content="{SITE}/assets/og.png">
<link rel="icon" href="/assets/favicon.ico" sizes="16x16 32x32">
<link rel="apple-touch-icon" href="/assets/apple-touch-icon.png">
<link rel="manifest" href="/manifest.webmanifest">
<link rel="alternate" type="text/markdown" href="/index.md">
<script type="application/ld+json">
{json.dumps(ld, indent=1)}
</script>
</head>
<body>
"""
    return head + body + "\n</body>\n</html>\n"


# ---------------------------------------------------------------------------------------------------------------
# the generated text files
# ---------------------------------------------------------------------------------------------------------------

# pages whose first prose sentence is an example's, not the page's: a curated one-line description instead
DESCRIPTIONS = {
    "docs/agent/03-lexical.md": "The lexical rules of LR 3 by rule ID: names, keywords, literals, strings and "
                                "comments, with codes and corrections.",
    "docs/agent/05-statements.md": "The statement and expression rules of LR 5 by rule ID, with codes and "
                                   "corrections.",
    "docs/agent/09-classes.md": "The class, interface, mixin and enum rules of LR 9 by rule ID, with codes and "
                                "corrections.",
    "docs/agent/10-outcomes.md": "The outcome and failure rules of LR 10 by rule ID: declared errors, obligations "
                                 "and recovery, with codes and corrections.",
    "docs/agent/11-effects.md": "The effect and contract rules of LR 11 by rule ID: claims, capabilities and "
                                "noalloc, with codes and corrections.",
    "docs/agent/12-async.md": "The cleanup, async and concurrency rules of LR 12 by rule ID, with codes and "
                              "corrections.",
    "docs/agent/library.md": "The signatures of the core library package, generated from the declarations of "
                             "library/fortis.",
    "docs/releases/v0.0.1.md": "The v0.0.1 release notes: what the release ships, how it was checked and "
                               "reviewed, and what happens next.",
    "docs/releases/v0.1.0.md": "The v0.1.0 release: domain modelling, conversions, collections and text.",
    "docs/releases/v0.0.2.md": "The v0.0.2 release notes: the reading release's seventeen changes, the decisions "
                               "for review, the defects fixed and how to migrate with fortis fmt.",
}


def first_sentence(page):
    """A one-line description of a page: the first sentence of its first substantive paragraph."""
    if page in DESCRIPTIONS:
        return DESCRIPTIONS[page]
    text = open(os.path.join(ROOT, page), encoding="utf-8").read()
    text = re.sub(r"^---\n.*?\n---\n", "", text, flags=re.S)
    paras, para, fence = [], [], None
    for l in text.split("\n"):
        s = l.strip()
        fm = re.match(r"^(`{3,}|~{3,})", s)
        if fence:
            if s == fence:
                fence = None
            continue
        if fm:
            if para:
                paras.append(para)
                para = []
            fence = fm.group(1)
            continue
        if not s or s.startswith(("#", ">", "|", "<!--")):
            if para:
                paras.append(para)
                para = []
            continue
        para.append(s)
    if para:
        paras.append(para)

    def plain_of(para):
        plain = html.unescape(re.sub(r"[`*]", "", re.sub(r"\[([^\]]*)\]\([^)]*\)", r"\1", " ".join(para))))
        return re.sub(r"^(?:[-*+]|\d+[.)])\s+", "", plain)

    paras = [p for p in paras if not plain_of(p).startswith(("Agent mode.", "Human mode."))]
    plain = ""
    for bullet in (False, True):
        for para in paras:
            if bool(re.match(r"^(?:[-*+]|\d+[.)])\s+", para[0])) != bullet:
                continue
            candidate = plain_of(para)
            if len(candidate) >= 60:
                plain = candidate
                break
        if plain:
            break
    if not plain and paras:
        plain = plain_of(paras[0])
    m = re.search(r"(?<=\.)\s+", plain)
    return (plain[:m.end() - 1].strip() if m else plain)[:200]


def entry(title, url, desc):
    return f"- [{title}]({url})" + (f": {desc}" if desc else "")


def release_ladder():
    """The release ladder as the delivery plan's table states it: versions and what each lets you do. The plan
    is internal; llms.txt summarises its table inline and never links it. In a checkout without the plan (a
    maintainers' planning file), None: llms.txt leaves the ladder out."""
    import planning
    path = planning.optional(planning.RELEASES, "website", "the release ladder in llms.txt")
    if not path:
        return None
    text = open(path, encoding="utf-8").read()
    m = re.search(r"^## The ladder\n(.*?)(?=^## )", text, re.M | re.S)
    if not m:
        fail(f"{planning.RELEASES} has no `## The ladder` section to summarise in llms.txt")
    rows = re.findall(r"^\|\s*(v[\d.]+)\s*\|\s*([^|]+?)\s*\|", m.group(1), re.M)
    if not rows:
        fail(f"the release ladder table in {planning.RELEASES} is empty")
    return rows


def llms_txt(examples):
    pages = rendered_pages()
    agent = docsite.order("docs/agent/README.md", [p for p in pages if docsite.mode_of(p) == "agent"])
    human = docsite.order("docs/guide/README.md", [p for p in pages if docsite.mode_of(p) == "human"])
    readme = open(os.path.join(ROOT, "README.md"), encoding="utf-8").read()
    para = readme.split("\n\n", 2)[1] if readme.startswith("# ") else readme.split("\n\n", 1)[0]
    para = re.sub(r"\s*\n\s*", " ", para)
    para = re.sub(r"\[([^\]]*)\]\([^)]*\)", r"\1", para)
    out = ["# Fortis", "", "> " + para.strip(), ""]
    out += ["Latest: Model a domain (v0.1.0). Developed by Pickering Technologies Ltd. MIT licence.",
            "Markdown of every page is published beside its HTML; the full agent-mode documentation is "
            "concatenated in /llms-full.txt.", ""]
    out.append("## Agent documentation")
    out += [entry(docsite.title_of(p), page_url(p, markdown=True), first_sentence(p)) for p in agent]
    out += ["", "## Language Reference"]
    out += [entry(docsite.title_of(p), page_url(p, markdown=True), first_sentence(p)) for p in PUBLISHED_SPECS]
    out += ["", "## Human guide"]
    out += [entry(docsite.title_of(p), page_url(p, markdown=True), first_sentence(p))
            for p in ["README.md"] + human]
    out += ["", "## Release"]
    out += [entry("Latest: Model a domain (v0.1.0)", page_url("docs/releases/v0.1.0.md", markdown=True),
                  first_sentence("docs/releases/v0.1.0.md")),
            entry("v0.0.2 release notes", page_url("docs/releases/v0.0.2.md", markdown=True),
                  first_sentence("docs/releases/v0.0.2.md")),
            entry("v0.0.1 release notes", page_url("docs/releases/v0.0.1.md", markdown=True),
                  first_sentence("docs/releases/v0.0.1.md")),
            entry("Known limitations (human guide)", page_url("docs/guide/known-limitations.md", markdown=True),
                  first_sentence("docs/guide/known-limitations.md")),
            entry("Known limitations (agent mode)", page_url("docs/agent/known-limitations.md", markdown=True),
                  first_sentence("docs/agent/known-limitations.md"))]
    ladder = release_ladder()
    if ladder:
        out += ["", "The release ladder (each release keeps everything earlier releases shipped, and adds):"]
        out += [f"- {v}: {what}" for v, what in ladder]
    out += ["", "## Examples"]
    out.append(f"- [Examples (JSON)](/examples.json): the four example programs with their review notes, line "
               f"mappings and expected output.")
    out += [entry(e["title"], f"/examples/{e['id']}.md", f"the complete program `{e['file']}` with its review notes.")
            for e, _ in examples]
    out += ["", "## Optional"]
    optional = ["skills/fortis-code/SKILL.md", "/index.md", "/llms-full.txt"]
    desc = {"skills/fortis-code/SKILL.md": "the official agent skill, installable with `bin/fortis skill install`.",
            "/index.md": "the landing page as Markdown.",
            "/llms-full.txt": "the full agent-mode documentation in one file."}
    out += [entry(docsite.title_of(p) if not p.startswith("/") else p.split("/")[-1],
                  page_url(p, markdown=True) if not p.startswith("/") else p, desc[p]) for p in optional]
    return "\n".join(out) + "\n"


def llms_full_txt():
    pages = rendered_pages()
    agent = docsite.order("docs/agent/README.md", [p for p in pages if docsite.mode_of(p) == "agent"])
    out = ["The Fortis agent-mode documentation, concatenated in reading order. Each page is preceded by its",
           "source path in the repository; the same Markdown is published beside each page's HTML under /docs/.",
           ""]
    for p in agent:
        text = open(os.path.join(ROOT, p), encoding="utf-8").read()
        if p.endswith("SKILL.md"):
            text = re.sub(r"^---\n.*?\n---\n", "", text, flags=re.S)
        # the same Markdown as published beside the page's HTML, its excluded links rewritten the same way
        out += ["", "---", f"source: {p}", "---", "", rewrite_markdown_links(p, text.rstrip("\n"))]
    return "\n".join(out) + "\n"


def fmt_lines(lines):
    out, start, prev = [], None, None
    for n in sorted(lines):
        if prev is not None and n == prev + 1:
            prev = n
            continue
        if start is not None:
            out.append(str(start) if start == prev else f"{start}–{prev}")
        start = prev = n
    if start is not None:
        out.append(str(start) if start == prev else f"{start}–{prev}")
    return ", ".join(out)


def example_md(entry, program):
    out = [f"# {entry['title']}", "",
           f"The complete program `{entry['file']}`, from the `{entry['source']}` example of the Fortis "
           f"documentation.", "",
           "```fortis", program, "```", "", "Expected output:", "", "```", entry["output"], "```", ""]
    if entry["notes"]:
        out += ["Review notes:", ""]
        out += [f"{i}. **{n['title']}** — {n['text']} (lines {fmt_lines(n['lines'])})"
                for i, n in enumerate(entry["notes"], 1)]
        out += [""]
    return "\n".join(out)


def index_md(examples):
    out = ["# Fortis: AI-written code people can review", "",
           "Fortis is a statically checked, compiled language for software written mostly by AI agents. "
           "**People can read, review and trust it because every decision is written in the source.** Ownership, "
           "changes, failures and effects are explicit.", "",
           "Latest: Model a domain (v0.1.0). Language 1.0 is still being built and is not released.", "",
           "## Examples", ""]
    for entry, program in examples:
        out += [f"### {entry['title']} (`{entry['file']}`)", "",
                f"Source: `{entry['source']}`. Expected output: `{entry['output']}`.", "",
                "```fortis", program, "```", ""]
        if entry["notes"]:
            out += ["Notes:", ""]
            out += [f"{i}. **{n['title']}** — {n['text']} (lines {fmt_lines(n['lines'])})"
                    for i, n in enumerate(entry["notes"], 1)]
            out += [""]
    out += ["## Try it", "",
            "From a checkout of the repository, with Python 3 and a Rust toolchain:", "",
            "```sh", "cargo build --release --manifest-path compiler/Cargo.toml",
            "bin/fortis check mean.ft", "bin/fortis run mean.ft", "```", "",
            "The same program runs on the definitional interpreter (`bin/fortis run`), the development VM "
            "(`compiler/target/release/fortisc vm`) and native code (`bin/fortis build`), and all three must "
            "agree.", "",
            "## Roadmap", "",
            "- **v0.0.1**: write, check, run, build, format and test programs. Modules, functions, classes, "
            "enums, ownership and failures.",
            "- **v0.0.2**: code that says only what matters. Fewer markers, capabilities as parameters and "
            "built-in logging.",
            "- **v0.1.0 (latest)**: model a domain. Refinements, newtypes, conversions, interfaces, generics, contracts, text and collections.",
            "- **v0.2**: trust effects and test properly. Checked effects and capabilities, and property tests.",
            "- **v0.3**: build a web service. An HTTP server with health, readiness and metrics built in, SQLite "
            "and typed queries.",
            "- **v0.4**: build web apps. HTML, forms, sessions, authentication, Postgres and migrations.",
            "- **v0.5**: run it in production. Telemetry, connection pools, timeouts, rate limits and containers.",
            "- **v0.6**: organise and extend code. Packages with dependencies, checked architecture and extensions.",
            "- **v0.7–0.9**: async in your own code, a complete standard library, and tooling for large code bases.",
            "- **v1.0**: stability. Editions, migration and a stability promise. Performance judged against Rust.",
            "", "## Licence", "",
            "Developed by Pickering Technologies Ltd. Released under the MIT License.", ""]
    return "\n".join(out)


# ---------------------------------------------------------------------------------------------------------------
# the build
# ---------------------------------------------------------------------------------------------------------------

def make_og(dest):
    from PIL import Image
    logo = Image.open(os.path.join(ASSETS, "logo.png")).convert("RGBA")
    size = 470
    logo = logo.resize((size, size), Image.LANCZOS)
    canvas = Image.new("RGBA", (1200, 630), NAVY + (255,))
    canvas.paste(logo, ((1200 - size) // 2, (630 - size) // 2), logo)
    canvas.convert("RGB").save(dest)


def write(out, examples):
    REWRITES.clear()
    if os.path.isdir(out):
        shutil.rmtree(out)
    os.makedirs(out)

    render_docs(out)

    open(os.path.join(out, "index.html"), "w", encoding="utf-8").write(landing_html(examples))

    shutil.copytree(ASSETS, os.path.join(out, "assets"))
    make_og(os.path.join(out, "assets", "og.png"))

    # the raw Markdown of every published page beside its HTML, with the same link rewrites as the HTML
    for page in rendered_pages():
        dest = os.path.join(out, "docs", page)
        os.makedirs(os.path.dirname(dest), exist_ok=True)
        text = open(os.path.join(ROOT, page), encoding="utf-8").read()
        open(dest, "w", encoding="utf-8").write(rewrite_markdown_links(page, text))

    os.makedirs(os.path.join(out, "examples"))
    for entry, program in examples:
        open(os.path.join(out, "examples", f"{entry['id']}.md"), "w", encoding="utf-8").write(example_md(entry, program))
    pub = [{"id": e["id"], "title": e["title"], "file": e["file"], "source": e["source"], "program": p,
            "output": e["output"], "notes": e["notes"], "sha256": e["sha256"]} for e, p in examples]
    open(os.path.join(out, "examples.json"), "w", encoding="utf-8").write(json.dumps(pub, indent=1) + "\n")

    open(os.path.join(out, "llms.txt"), "w", encoding="utf-8").write(llms_txt(examples))
    open(os.path.join(out, "llms-full.txt"), "w", encoding="utf-8").write(llms_full_txt())
    open(os.path.join(out, "index.md"), "w", encoding="utf-8").write(index_md(examples))

    manifest = {"name": "Fortis", "short_name": "Fortis", "description": DESCRIPTION, "start_url": "/", "scope": "/",
                "display": "standalone", "background_color": "#142941", "theme_color": "#142941",
                "icons": [{"src": "/assets/icon-192.png", "sizes": "192x192", "type": "image/png"},
                          {"src": "/assets/icon-512.png", "sizes": "512x512", "type": "image/png"}]}
    open(os.path.join(out, "manifest.webmanifest"), "w", encoding="utf-8").write(json.dumps(manifest, indent=1) + "\n")

    open(os.path.join(out, "robots.txt"), "w", encoding="utf-8").write(
        "# Reading agents: a map of this site in Markdown is at /llms.txt, the full documentation at /llms-full.txt.\n"
        "User-agent: *\nAllow: /\n\n"
        f"Sitemap: {SITE}/sitemap.xml\n")

    urls = [SITE + "/"]
    for root, _, files in os.walk(out):
        for f in sorted(files):
            if f.endswith(".html"):
                rel = os.path.relpath(os.path.join(root, f), out).replace(os.sep, "/")
                if rel != "index.html":
                    urls.append(f"{SITE}/{rel}")
    open(os.path.join(out, "sitemap.xml"), "w", encoding="utf-8").write(
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n'
        + "".join(f"<url><loc>{html.escape(u)}</loc></url>\n" for u in urls)
        + "</urlset>\n")

    rewrite_log()


# ---------------------------------------------------------------------------------------------------------------
# the check
# ---------------------------------------------------------------------------------------------------------------

def anchors_of(path, cache={}):
    if path not in cache:
        try:
            text = open(path, encoding="utf-8").read()
        except (OSError, UnicodeDecodeError):
            text = ""
        cache[path] = set(re.findall(r'\bid="([^"]+)"', text))
    return cache[path]


def check(out):
    problems = []

    def resolve(base, href):
        if re.match(r"^[a-z]+:", href):
            return None
        target, _, anchor = href.partition("#")
        if not target:
            return None
        full = os.path.normpath(os.path.join(base, target.lstrip("/"))) if target.startswith("/") else \
            os.path.normpath(os.path.join(base, target))
        if not os.path.exists(full):
            problems.append(f"{os.path.relpath(base, out) if base != out else 'index.html'}: {href}: missing target")
            return None
        if anchor and full.endswith(".html") and anchor not in anchors_of(full):
            problems.append(f"{href}: missing anchor in {os.path.relpath(full, out)}")
        return full

    landing = open(os.path.join(out, "index.html"), encoding="utf-8").read()
    for href in re.findall(r'\b(?:href|src)="([^"]*)"', landing):
        if href.startswith(EXTERNAL_OK):
            continue
        if href == SITE or href.startswith(SITE + "/"):
            resolve(out, href[len(SITE):] or "/")
            continue
        if re.match(r"^[a-z]+:", href):
            problems.append(f"the landing page links outside the site: {href}")
            continue
        resolve(out, href)

    for root, _, files in os.walk(out):
        for f in files:
            if not f.endswith(".html") or os.path.join(root, f) == os.path.join(out, "index.html"):
                continue
            text = open(os.path.join(root, f), encoding="utf-8").read()
            for href in re.findall(r'\b(?:href|src)="([^"]*)"', text):
                if re.match(r"^[a-z]+:", href):
                    continue
                resolve(root, href)

    for href in re.findall(r"\]\(([^)\s]+)\)", open(os.path.join(out, "llms.txt"), encoding="utf-8").read()):
        resolve(out, href)

    examples = json.load(open(os.path.join(out, "examples.json"), encoding="utf-8"))
    notes = json.load(open(os.path.join(SRC, "notes.json"), encoding="utf-8"))
    embedded = json.loads(re.search(r'<script type="application/json" id="examples">(.*?)</script>', landing, re.S).group(1))
    for entry in notes:
        program = extract_program(entry["source"])
        sha = hashlib.sha256(program.encode("utf-8")).hexdigest()
        if sha != entry["sha256"]:
            problems.append(f"example {entry['id']}: the source program changed; the notes need reviewing")
        pub = next(e for e in examples if e["id"] == entry["id"])
        emb = next(e for e in embedded if e["id"] == entry["id"])
        if pub["program"] != program or emb["program"] != program:
            problems.append(f"example {entry['id']}: the published program is not the source program")

    view = re.search(r'<pre id="source-view".*?</pre>', landing, re.S).group(0)
    shown = [html.unescape(re.sub(r"<[^>]+>", "", m))
             for m in re.findall(r'<span class="line-text"[^>]*>(.*?)</span></span>', view, re.S)]
    mean = next(e for e in examples if e["id"] == "mean")["program"]
    if shown != mean.split("\n"):
        problems.append("the landing page's static source view (shown without JavaScript) is not the mean program; "
                        "the notes need reviewing")

    for name, path in (("the JSON-LD", re.search(r'<script type="application/ld\+json">(.*?)</script>', landing, re.S).group(1)),
                       ("the manifest", open(os.path.join(out, "manifest.webmanifest"), encoding="utf-8").read())):
        try:
            json.loads(path)
        except ValueError as e:
            problems.append(f"{name} does not parse: {e}")

    htmls = {os.path.relpath(os.path.join(r, f), out).replace(os.sep, "/")
             for r, _, fs in os.walk(out) for f in fs if f.endswith(".html")}
    listed = set(re.findall(r"<loc>https://fortis\.picktech\.co\.uk(/[^<]*)?</loc>",
                            open(os.path.join(out, "sitemap.xml"), encoding="utf-8").read()))
    listed = {l.lstrip("/") or "index.html" for l in listed}
    if listed != htmls:
        problems.append(f"the sitemap lists {len(listed)} pages, the site has {len(htmls)}")

    # nothing excluded is published: a planning or internal document fails the check wherever it appears in
    # dist, and every documentation path under the documentation root must be a published page (paths under
    # dist/docs/ mirror repository paths; copied repository files elsewhere are the linked tools and sources)
    for root, _, files in os.walk(out):
        for f in sorted(files):
            rel = os.path.relpath(os.path.join(root, f), out).replace(os.sep, "/")
            repo_rel = rel[len("docs/"):] if rel.startswith("docs/") else rel
            if excluded_path(repo_rel):
                problems.append(f"dist holds an excluded path: {rel} (the site publishes user-facing "
                                f"documentation only)")
            elif repo_rel.startswith("docs/") and repo_rel not in PUBLISHED:
                problems.append(f"dist holds {rel}, which is not a published documentation page")
    return problems


def main(argv):
    checking = "--check" in argv
    examples = load_examples(checking)
    stage = tempfile.mkdtemp(prefix="fortis-website-")
    try:
        write(stage, examples)
        problems = check(stage)
        for p in problems:
            print(f"FAIL {p}")
        if problems:
            if not checking and os.path.isdir(DIST):
                shutil.rmtree(DIST)
            print(f"website: FAILED ({len(problems)} problems); nothing shipped")
            return 1
        if checking:
            print(f"website: check OK ({stage} kept for inspection)" if os.environ.get("WEBSITE_KEEP")
                  else f"website: check OK")
            return 0
        if os.path.isdir(DIST):
            shutil.rmtree(DIST)
        shutil.move(stage, DIST)
        pages = sum(len([f for f in fs if f.endswith('.html')]) for _, _, fs in os.walk(DIST))
        files = sum(len(fs) for _, _, fs in os.walk(DIST))
        size = sum(os.path.getsize(os.path.join(r, f)) for r, _, fs in os.walk(DIST) for f in fs)
        print(f"website: OK ({pages} HTML pages, {files} files, {size / 1e6:.1f} MB in website/dist/)")
        return 0
    finally:
        if os.path.isdir(stage):
            shutil.rmtree(stage, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
