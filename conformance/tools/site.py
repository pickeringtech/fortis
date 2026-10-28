"""DEL-49 criterion 4: the documentation website, built offline from the repository, with search over both modes and
every link checked (PROG-AGT-07).

  python3 -I conformance/tools/site.py [--out DIR] [--json]

The site is built from the same Markdown the repository holds, with nothing fetched and no third-party package:

- pages: `README.md`, the human mode (`docs/guide`), the agent mode (`docs/agent` and the `fortis-code` skill), and
  the specification (`docs/Fortis_*.md`), each rendered to HTML at the same relative path (`.md` becomes `.html`),
  so every relative link in the sources works unchanged;
- Fortis code is highlighted from the language's own lexer (`fortisc view --json`, Programme 9.2);
- citations become links: `LR 7.3` to the Language Reference heading, `LANG-07-05`, `PROG-…` and `PLAT-…` to the
  rule's table row, a diagnostic code to its row in the agent mode's code index; each chapter links to the
  chapter of the other mode that covers the same Language Reference section;
- search: `search.js` holds every section of every page, searched in the browser (both modes and the
  specification, each result labelled with its mode);
- a repository file that a page links to (a tool, a schema, a directory) is copied, or listed, into the site, so
  the link resolves offline.

After writing, every `href` and `src` of every generated page is resolved, including its `#anchor`; an external
link fails the check. The site goes to `build/site/` (open `build/site/index.html`); `--json` writes the build
log to `build/site.json`. The page template's assets are `docs/site/style.css` and `docs/site/site.js`.
"""
from __future__ import annotations

import html
import json
import os
import re
import shutil
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
sys.path.insert(0, HERE)
from cite_check import SPECS, CITE, doc_pages, spec_sections  # noqa: E402

FORTISC = os.path.join(ROOT, "compiler", "target", "release", "fortisc")
ASSETS = os.path.join(ROOT, "docs", "site")
SPEC_PAGES = ["docs/Fortis_Specification.md", "docs/Fortis_Language_Reference.md", "docs/Fortis_Platform_Specification.md",
              "docs/Fortis_Delivery_Programme.md"]
RULE_ID = re.compile(r"\b(LANG-\d\d-\d\d|PROG-[A-Z]{3}-\d\d|PLAT-\d\d-\d\d)\b")
CODE_ID = re.compile(r"\bE-[A-Z]{2,3}-\d{3}\b")


def mode_of(page):
    if page.startswith("docs/guide/"):
        return "human"
    if page.startswith("docs/agent/") or page.startswith("skills/"):
        return "agent"
    if page in SPEC_PAGES:
        return "spec"
    return "home"


def pages():
    """The documentation set (the same recursive inventory the citation and example checks use) and the
    specification."""
    return doc_pages() + [p for p in SPEC_PAGES if os.path.exists(os.path.join(ROOT, p))]


def strip_comments(text):
    """The text with its HTML comments removed outside fenced blocks: only the comment goes, so prose before `<!--`
    or after `-->` on the same line is rendered (and was checked by cite_check, which reads it the same way). A line
    that held only a comment becomes blank."""
    out, fence, in_comment = [], None, False
    for l in text.split("\n"):
        s = l.strip()
        if fence:
            if s == fence:
                fence = None
            out.append(l)
            continue
        if in_comment:
            if "-->" not in l:
                out.append("")
                continue
            l, in_comment = l.split("-->", 1)[1], False
        had = "<!--" in l
        l = re.sub(r"<!--.*?-->", "", l)
        if "<!--" in l:
            l, in_comment = l.split("<!--", 1)[0], True
        if had and not l.strip():
            l = ""
        m = re.match(r"^\s*(`{3,})", l)
        if m:
            fence = m.group(1)
        out.append(l)
    return "\n".join(out)


def slugify(text):
    t = re.sub(r"<[^>]+>", "", text)
    t = html.unescape(t).strip().lower()
    t = re.sub(r"[^\w\- ]", "", t)
    return t.replace(" ", "-")


# ---------------------------------------------------------------------------------------------------------------
# Markdown (the subset the repository's documents use: headings, paragraphs, lists, tables, fenced code,
# block quotes, rules, comments, code spans, links, emphasis)
# ---------------------------------------------------------------------------------------------------------------

class Page:
    def __init__(self, path, linker):
        self.path = path
        self.linker = linker
        self.ids = {}
        self.headings = []        # (level, text, id)
        self.code_blocks = []     # (index, lang, text)
        self.links = []

    def anchor(self, text):
        base = slugify(text)
        n = self.ids.get(base, 0)
        self.ids[base] = n + 1
        return base if n == 0 else f"{base}-{n}"

    # -- inline ---------------------------------------------------------------------------------------------------
    def inline(self, text, autolink=True):
        slots = []

        def keep(s):
            slots.append(s)
            return f"\x00{len(slots) - 1}\x00"

        def code(m):
            body = m.group(2)
            if body.startswith(" ") and body.endswith(" ") and body.strip():
                body = body[1:-1]
            span = f"<code>{html.escape(body.replace(chr(92) + '|', '|'), quote=False)}</code>"
            # a code span that is exactly a rule ID or a diagnostic code links like the bare form
            if autolink and (RULE_ID.fullmatch(body) or CODE_ID.fullmatch(body)):
                inner = []
                self.linker.autolink(self.path, html.escape(body), lambda s: inner.append(s) or "")
                if inner:
                    return keep(re.sub(r">([^<]+)</a>$", lambda mm: ">" + span + "</a>", inner[0]))
            return keep(span)

        text = re.sub(r"(`+)(.+?)(?<!`)\1(?!`)", code, text)
        text = re.sub(r"\\([\\`*_\[\]()#+\-.!|<>{}])", lambda m: keep(html.escape(m.group(1))), text)

        def link(m):
            label, target = m.group(1), m.group(2)
            href = self.linker.href(self.path, target)
            self.links.append(target)
            # the label may hold this call's placeholders (a code span): they are restored with the rest below
            label = html.escape(label, quote=False)
            label = re.sub(r"\*\*(.+?)\*\*", r"<strong>\1</strong>", label)
            return keep(f'<a href="{html.escape(href)}">{label}</a>')

        text = re.sub(r"\[([^\]]+)\]\(([^)\s]+)\)", link, text)
        text = html.escape(text, quote=False)
        if autolink:
            text = self.linker.autolink(self.path, text, keep)
        text = re.sub(r"\*\*(.+?)\*\*", r"<strong>\1</strong>", text)
        text = re.sub(r"(?<![\w*])\*(?!\s)(.+?)(?<!\s)\*(?![\w*])", r"<em>\1</em>", text)
        text = re.sub(r"(?<![\w])_(?!\s)(.+?)(?<!\s)_(?![\w])", r"<em>\1</em>", text)
        for _ in range(3):
            text = re.sub(r"\x00(\d+)\x00", lambda m: slots[int(m.group(1))], text)
        return text

    # -- blocks ---------------------------------------------------------------------------------------------------
    def render(self, text):
        return self.blocks(strip_comments(text).split("\n"))

    def blocks(self, lines):
        out, i, n = [], 0, len(lines)
        while i < n:
            line = lines[i]
            s = line.strip()
            fm = re.match(r"^(\s*)(`{3,})([\w-]*)\s*$", line)
            if fm:
                tick, lang = fm.group(2), fm.group(3)
                j = i + 1
                while j < n and lines[j].strip() != tick:
                    j += 1
                body = "\n".join(l[len(fm.group(1)):] if l.startswith(fm.group(1)) else l for l in lines[i + 1:j])
                idx = len(self.code_blocks)
                self.code_blocks.append((idx, lang, body))
                out.append(f"\x01CODE{idx}\x01")
                i = j + 1
                continue
            if not s:
                i += 1
                continue
            if s.startswith("<!--"):
                while i < n and "-->" not in lines[i]:
                    i += 1
                i += 1
                continue
            hm = re.match(r"^(#{1,6})\s+(.*?)\s*#*\s*$", s)
            if hm:
                level, title = len(hm.group(1)), hm.group(2)
                rendered = self.inline(title, autolink=False)
                hid = self.anchor(rendered)
                self.headings.append((level, re.sub(r"<[^>]+>", "", rendered), hid))
                out.append(f'<h{level} id="{hid}">{rendered}<a class="anchor" href="#{hid}" aria-label="link">#</a></h{level}>')
                i += 1
                continue
            if re.fullmatch(r"(-{3,}|\*{3,}|_{3,})", s):
                out.append("<hr>")
                i += 1
                continue
            if s.startswith("|") and i + 1 < n and re.fullmatch(r"\|?[\s:|-]+\|?", lines[i + 1].strip()) and "-" in lines[i + 1]:
                rows = []
                j = i
                while j < n and lines[j].strip().startswith("|"):
                    if j != i + 1:
                        rows.append(self.cells(lines[j].strip()))
                    j += 1
                out.append(self.table(rows))
                i = j
                continue
            if s.startswith(">"):
                j = i
                quoted = []
                while j < n and lines[j].strip().startswith(">"):
                    quoted.append(re.sub(r"^\s*>\s?", "", lines[j]))
                    j += 1
                out.append(f"<blockquote>{self.blocks(quoted)}</blockquote>")
                i = j
                continue
            lm = re.match(r"^(\s*)([-*+]|\d+[.)])\s+", line)
            if lm:
                html_list, i = self.list_block(lines, i)
                out.append(html_list)
                continue
            para = [s]
            j = i + 1
            while j < n:
                t = lines[j]
                ts = t.strip()
                if not ts or re.match(r"^(#{1,6}\s|`{3,}|>|\|)", ts) or re.match(r"^([-*+]|\d+[.)])\s+", ts) or ts.startswith("<!--"):
                    break
                para.append(ts)
                j += 1
            out.append(f"<p>{self.inline(' '.join(para))}</p>")
            i = j
        return "\n".join(out)

    def list_block(self, lines, i):
        n = len(lines)
        first = re.match(r"^(\s*)([-*+]|\d+[.)])\s+", lines[i])
        indent = len(first.group(1))
        ordered = first.group(2)[0].isdigit()
        items = []
        while i < n:
            m = re.match(r"^(\s*)([-*+]|\d+[.)])\s+(.*)$", lines[i])
            if not m or len(m.group(1)) != indent or m.group(2)[0].isdigit() != ordered:
                break
            content_indent = len(m.group(1)) + len(m.group(2)) + 1
            body = [m.group(3)]
            j = i + 1
            while j < n:
                t = lines[j]
                if not t.strip():
                    # a blank line ends the item unless the next line is indented into it
                    if j + 1 < n and lines[j + 1].strip() and len(lines[j + 1]) - len(lines[j + 1].lstrip()) >= content_indent:
                        body.append("")
                        j += 1
                        continue
                    break
                lead = len(t) - len(t.lstrip())
                nm = re.match(r"^(\s*)([-*+]|\d+[.)])\s+", t)
                if nm and lead <= indent:
                    break
                if lead >= content_indent:
                    body.append(t[content_indent:])
                elif nm:
                    body.append(t[min(lead, content_indent):])
                else:
                    body.append(t.strip())       # a lazy continuation line
                j += 1
            items.append(body)
            i = j
            while i < n and not lines[i].strip() and i + 1 < n and re.match(rf"^\s{{{indent}}}([-*+]|\d+[.)])\s+", lines[i + 1]) and len(lines[i + 1]) - len(lines[i + 1].lstrip()) == indent:
                i += 1
        tag = "ol" if ordered else "ul"
        parts = []
        for body in items:
            inner = self.blocks(body)
            if inner.startswith("<p>") and inner.count("<p>") == 1:
                inner = inner[3:].replace("</p>", "", 1)
            parts.append(f"<li>{inner}</li>")
        return f"<{tag}>" + "".join(parts) + f"</{tag}>", i

    @staticmethod
    def cells(row):
        row = row.strip()
        if row.startswith("|"):
            row = row[1:]
        if row.endswith("|") and not row.endswith("\\|"):
            row = row[:-1]
        cells, cur, i, in_code = [], "", 0, 0
        while i < len(row):
            c = row[i]
            if c == "\\" and i + 1 < len(row) and row[i + 1] == "|":
                cur += "\\|"
                i += 2
                continue
            if c == "|":
                cells.append(cur.strip())
                cur = ""
            else:
                cur += c
            i += 1
        cells.append(cur.strip())
        return cells

    def table(self, rows):
        head, body = rows[0], rows[1:]
        out = ["<div class=\"table\"><table><thead><tr>" + "".join(f"<th>{self.inline(c)}</th>" for c in head) + "</tr></thead><tbody>"]
        for r in body:
            first = re.sub(r"[`*]", "", r[0]) if r else ""
            m = re.fullmatch(r"(LANG-\d\d-\d\d|PROG-[A-Z]{3}-\d\d|PLAT-\d\d-\d\d|E-[A-Z]{2,3}-\d{3}|X-\d+)", first.strip())
            rid = f' id="{m.group(1)}"' if m and m.group(1) not in self.ids else ""
            if m:
                self.ids[m.group(1)] = 1
            out.append(f"<tr{rid}>" + "".join(f"<td>{self.inline(c)}</td>" for c in r) + "</tr>")
        out.append("</tbody></table></div>")
        return "".join(out)


class Linker:
    """Rewrites links between sources to links between pages, and turns citations into links."""

    def __init__(self, page_set, code_page):
        self.page_set = set(page_set)
        self.code_page = code_page
        # the codes the index has a row for (its first cell): only those are linked
        self.code_ids = set()
        if code_page:
            for l in open(os.path.join(ROOT, code_page), encoding="utf-8"):
                m = re.match(r"^\|\s*`?(E-[A-Z]{2,3}-\d{3})`?\s*\|", l)
                if m:
                    self.code_ids.add(m.group(1))
        self.sections = {doc: spec_sections(path) for doc, path in SPECS.items()}
        self.copies = set()

    def rel(self, page, target_page, anchor=""):
        here = os.path.dirname(page)
        url = os.path.relpath(target_page, here or ".").replace(os.sep, "/")
        return url + (f"#{anchor}" if anchor else "")

    def href(self, page, target):
        if re.match(r"^[a-z]+:", target):
            return target
        path, _, anchor = target.partition("#")
        if not path:
            return f"#{anchor}"
        full = os.path.normpath(os.path.join(os.path.dirname(page), path))
        if full.endswith(".md") and full in self.page_set:
            return self.rel(page, full[:-3] + ".html", anchor)
        if os.path.isdir(os.path.join(ROOT, full)):
            self.copies.add(full + "/")
            return self.rel(page, os.path.join(full, "index.html"), anchor)
        self.copies.add(full)
        return self.rel(page, full, anchor)

    def autolink(self, page, text, keep):
        def cite(m):
            doc = m.group(1)
            path = SPECS[doc]
            parts = re.split(r"(,\s*|\s+and\s+)", m.group(2))
            out = []
            for k, part in enumerate(parts):
                if k % 2:
                    out.append(part)
                    continue
                sec = self.sections[doc].get(part)
                label = f"{doc} {part}" if k == 0 else part
                if sec and page != path:
                    out.append(keep(f'<a class="cite" href="{self.rel(page, path[:-3] + ".html", sec[2])}">{label}</a>'))
                else:
                    out.append(label)
            return "".join(out)

        text = CITE.sub(cite, text)

        def rule(m):
            rid = m.group(1)
            doc = {"LANG": "docs/Fortis_Language_Reference.md", "PROG": "docs/Fortis_Delivery_Programme.md", "PLAT": "docs/Fortis_Platform_Specification.md"}[rid.split("-")[0]]
            if doc == page:
                return keep(f'<a class="rule" href="#{rid}">{rid}</a>')
            return keep(f'<a class="rule" href="{self.rel(page, doc[:-3] + ".html", rid)}">{rid}</a>')

        text = RULE_ID.sub(rule, text)
        if self.code_page:
            def code(m):
                c = m.group(0)
                if c not in self.code_ids:
                    return c
                if page == self.code_page:
                    return keep(f'<a class="code" href="#{c}">{c}</a>')
                return keep(f'<a class="code" href="{self.rel(page, self.code_page[:-3] + ".html", c)}">{c}</a>')
            text = CODE_ID.sub(code, text)
        return text


# ---------------------------------------------------------------------------------------------------------------
# highlighting from the language's own lexer
# ---------------------------------------------------------------------------------------------------------------

def highlight_all(blocks, work):
    """{block key: highlighted HTML} for every fortis block, from one `fortisc view --json` over all of them."""
    os.makedirs(work, exist_ok=True)
    paths = {}
    for key, text in blocks.items():
        p = os.path.join(work, f"{key}.ft")
        open(p, "w", encoding="utf-8").write(text + "\n")
        paths[os.path.abspath(p)] = key
    out = {}
    if not paths or not os.path.exists(FORTISC):
        return out
    r = subprocess.run([FORTISC, "view", "--json"] + sorted(paths), capture_output=True, text=True, cwd=ROOT)
    if r.returncode != 0:
        return out                     # nothing is highlighted; the check then fails on the count
    lines, texts = {}, {}
    for l in r.stdout.splitlines():
        try:
            rec = json.loads(l)
        except ValueError:
            continue
        if rec.get("kind") != "line":
            continue
        key = paths.get(os.path.abspath(os.path.join(ROOT, rec["path"])))
        if key is None:
            continue
        text, spans = rec["text"], rec["spans"]
        parts, col = [], 1
        for start, length, cls in spans:
            if start > col:
                parts.append(html.escape(text[col - 1:start - 1], quote=False))
            parts.append(f'<span class="t-{cls}">{html.escape(text[start - 1:start - 1 + length], quote=False)}</span>')
            col = start + length
        parts.append(html.escape(text[col - 1:], quote=False))
        lines.setdefault(key, []).append((rec["line"], "".join(parts)))
        texts.setdefault(key, []).append((rec["line"], text))
    for key, ls in lines.items():
        # only a block the viewer gave back whole, line for line, is replaced by its highlighting
        if [t for _, t in sorted(texts[key])] == blocks[key].split("\n"):
            out[key] = "\n".join(t for _, t in sorted(ls))
    return out


# ---------------------------------------------------------------------------------------------------------------
# the site
# ---------------------------------------------------------------------------------------------------------------

def order(index_page, members):
    """Pages in the order the mode's index links them; the rest alphabetically."""
    text = open(os.path.join(ROOT, index_page), encoding="utf-8").read()
    seen = [index_page]
    for t in re.findall(r"\]\(([^)#\s]+\.md)", text):
        p = os.path.normpath(os.path.join(os.path.dirname(index_page), t))
        if p in members and p not in seen:
            seen.append(p)
    return seen + sorted(p for p in members if p not in seen)


def covers(page):
    text = open(os.path.join(ROOT, page), encoding="utf-8").read()
    m = re.search(r"<!--\s*(?:stub:\s*[\w-]+\s*;\s*)?covers:\s*(.*?)-->", text)
    if not m:
        return []
    return [s for d, s in ((m2.group(1), m2.group(2)) for m2 in CITE.finditer(m.group(1))) for s in re.split(r",\s*|\s+and\s+", s) if d == "LR"]


def title_of(page):
    for l in open(os.path.join(ROOT, page), encoding="utf-8"):
        if l.startswith("# "):
            return re.sub(r"[`*]", "", l[2:].strip())
        if l.startswith("name: "):
            return l[6:].strip()
    return os.path.basename(page)


def render_site(out_dir):
    all_pages = pages()
    code_page = "docs/agent/codes.md" if "docs/agent/codes.md" in all_pages else None
    linker = Linker(all_pages, code_page)
    human = order("docs/guide/README.md", [p for p in all_pages if mode_of(p) == "human"]) if os.path.exists(os.path.join(ROOT, "docs/guide/README.md")) else []
    agent = order("docs/agent/README.md", [p for p in all_pages if mode_of(p) == "agent"]) if os.path.exists(os.path.join(ROOT, "docs/agent/README.md")) else []
    spec = [p for p in all_pages if mode_of(p) == "spec"]
    nav = {"human": human, "agent": agent, "spec": spec, "home": ["README.md"]}
    # counterpart chapters: the other mode's page that covers the same first section
    cover = {p: covers(p) for p in human + agent}
    rendered, index, fortis_blocks = {}, [], {}
    for page in all_pages:
        text = open(os.path.join(ROOT, page), encoding="utf-8").read()
        if page.endswith("SKILL.md"):
            text = re.sub(r"^---\n.*?\n---\n", "", text, flags=re.S)
        pg = Page(page, linker)
        body = pg.render(text)
        rendered[page] = (pg, body)
        for idx, lang, code in pg.code_blocks:
            if lang == "fortis":
                fortis_blocks[f"{slugify(page)}-{idx}"] = code
    hl = highlight_all(fortis_blocks, os.path.join(ROOT, "build", "site-work"))
    commit = subprocess.run(["git", "rev-parse", "--short=12", "HEAD"], capture_output=True, text=True, cwd=ROOT).stdout.strip()
    if os.path.isdir(out_dir):
        shutil.rmtree(out_dir)
    os.makedirs(out_dir)
    for page, (pg, body) in rendered.items():
        def code_html(m):
            idx = int(m.group(1))
            _, lang, code = pg.code_blocks[idx]
            key = f"{slugify(page)}-{idx}"
            inner = hl.get(key) if lang == "fortis" else None
            if inner is None:
                inner = html.escape(code, quote=False)
            label = {"fortis": "Fortis", "output": "output", "records": "records", "yaml": "YAML"}.get(lang, lang)
            first = code.split("\n", 1)[0]
            kind = ""
            if lang == "fortis":
                kind = " accept" if first.startswith("// accept") else " reject" if first.startswith("// reject") else " illustrative"
            return f'<div class="code{kind}"><span class="lang">{html.escape(label)}</span><pre><code>{inner}</code></pre></div>'
        body = re.sub(r"\x01CODE(\d+)\x01", code_html, body)
        mode = mode_of(page)
        counterpart = ""
        if mode in ("human", "agent") and cover.get(page):
            other = agent if mode == "human" else human
            match = next((o for o in other if cover.get(o) and cover[o][0] == cover[page][0]), None)
            if match:
                counterpart = f'<a class="counterpart" href="{linker.rel(page, match[:-3] + ".html")}">{"Agent mode" if mode == "human" else "Human mode"}: {html.escape(title_of(match))}</a>'
        html_page = template(page, mode, title_of(page), body, nav, linker, counterpart, commit)
        dest = os.path.join(out_dir, page[:-3] + ".html")
        os.makedirs(os.path.dirname(dest), exist_ok=True)
        open(dest, "w", encoding="utf-8").write(html_page)
        # the search index: one entry per section
        sections = re.split(r'(?=<h[1-3] id=")', body)
        for sec in sections:
            hm = re.match(r'<h[1-3] id="([^"]+)">(.*?)<a class="anchor"', sec)
            plain = html.unescape(re.sub(r"<[^>]+>", " ", sec))
            plain = re.sub(r"\s+", " ", plain).strip()
            if not plain:
                continue
            heading = html.unescape(re.sub(r"<[^>]+>", "", hm.group(2))) if hm else title_of(page)
            # the whole section is searchable: a long one is indexed in pieces under the same heading
            for k in range(0, max(len(plain), 1), 3900):    # pieces overlap, so no word is split unfound
                index.append({"u": page[:-3] + ".html" + (f"#{hm.group(1)}" if hm else ""), "p": title_of(page), "h": heading, "m": mode, "x": plain[k:k + 4000]})
    # assets, the landing page, the index and the linked repository files
    for f in ("style.css", "site.js"):
        shutil.copy(os.path.join(ASSETS, f), os.path.join(out_dir, f))
    open(os.path.join(out_dir, "search.js"), "w", encoding="utf-8").write("window.FORTIS_SEARCH = " + json.dumps(index, separators=(",", ":")) + ";\n")
    open(os.path.join(out_dir, "index.html"), "w", encoding="utf-8").write(landing(nav, commit))
    for path in sorted(linker.copies):
        src = os.path.join(ROOT, path.rstrip("/"))
        if path.endswith("/"):
            d = os.path.join(out_dir, path)
            os.makedirs(d, exist_ok=True)
            items = sorted(os.listdir(src))
            rows = "".join(f"<li><code>{html.escape(i)}{'/' if os.path.isdir(os.path.join(src, i)) else ''}</code></li>" for i in items if not i.startswith("."))
            open(os.path.join(d, "index.html"), "w", encoding="utf-8").write(
                f'<!doctype html><html lang="en"><head><meta charset="utf-8"><title>{html.escape(path)}</title></head>'
                f'<body><h1><code>{html.escape(path)}</code></h1><p>A directory of the repository.</p><ul>{rows}</ul></body></html>')
        elif os.path.isfile(src):
            dest = os.path.join(out_dir, path)
            os.makedirs(os.path.dirname(dest), exist_ok=True)
            shutil.copy(src, dest)
    return all_pages, len(index), len(fortis_blocks), len(hl)


def nav_html(page, nav, linker):
    def group(name, label, members):
        items = "".join(f'<li{" class=current" if p == page else ""}><a href="{linker.rel(page, p[:-3] + ".html")}">{html.escape(title_of(p))}</a></li>' for p in members)
        return f'<details{" open" if mode_of(page) == name else ""}><summary>{label}</summary><ul>{items}</ul></details>'
    return group("human", "Guide (human mode)", nav["human"]) + group("agent", "Agent mode", nav["agent"]) + group("spec", "Specification", nav["spec"])


def template(page, mode, title, body, nav, linker, counterpart, commit):
    root = os.path.relpath(".", os.path.dirname(page) or ".").replace(os.sep, "/")
    root = "." if root == "." else root
    badge = {"human": "Human mode", "agent": "Agent mode", "spec": "Specification", "home": "Fortis"}[mode]
    return f"""<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{html.escape(title)} · Fortis</title>
<link rel="stylesheet" href="{root}/style.css">
</head>
<body class="mode-{mode}" data-root="{root}">
<header class="top">
  <a class="brand" href="{root}/index.html">Fortis</a>
  <span class="badge">{badge}</span>
  <input id="search" type="search" placeholder="Search both modes" autocomplete="off" aria-label="Search">
</header>
<div id="results" hidden></div>
<div class="layout">
<nav class="side">{nav_html(page, nav, linker)}</nav>
<main>
{('<p class="mode-switch">' + counterpart + '</p>') if counterpart else ''}
{body}
<footer>Built from the repository at {html.escape(commit or 'a working tree')} by <code>conformance/tools/site.py</code>; every example on this page is checked by <code>conformance/tools/docset_check.py</code>.</footer>
</main>
</div>
<script src="{root}/search.js"></script>
<script src="{root}/site.js"></script>
</body>
</html>
"""


def landing(nav, commit):
    def first(members, fallback):
        return (members[0][:-3] + ".html") if members else fallback
    return f"""<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Fortis documentation</title>
<link rel="stylesheet" href="./style.css">
</head>
<body class="mode-home" data-root=".">
<header class="top">
  <a class="brand" href="./index.html">Fortis</a>
  <span class="badge">Documentation</span>
  <input id="search" type="search" placeholder="Search both modes" autocomplete="off" aria-label="Search">
</header>
<div id="results" hidden></div>
<main class="landing">
<h1>Fortis</h1>
<p class="lede">A statically checked, compiled language designed so that code written mostly by agents can be read, reviewed and trusted by people.</p>
<div class="cards">
<a class="card human" href="{first(nav['human'], 'README.html')}"><h2>Guide</h2><p>Human mode: the language explained chapter by chapter, with every example run on three backends.</p></a>
<a class="card agent" href="{first(nav['agent'], 'README.html')}"><h2>Agent mode</h2><p>Rules by ID, diagnostic codes, record schemas and correction patterns, for agents writing Fortis.</p></a>
<a class="card spec" href="docs/Fortis_Language_Reference.html"><h2>Language Reference</h2><p>The source of truth: every rule, numbered, with its evidence.</p></a>
</div>
<p><a href="README.html">The repository README</a> · built at {html.escape(commit or 'a working tree')}</p>
</main>
<script src="./search.js"></script>
<script src="./site.js"></script>
</body>
</html>
"""


def check_links(out_dir):
    """[(page, href, problem)] for every link and asset of every generated page."""
    problems, ids_cache = [], {}

    def ids_of(path):
        if path not in ids_cache:
            try:
                text = open(path, encoding="utf-8").read()
            except (OSError, UnicodeDecodeError):
                text = ""
            ids_cache[path] = set(re.findall(r'\bid="([^"]+)"', text))
        return ids_cache[path]

    n = 0
    for root, _, files in os.walk(out_dir):
        for f in files:
            if not f.endswith(".html"):
                continue
            path = os.path.join(root, f)
            text = open(path, encoding="utf-8").read()
            for href in re.findall(r'\b(?:href|src)="([^"]*)"', text):
                n += 1
                href = html.unescape(href)
                if re.match(r"^[a-z]+:", href):
                    problems.append((os.path.relpath(path, out_dir), href, "external link"))
                    continue
                target, _, anchor = href.partition("#")
                full = os.path.normpath(os.path.join(root, target)) if target else path
                if not os.path.exists(full):
                    problems.append((os.path.relpath(path, out_dir), href, "missing target"))
                elif anchor and full.endswith(".html") and anchor not in ids_of(full):
                    problems.append((os.path.relpath(path, out_dir), href, "missing anchor"))
    return problems, n


def main(argv):
    out_dir = os.path.join(ROOT, "build", "site")
    if "--out" in argv:
        out_dir = os.path.abspath(argv[argv.index("--out") + 1])
    built, sections, blocks, highlighted = render_site(out_dir)
    problems, n = check_links(out_dir)
    # every page of the documentation set is rendered and searchable
    missing = [p for p in doc_pages() if not os.path.exists(os.path.join(out_dir, p[:-3] + ".html"))]
    search = open(os.path.join(out_dir, "search.js"), encoding="utf-8").read()
    unindexed = [p for p in doc_pages() if f'"u":"{p[:-3]}.html' not in search]
    problems += [(p, "", "a page of the documentation set was not rendered") for p in missing]
    problems += [(p, "", "a page of the documentation set is not in the search index") for p in unindexed]
    # the specification's own references to outside sources are not this site's links: report them separately
    spec_external = [p for p in problems if p[2] == "external link" and p[0].startswith("docs/Fortis_")]
    problems = [p for p in problems if p not in spec_external]
    for page, href, why in problems:
        print(f"FAIL {page}: {href}: {why}")
    print(f"site: {len(built)} pages, {sections} searchable sections, {blocks} Fortis blocks ({highlighted} highlighted by fortisc view); "
          f"{n} links checked, {len(problems)} broken, {len(spec_external)} external links in the specification (not followed)")
    if "--json" in argv:
        json.dump({"rule": "PROG-AGT-07", "chunk": "DEL-49", "out": os.path.relpath(out_dir, ROOT), "pages": built, "sections": sections,
                   "links": n, "broken": [{"page": p, "href": h, "problem": w} for p, h, w in problems],
                   "spec_external": [{"page": p, "href": h} for p, h, _ in spec_external]},
                  open(os.path.join(ROOT, "build", "site.json"), "w", encoding="utf-8"), indent=1)
    ok = not problems and blocks == highlighted
    if blocks != highlighted:
        print(f"FAIL {blocks - highlighted} Fortis blocks were not highlighted by fortisc view")
    print("site: " + ("OK" if ok else "FAILED") + f" ({os.path.relpath(out_dir, ROOT)}/index.html)")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
