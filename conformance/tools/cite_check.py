"""DEL-49 criteria 1 and 3: the documentation set's coverage of the Language Reference, and its citations
(PROG-AGT-07).

  python3 -I conformance/tools/cite_check.py [--json] [--release] [-v]

**Citations.** Over `README.md`, `docs/guide`, `docs/agent` and the `fortis-code` skill:

- every section citation resolves: `LR 7.3`, `LR C.2`, `LR A` (the Language Reference), `Platform 3.1` and
  `Programme 7.4` name a heading of that document; a comma or `and` list continues one citation
  (`LR 7.3, 7.5 and 7.6`); a Markdown link into a specification document names an anchor that exists;
- every rule ID (`LANG-07-05`, `PROG-TST-08`, `PLAT-03-01`), diagnostic code (`E-OWN-011`), register row
  (`X-137`) and defect (`D-30`) resolves to the specification, the decision register or the defect register;
  the Delivery Programme and the two registers are maintainers' planning files (planning.py), so in a checkout
  without them those citations and IDs are not resolved, and the check says so;
- a paragraph, list item or table row that names a diagnostic code or a `LANG-` rule cites a section, in itself
  or in the heading it sits under, and every code it names occurs in one of the sections it cites (so a page
  cannot attach a code to a rule the cited section does not state), and every `LANG-` rule it names belongs to
  a cited Language Reference section;
- every `##` section of a chapter cites at least one section of the specification, unless it carries
  `<!-- no-citation: reason -->`.

**Coverage.** Every chapter starts with `<!-- covers: LR 7 -->` (a list of Language Reference sections), or
`<!-- stub: SP-18; covers: … -->` for a chapter that waits for a spike. Every top-level Language Reference section
(0-16 and appendices A-E) must have a chapter in each mode (`docs/guide` is the human mode, `docs/agent` the
agent mode), and every subsection must be cited in each mode. A stub chapter, and a section reached only through
stubs, is reported as pending: the run exits 0 with pending items (the merge gate), and with `--release` (DEL-49
criteria 1 and 3 at the release check) it exits 1, since criterion 1 does not accept a stub.

The report goes to `build/cite.json` (`--json`).
"""
from __future__ import annotations

import json
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
sys.path.insert(0, HERE)
import planning  # noqa: E402

SPECS = {
    "LR": "docs/Fortis_Language_Reference.md",
    "Platform": "docs/Fortis_Platform_Specification.md",
    "Programme": planning.PROGRAMME,
}
REGISTER = planning.REGISTER
CODE = re.compile(r"\b[EW]-[A-Z]{2,4}-\d{3}\b")
# the IDs the release check counts (DW-11), exactly as it matches them
RELEASE_ID = re.compile(r"\b(?:LANG-\d\d-\d\d|PROG-[A-Z]{3}-\d\d|PLAT-\d\d-\d\d|[EW]-[A-Z]{2,4}-\d{3})\b")
RULE = re.compile(r"\b(?:LANG-\d\d-\d\d|PROG-[A-Z]{3}-\d\d|PLAT-\d\d-\d\d)\b")
XROW = re.compile(r"\bX-\d{2,3}\b")
DEF = re.compile(r"\bD-\d{1,3}\b")
SECNUM = r"(?:\d{1,2}(?:\.\d{1,2})?|[A-E](?:\.\d)?)"
CITE = re.compile(rf"\b(LR|Platform|Programme)\s+({SECNUM}(?:(?:,\s*|\s+and\s+){SECNUM}(?![\w-]))*)(?![\w.-]*\w)")

SITE = "https://fortis.picktech.co.uk"  # the documentation set, published (website/); the one outside address it may link to


# The sections that state each LANG rule. The Language Reference collects a section's rules in a table at its end,
# so the table gives only the chapter; this map gives the subsections whose text the rule summarises, so a page that
# names LANG-07-08 must cite LR 7.8 (or the whole of LR 7), not LR 7.3. Every rule of the Language Reference must
# have an entry: a new rule fails the check until it is mapped.
RULE_SECTIONS = {
    "LANG-03-01": ["3.2"], "LANG-03-02": ["3.2"], "LANG-03-03": ["3.3"], "LANG-03-04": ["3.4", "A"],
    "LANG-03-05": ["3.5"], "LANG-03-06": ["3.6"],
    "LANG-04-01": ["4.2"], "LANG-04-02": ["4.2"], "LANG-04-03": ["4.3", "4.4"], "LANG-04-04": ["4.1"],
    "LANG-04-05": ["4.5"], "LANG-04-06": ["4.6"],
    "LANG-05-01": ["5.1"], "LANG-05-02": ["5.1", "7.9"], "LANG-05-03": ["5.2", "5.3"], "LANG-05-04": ["5.3", "6.3"],
    "LANG-05-05": ["5.4"], "LANG-05-06": ["5.6"], "LANG-05-07": ["5.7"], "LANG-05-08": ["5.5", "6.12", "12.3"],
    "LANG-05-09": ["5.8"],
    "LANG-06-01": ["6.1", "6.9"], "LANG-06-02": ["6.3"], "LANG-06-03": ["6.3", "6.9"], "LANG-06-04": ["6.4", "6.5"],
    "LANG-06-05": ["6.5", "6.3"], "LANG-06-06": ["6.7"], "LANG-06-07": ["6.11"], "LANG-06-08": ["6.8", "6.1"],
    "LANG-06-09": ["6.12"], "LANG-06-10": ["6.10", "9.5"], "LANG-06-11": ["6.10"],
    "LANG-07-01": ["7.5"], "LANG-07-02": ["7.6"], "LANG-07-03": ["7.10", "C.1"], "LANG-07-04": ["7.3", "7.4", "7.5"],
    "LANG-07-05": ["7.3"], "LANG-07-06": ["7.6", "7.7"], "LANG-07-07": ["7.9", "12.2"], "LANG-07-08": ["7.8"],
    "LANG-07-09": ["7.10", "C.2"],
    "LANG-08-01": ["8.1"], "LANG-08-02": ["8.2", "8.3"], "LANG-08-03": ["8.3"], "LANG-08-04": ["8.4"],
    "LANG-09-01": ["9.1", "9.2"], "LANG-09-02": ["9.2", "4.4"], "LANG-09-03": ["9.5"], "LANG-09-04": ["9.6"],
    "LANG-09-05": ["9.8", "9.7"], "LANG-09-06": ["9.8"],
    "LANG-10-01": ["10.1", "10.2"], "LANG-10-02": ["10.2", "10.7", "6.12"], "LANG-10-03": ["10.6"],
    "LANG-10-04": ["10.3", "11.4"], "LANG-10-05": ["10.4", "10.5"], "LANG-10-06": ["10.8"],
    "LANG-11-01": ["11.1", "11.3"], "LANG-11-02": ["11.1", "11.3"], "LANG-11-03": ["11.4", "C.3"], "LANG-11-04": ["11.3"],
    "LANG-11-05": ["11.2", "11.3", "8.1"], "LANG-11-06": ["11.5"], "LANG-11-07": ["11.6"], "LANG-11-08": ["11.5", "11.8"],
    "LANG-11-09": ["11.8"],
    "LANG-12-01": ["12.1"], "LANG-12-02": ["12.1"], "LANG-12-03": ["12.3", "12.4"], "LANG-12-04": ["12.5"],
    "LANG-12-05": ["12.5", "12.7", "7.7"], "LANG-12-06": ["12.7", "12.3"],
    "LANG-13-01": ["13"], "LANG-13-02": ["13"], "LANG-13-03": ["13"], "LANG-13-04": ["13"],
    "LANG-14-01": ["14"], "LANG-14-02": ["14"], "LANG-14-03": ["14"],
}


def states(cited, rule):
    """Whether citing LR `cited` cites a section that states `rule`: one of its mapped sections (or a part of one),
    or the whole chapter whose table lists it."""
    chapter = str(int(rule.split("-")[1]))
    if cited == chapter:
        return True
    return any(cited == s or cited.startswith(s + ".") for s in RULE_SECTIONS.get(rule, []))


def github_anchor(title):
    """GitHub's anchor for a heading: lowercase, punctuation removed except `-` and `_`, spaces to `-`."""
    t = title.strip().lower()
    t = re.sub(r"[^\w\- ]", "", t)
    return t.replace(" ", "-")


def spec_sections(path):
    """{section id: (title, text including its subsections, anchor)} of one specification document."""
    lines = open(os.path.join(ROOT, path), encoding="utf-8").read().split("\n")
    heads = []
    for i, l in enumerate(lines):
        m = re.match(r"^##\s+(\d+)\.\s+(.*)$", l) or re.match(r"^##\s+Appendix\s+([A-Z])\.\s+(.*)$", l)
        if m:
            heads.append((i, 2, m.group(1), l.lstrip("# ").strip()))
            continue
        m = re.match(r"^###\s+((?:\d+|[A-Z])\.\d+)\s+(.*)$", l)
        if m:
            heads.append((i, 3, m.group(1), l.lstrip("# ").strip()))
            continue
        m = re.match(r"^(#{2,3})\s", l)
        if m:
            heads.append((i, len(m.group(1)), None, l.lstrip("# ").strip()))
    out = {}
    for k, (i, level, sid, title) in enumerate(heads):
        end = len(lines)
        for j, lv, _, _ in heads[k + 1:]:
            if lv <= level:
                end = j
                break
        if sid:
            out[sid] = (title, "\n".join(lines[i:end]), github_anchor(title))
    return out


def anchors(path):
    text = open(os.path.join(ROOT, path), encoding="utf-8").read()
    return {github_anchor(l.lstrip("#").strip()) for l in text.split("\n") if re.match(r"^#{1,6}\s", l)}


def present_specs(tool="cite"):
    """SPECS without the documents this checkout does not have (the Delivery Programme is a planning file); a
    citation of a missing document is then not resolved, and `tool` says so once."""
    out = {}
    for name, path in SPECS.items():
        if name == "Programme" and not planning.optional(path, tool, "resolving `Programme n.n` citations and PROG- rule IDs"):
            continue
        out[name] = path
    return out


def known_ids():
    """The IDs each family resolves to; None for a family whose planning file this checkout does not have."""
    ids = {"LANG": set(), "PROG": set(), "PLAT": set(), "codes": set(), "X": set(), "D": set()}
    specs = present_specs()
    if "Programme" not in specs:
        ids["PROG"] = None
    for name, path in specs.items():
        text = open(os.path.join(ROOT, path), encoding="utf-8").read()
        for m in re.finditer(r"^\|\s*((?:LANG|PROG|PLAT)-[A-Z0-9]+-\d+)\s*\|", text, re.M):
            if ids[m.group(1).split("-")[0]] is not None:
                ids[m.group(1).split("-")[0]].add(m.group(1))
        ids["codes"] |= set(CODE.findall(text))
    reg = planning.optional(REGISTER, "cite", "resolving register rows (X-nn)")
    ids["X"] = set(re.findall(r"^\|\s*(X-\d+)\s*\|", open(reg, encoding="utf-8").read(), re.M)) if reg else None
    defects = planning.optional(planning.DEFECTS, "cite", "resolving defect IDs (D-nn)")
    ids["D"] = {d["id"] for d in json.load(open(defects, encoding="utf-8"))["defects"]} if defects else None
    return ids


def doc_pages():
    pages = ["README.md"]
    for d in ("docs/guide", "docs/agent"):
        for root, _, files in sorted(os.walk(os.path.join(ROOT, d))):
            for f in sorted(files):
                if f.endswith(".md"):
                    pages.append(os.path.relpath(os.path.join(root, f), ROOT))
    pages.append("skills/fortis-code/SKILL.md")
    return [p for p in pages if os.path.exists(os.path.join(ROOT, p))]


def release_scope():
    """The files the release check counts citations in: README.md, docs/guide/** and docs/agent/** (.md, .html,
    .txt), code blocks and comments included."""
    files = ["README.md"]
    for d in ("docs/guide", "docs/agent"):
        for root, _, fs in sorted(os.walk(os.path.join(ROOT, d))):
            files += [os.path.relpath(os.path.join(root, f), ROOT) for f in sorted(fs) if f.endswith((".md", ".html", ".txt"))]
    return [f for f in files if os.path.exists(os.path.join(ROOT, f))]


def mode_of(page):
    if page.startswith("docs/guide/"):
        return "human"
    if page.startswith("docs/agent/"):
        return "agent"
    return "other"


def units(text):
    """(heading, line, text) for every paragraph, list item and table row outside fenced blocks and comments."""
    out, heading, h2, buf, start = [], "", "", [], 0
    fence = None
    in_comment = False

    def flush():
        nonlocal buf
        if buf:
            out.append((heading, h2, start, " ".join(buf)))
        buf = []

    for i, l in enumerate(text.split("\n"), 1):
        s = l.strip()
        if fence:
            if s == fence:
                fence = None
            continue
        # only the comment itself is hidden: text before `<!--` or after `-->` on the same line is prose and is
        # checked like any other (a reader sees it)
        if in_comment:
            if "-->" not in l:
                continue
            l, in_comment = l.split("-->", 1)[1], False
        had_comment = "<!--" in l
        l = re.sub(r"<!--.*?-->", " ", l)
        if "<!--" in l:
            l, in_comment = l.split("<!--", 1)[0], True
        s = l.strip()
        if had_comment and not s:
            flush()
            continue
        m = re.match(r"^(`{3,})", s)
        if m:
            flush()
            fence = m.group(1)
            continue
        hm = re.match(r"^(#{1,6})\s+(.*)$", s)
        if hm:
            flush()
            heading = hm.group(2)
            if len(hm.group(1)) <= 2:
                h2 = heading
            out.append(("heading", h2, i, heading))
            continue
        if not s:
            flush()
            continue
        if s.startswith("|") or re.match(r"^([-*]|\d+\.)\s", s):
            flush()
            if s.startswith("|") and re.fullmatch(r"\|[\s:|-]+\|?", s):
                continue
            start = i
            buf = [s]
            if s.startswith("|"):
                flush()
            continue
        if not buf:
            start = i
        buf.append(s)
    flush()
    return out


def citations(text):
    """[(document, section id)] cited in the text."""
    out = []
    for m in CITE.finditer(text):
        doc = m.group(1)
        for sid in re.split(r",\s*|\s+and\s+", m.group(2)):
            out.append((doc, sid.strip()))
    return out


def link_targets(text):
    """Markdown link targets, outside code spans (where `f[Int](x)` is code, not a link)."""
    return re.findall(r"\]\(([^)\s]+)\)", re.sub(r"(`+).+?\1", "", text))


def main(argv):
    verbose = "-v" in argv
    ids = known_ids()
    specs = present_specs()
    sections = {doc: spec_sections(path) for doc, path in specs.items()}
    spec_anchor = {path: anchors(path) for path in specs.values()}
    problems, cited = [], {"human": {}, "agent": {}, "other": {}}
    covers = {"human": {}, "agent": {}}
    pages = doc_pages()
    # the rule map covers exactly the Language Reference's rules, and names only sections that exist
    for r in sorted(ids["LANG"] - set(RULE_SECTIONS)):
        problems.append(("conformance/tools/cite_check.py", 0, f"{r} is in the Language Reference but not in RULE_SECTIONS"))
    for r, secs in sorted(RULE_SECTIONS.items()):
        if r not in ids["LANG"]:
            problems.append(("conformance/tools/cite_check.py", 0, f"RULE_SECTIONS names {r}, which the Language Reference does not have"))
        for s in secs:
            if s not in sections["LR"]:
                problems.append(("conformance/tools/cite_check.py", 0, f"RULE_SECTIONS maps {r} to LR {s}, which does not exist"))
    for page in pages:
        text = open(os.path.join(ROOT, page), encoding="utf-8").read()
        mode = mode_of(page)
        cm = re.search(r"<!--\s*(?:stub:\s*([\w-]+)\s*;\s*)?covers:\s*(.*?)-->", text)
        if mode in covers:
            if not cm:
                problems.append((page, 1, "a chapter must start with `<!-- covers: LR … -->` (or `<!-- stub: SP-nn; covers: … -->`)"))
            else:
                for doc, sid in citations(cm.group(2)):
                    if doc == "LR":
                        covers[mode].setdefault(sid, []).append((page, cm.group(1)))
        # sections of the page: every `##` section must cite (unless it says why not)
        sec_cites, sec_start, sec_name, no_cite = 0, 0, None, False
        h2_cites = {}
        for heading, h2, line, body in units(text):
            if heading == "heading":
                if line and re.match(r"^#{1,2}\s", text.split("\n")[line - 1]):
                    if sec_name is not None and not sec_cites and not no_cite and mode in ("human", "agent") and not page.endswith("README.md"):
                        problems.append((page, sec_start, f"section `{sec_name}` cites no section of the specification (cite one, or add `<!-- no-citation: reason -->`)"))
                    sec_name, sec_start, sec_cites, no_cite = body, line, 0, False
                    sec_name = None if re.match(r"^#\s", text.split("\n")[line - 1]) else body
                h2_cites[body] = citations(body)
                # a heading is then validated like any other text: its citations, IDs, codes and links
                heading = body
            sec_cites += len(citations(body))
            for doc, sid in citations(body):
                if doc not in sections:
                    continue            # a document this checkout does not have (said once, above)
                if sid not in sections[doc]:
                    problems.append((page, line, f"`{doc} {sid}` names no section of {SPECS[doc]}"))
                else:
                    cited[mode].setdefault((doc, sid), []).append(f"{page}:{line}")
            for r in RULE.findall(body):
                if ids[r.split("-")[0]] is not None and r not in ids[r.split("-")[0]]:
                    problems.append((page, line, f"rule {r} is not in the specification"))
            for c in CODE.findall(body):
                if c not in ids["codes"]:
                    problems.append((page, line, f"diagnostic code {c} is named nowhere in the specification"))
            for x in XROW.findall(body):
                if ids["X"] is not None and x not in ids["X"]:
                    problems.append((page, line, f"register row {x} does not exist"))
            for d in DEF.findall(body):
                if ids["D"] is not None and d not in ids["D"]:
                    problems.append((page, line, f"defect {d} is not in the defect register"))
            for target in link_targets(body):
                # The published website is these same documents rendered, so a link to it is not an outside dependency.
                if target.rstrip("/") == SITE:
                    continue
                if target.startswith(("http://", "https://", "mailto:")):
                    problems.append((page, line, f"an external link ({target}): the documentation set links only inside the repository"))
                    continue
                path, _, anchor = target.partition("#")
                full = os.path.normpath(os.path.join(os.path.dirname(os.path.join(ROOT, page)), path)) if path else os.path.join(ROOT, page)
                rel = os.path.relpath(full, ROOT)
                if not os.path.exists(full):
                    problems.append((page, line, f"link to {target}: {rel} does not exist"))
                elif anchor and rel.endswith(".md") and anchor not in anchors(rel):
                    problems.append((page, line, f"link to {target}: {rel} has no heading with anchor #{anchor}"))
            # codes and LANG rules stated must sit with a citation of the section that states them
            named_codes = CODE.findall(body)
            named_rules = re.findall(r"\bLANG-\d\d-\d\d\b", body)
            if named_codes or named_rules:          # every page, the README and the skill included
                cites = citations(body) + h2_cites.get(heading, [])
                if not cites:
                    problems.append((page, line, f"names {', '.join(sorted(set(named_codes + named_rules)))} without citing a section: {body[:100]}"))
                    continue
                texts = [sections[d][s][1] for d, s in cites if d in sections and s in sections[d]]
                unseen = any(d not in sections for d, _ in cites)    # a cited document this checkout does not have
                for c in sorted(set(named_codes)):
                    if not unseen and not any(c in t for t in texts):
                        problems.append((page, line, f"{c} occurs in none of the cited sections ({', '.join(f'{d} {s}' for d, s in cites)}): {body[:100]}"))
                for r in sorted(set(named_rules)):
                    allowed = RULE_SECTIONS.get(r)
                    if allowed is None:
                        problems.append((page, line, f"{r} has no entry in RULE_SECTIONS (cite_check.py): add the sections that state it"))
                        continue
                    if not any(d == "LR" and states(s, r) for d, s in cites):
                        problems.append((page, line, f"{r} is stated in LR {', '.join(allowed)}, which this text does not cite: {body[:100]}"))
        if sec_name is not None and not sec_cites and mode in ("human", "agent") and not page.endswith("README.md"):
            problems.append((page, sec_start, f"section `{sec_name}` cites no section of the specification"))
        for m in re.finditer(r"<!--\s*no-citation", text):
            pass
    # no-citation escapes: recompute problems for sections that carry the comment
    exempt = set()
    for page in pages:
        lines = open(os.path.join(ROOT, page), encoding="utf-8").read().split("\n")
        current = None
        for i, l in enumerate(lines, 1):
            if re.match(r"^##?\s", l):
                current = i
            if "<!-- no-citation:" in l and current:
                exempt.add((page, current))
    problems = [p for p in problems if not ((p[0], p[1]) in exempt and "cites no section" in p[2])]
    # coverage
    top = [s for s in sections["LR"] if "." not in s]
    subs = [s for s in sections["LR"] if "." in s]
    rows, missing, pending = [], [], []
    for sid in top:
        row = {"section": sid, "title": sections["LR"][sid][0]}
        for mode in ("human", "agent"):
            pages_for = [(p, stub) for c, ps in covers[mode].items() for p, stub in ps if c == sid or c.split(".")[0] == sid]
            full = sorted({p for p, stub in pages_for if not stub})
            stubs = sorted({f"{p} (stub {stub})" for p, stub in pages_for if stub})
            row[mode] = full + stubs
            if not pages_for:
                missing.append(f"LR {sid} has no {mode}-mode chapter")
        rows.append(row)
    for sid in subs:
        for mode in ("human", "agent"):
            if ("LR", sid) not in cited[mode]:
                stubbed = [p for c, ps in covers[mode].items() for p, stub in ps if stub and c == sid]
                if stubbed:
                    pending.append(f"LR {sid} ({mode}): only in stub {stubbed[0]}")
                else:
                    missing.append(f"LR {sid} is cited nowhere in the {mode} mode")
    for sid in top:
        for mode in ("human", "agent"):
            stubbed_only = all(stub for c, ps in covers[mode].items() for p, stub in ps if c == sid or c.split(".")[0] == sid) and any(True for c in covers[mode] if c == sid or c.split(".")[0] == sid)
            if stubbed_only:
                pending.append(f"LR {sid} ({mode}): only in stubs")
    # a stub chapter is pending whatever else covers its sections: the chapter itself is not written yet
    for mode in ("human", "agent"):
        for p, stub in sorted({(p, stub) for ps in covers[mode].values() for p, stub in ps if stub}):
            pending.append(f"{p} is a stub waiting for {stub}")
    for page, line, detail in problems:
        print(f"FAIL {page}:{line}: {detail}")
    for m in missing:
        print(f"FAIL coverage: {m}")
    for p in pending:
        print(f"PENDING {p}")
    if verbose:
        for r in rows:
            print(f"LR {r['section']:<3} human: {', '.join(r['human']) or '-'} | agent: {', '.join(r['agent']) or '-'}")
    n_cites = sum(len(v) for m in cited.values() for v in m.values())
    # criterion 3's count: every distinct ID the release check finds in its files, each resolved against the
    # specification (a rule in its table, a code named in a specification document)
    release_ids, first_seen = set(), {}
    for f in release_scope():
        if not f.endswith(".md"):
            # the release counts its IDs, but only Markdown pages are checked citation by citation
            problems.append((f, 0, "not a Markdown page: the documentation set is written in Markdown, and this file's citations would be counted without being checked"))
        for n, l in enumerate(open(os.path.join(ROOT, f), encoding="utf-8").read().split("\n"), 1):
            for i in RELEASE_ID.findall(l):
                release_ids.add(i)
                first_seen.setdefault(i, (f, n))
    for i in sorted(release_ids):
        known = ids[i.split("-")[0]] if i.startswith(("LANG-", "PROG-", "PLAT-")) else ids["codes"]
        if known is not None and i not in known:
            problems.append((*first_seen[i], f"{i} resolves to nothing in the specification"))
    print(f"\n{len(pages)} pages, {n_cites} citations; {len(top)} sections and {len(subs)} subsections of the Language Reference; "
          f"{len(problems)} citation problems, {len(missing)} coverage gaps, {len(pending)} pending on stubs")
    # one summary line per criterion, which the release check (DEL-44, DW-11) compares with the repository:
    # criterion 1, the numbered sections each mode covers (a chapter that is not a stub, every subsection cited);
    # criterion 3, the citations and the two kinds of citation problem
    numbered = [s for s in top if s.isdigit()]

    def covered(mode):
        out = []
        for sid in numbered:
            chapters = [p for c, ps in covers[mode].items() for p, stub in ps if not stub and (c == sid or c.split(".")[0] == sid)]
            gaps = [m for m in missing if f"LR {sid}" in m and mode in m and (f"LR {sid} " in m or f"LR {sid}." in m)]
            if chapters and not gaps:
                out.append(sid)
        return out
    unstated_marks = ("occurs in none of the cited sections", "is stated in LR", "without citing a section", "cites no section", "RULE_SECTIONS")
    unstated = sum(1 for _, _, d in problems if any(k in d for k in unstated_marks))
    print(json.dumps({"kind": "docs-check", "criterion": 1, "sections": numbered, "human": covered("human"), "agent": covered("agent"),
                      "readme": {"human": os.path.exists(os.path.join(ROOT, "README.md")),
                                 "agent": os.path.exists(os.path.join(ROOT, "docs", "agent", "README.md"))}}))
    print(json.dumps({"kind": "docs-check", "criterion": 3, "citations": len(release_ids), "unresolved": len(problems) - unstated, "unstated_rules": unstated}))
    if "--json" in argv:
        os.makedirs(os.path.join(ROOT, "build"), exist_ok=True)
        json.dump({"rule": "PROG-AGT-07", "chunk": "DEL-49", "coverage": rows, "missing": missing, "pending": pending,
                   "problems": [{"page": p, "line": l, "detail": d} for p, l, d in problems]},
                  open(os.path.join(ROOT, "build", "cite.json"), "w", encoding="utf-8"), indent=1)
    # `--release` (DEL-49 criteria 1 and 3, the release check): a section reached only through a stub is not covered
    ok = not problems and not missing and not ("--release" in argv and pending)
    print("cite check: " + ("OK" if ok else "FAILED") + (f" ({len(pending)} pending on stubs)" if pending else ""))
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
