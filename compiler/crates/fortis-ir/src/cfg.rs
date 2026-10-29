//! The control-flow graph of a function body (DEL-12): a port of checker/cfg.py. Blocks hold statements in canonical
//! token form and the places they read or assign; edges are labelled. `dump` is the stable text form kept as golden
//! files under conformance/cfg/golden and `parse` reads it back (dump(parse(dump(g))) == dump(g)).

use fortis_syntax::lexer::Kind;
use fortis_syntax::view::{C, N};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Block {
    pub id: usize,
    pub stmts: Vec<String>,
    pub places: Vec<String>,
    pub edges: Vec<(String, usize)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Cfg {
    pub name: String,
    pub blocks: Vec<Block>,
    pub entry: usize,
    pub exit: i64,
}

pub fn canon(node: C) -> String {
    let toks = node.tokens();
    let mut s = toks.iter().filter(|t| t.kind != Kind::Nl).map(|t| t.text.as_str()).collect::<Vec<_>>().join(" ");
    for (a, b) in [(" ( ", "("), ("( ", "("), (" )", ")"), (" ,", ","), (" .", "."), (". ", "."), (" [", "["), ("[ ", "["), (" ]", "]"), ("{ ", "{"), (" }", "}")] {
        s = s.replace(a, b);
    }
    s
}

pub fn places_of(node: N) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for pl in node.find_all("place") {
        let p = canon(C::N(pl));
        if !out.contains(&p) {
            out.push(p);
        }
    }
    for pe in node.find_all("postfixExpr") {
        let kids = pe.kids();
        let pk = kids[0].node().map(|n| n.kids()).unwrap_or_default();
        if pk.len() == 1 {
            if let C::T(t) = pk[0] {
                if matches!(t.kind, Kind::Lower | Kind::Kw) && !["null", "true", "false"].contains(&t.text.as_str()) {
                    let mut path = t.text.clone();
                    for pf in &kids[1..] {
                        let k = pf.node().map(|n| n.kids()).unwrap_or_default();
                        if !k.is_empty() && k[0].text_is(".") && k.len() > 1 && k[1].is("memberName") {
                            path = format!("{path}.{}", k[1].node().unwrap().tokens()[0].text);
                        } else {
                            break;
                        }
                    }
                    if !out.contains(&path) {
                        out.push(path);
                    }
                }
            }
        }
    }
    out
}

struct Builder {
    cfg: Cfg,
    loops: Vec<(Option<String>, usize, usize)>,
}

impl Builder {
    fn new_block(&mut self) -> usize {
        let id = self.cfg.blocks.len();
        self.cfg.blocks.push(Block { id, ..Default::default() });
        id
    }
    fn edge(&mut self, src: usize, label: &str, dst: usize) {
        self.cfg.blocks[src].edges.push((label.to_string(), dst));
    }
    fn add_places(&mut self, b: usize, ps: Vec<String>) {
        for p in ps {
            if !self.cfg.blocks[b].places.contains(&p) {
                self.cfg.blocks[b].places.push(p);
            }
        }
    }

    fn block(&mut self, blk: N, cur: Option<usize>, exit: usize) -> Option<usize> {
        let mut cur = cur;
        for st in blk.child_nodes("statement") {
            let c = match cur {
                Some(c) => c,
                None => self.new_block(),
            };
            cur = self.statement(st, c, exit);
        }
        cur
    }

    fn statement(&mut self, st: N, cur: usize, exit: usize) -> Option<usize> {
        let inner = match st.kid(0) {
            Some(C::N(n)) => n,
            _ => {
                let w = st.direct_tokens()[0].text.clone();
                self.cfg.blocks[cur].stmts.push(canon(C::N(st)));
                self.add_places(cur, places_of(st));
                if w == "return" || w == "fail" {
                    self.edge(cur, &w, exit);
                    return None;
                }
                if w == "give" || w == "recover" {
                    return Some(cur);
                }
                if w == "break" || w == "continue" {
                    let ts = st.direct_tokens();
                    let label = ts.get(1).map(|t| t.text.clone());
                    for (lab, cont, brk) in self.loops.clone().into_iter().rev() {
                        if label.is_none() || lab == label {
                            self.edge(cur, &w, if w == "break" { brk } else { cont });
                            return None;
                        }
                    }
                    return None;
                }
                return Some(cur);
            }
        };
        match inner.name() {
            "ifStmt" => {
                let conds = inner.child_nodes("expr");
                let blocks = inner.child_nodes("block");
                let has_else = blocks.len() > conds.len();
                let join = self.new_block();
                let mut test = cur;
                for (i, (cond, blk)) in conds.iter().zip(blocks.iter()).enumerate() {
                    self.cfg.blocks[test].stmts.push(format!("if {}", canon(C::N(*cond))));
                    self.add_places(test, places_of(*cond));
                    let then = self.new_block();
                    self.edge(test, "true", then);
                    if let Some(end) = self.block(*blk, Some(then), exit) {
                        self.edge(end, "next", join);
                    }
                    if i + 1 < conds.len() {
                        let nxt = self.new_block();
                        self.edge(test, "false", nxt);
                        test = nxt;
                    } else if has_else {
                        let els = self.new_block();
                        self.edge(test, "false", els);
                        if let Some(end) = self.block(*blocks.last().unwrap(), Some(els), exit) {
                            self.edge(end, "next", join);
                        }
                    } else {
                        self.edge(test, "false", join);
                    }
                }
                Some(join)
            }
            "whileStmt" | "loopStmt" | "forStmt" => {
                let label = inner.find("label").map(|l| l.tokens()[0].text.clone());
                let head = self.new_block();
                self.edge(cur, "loop", head);
                let body = self.new_block();
                let after = self.new_block();
                match inner.name() {
                    "whileStmt" => {
                        let cond = inner.find("expr").unwrap();
                        self.cfg.blocks[head].stmts.push(format!("while {}", canon(C::N(cond))));
                        let ps = places_of(cond);
                        self.cfg.blocks[head].places.extend(ps);
                        self.edge(head, "true", body);
                        self.edge(head, "false", after);
                    }
                    "forStmt" => {
                        let ts = inner.direct_tokens();
                        let var = ts[ts.iter().position(|t| t.text == "for").unwrap() + 1].text.clone();
                        let src = inner.kids().into_iter().filter_map(|c| c.node()).find(|c| ["expr", "rangeExpr", "place"].contains(&c.name())).unwrap();
                        self.cfg.blocks[head].stmts.push(format!("for {var} in {}", canon(C::N(src))));
                        let ps = places_of(src);
                        self.cfg.blocks[head].places.extend(ps);
                        self.edge(head, "next", body);
                        self.edge(head, "done", after);
                    }
                    _ => {
                        self.cfg.blocks[head].stmts.push("loop".into());
                        self.edge(head, "again", body);
                    }
                }
                self.loops.push((label, head, after));
                let end = self.block(inner.find("block").unwrap(), Some(body), exit);
                self.loops.pop();
                if let Some(end) = end {
                    self.edge(end, "back", head);
                }
                Some(after)
            }
            "block" => {
                let ib = self.new_block();
                self.edge(cur, "enter", ib);
                let end = self.block(inner, Some(ib), exit)?;
                let out = self.new_block();
                self.edge(end, "leave", out);
                Some(out)
            }
            "exprStmt" if inner.first_token().map_or(false, |t| t.text == "match") => {
                let m = inner.find("matchExpr").unwrap();
                let scrut = m.kids().into_iter().filter_map(|c| c.node()).find(|c| c.name() == "expr" || c.name() == "place").unwrap();
                self.cfg.blocks[cur].stmts.push(format!("match {}", canon(C::N(scrut))));
                self.add_places(cur, places_of(scrut));
                let join = self.new_block();
                let mut any_open = false;
                for arm in m.child_nodes("matchArm") {
                    // the alternatives of an or-pattern `A | B` (9.8)
                    let label = arm.child_nodes("pattern").into_iter().map(|p| canon(C::N(p))).collect::<Vec<_>>().join(" | ");
                    let ab = self.new_block();
                    self.edge(cur, &label, ab);
                    let body = arm.kids().into_iter().filter_map(|c| c.node()).filter(|c| c.name() == "expr" || c.name() == "block").last().unwrap();
                    let end = if body.name() == "block" {
                        self.block(body, Some(ab), exit)
                    } else {
                        self.cfg.blocks[ab].stmts.push(canon(C::N(body)));
                        let ps = places_of(body);
                        self.cfg.blocks[ab].places.extend(ps);
                        if body.tokens()[0].text == "fail" {
                            self.edge(ab, "fail", exit);
                            None
                        } else {
                            Some(ab)
                        }
                    };
                    if let Some(end) = end {
                        self.edge(end, "next", join);
                        any_open = true;
                    }
                }
                if any_open {
                    Some(join)
                } else {
                    None
                }
            }
            _ => {
                self.cfg.blocks[cur].stmts.push(canon(C::N(inner)));
                self.add_places(cur, places_of(inner));
                Some(cur)
            }
        }
    }
}

/// The CFG of a block-bodied function declaration (None for an expression body).
pub fn build(func_node: N, fname: &str) -> Option<Cfg> {
    let body = func_node.find("funcBody")?;
    build_body(body, fname)
}

/// The CFG of a function body (`funcBody`) whose first child is a block.
pub fn build_body(body: N, fname: &str) -> Option<Cfg> {
    let blk = body.find("block")?;
    Some(build_block(blk, fname))
}

/// The CFG of a block standing as a body (a test, a property, a cleanup).
pub fn build_block(blk: N, fname: &str) -> Cfg {
    let mut b = Builder { cfg: Cfg { name: fname.to_string(), blocks: vec![], entry: 0, exit: -1 }, loops: vec![] };
    let entry = b.new_block();
    let exit = b.new_block();
    b.cfg.exit = exit as i64;
    if let Some(end) = b.block(blk, Some(entry), exit) {
        b.edge(end, "fallthrough", exit);
    }
    b.cfg
}

pub fn dump(cfg: &Cfg) -> String {
    let mut lines = vec![format!("cfg {} entry={} exit={}", cfg.name, cfg.entry, cfg.exit)];
    for b in &cfg.blocks {
        lines.push(format!("B{}:", b.id));
        for s in &b.stmts {
            lines.push(format!("  stmt {s}"));
        }
        if !b.places.is_empty() {
            lines.push(format!("  places {}", b.places.join(", ")));
        }
        for (label, dst) in &b.edges {
            lines.push(format!("  -> B{dst} [{label}]"));
        }
    }
    lines.join("\n") + "\n"
}

pub fn parse(text: &str) -> Option<Cfg> {
    let mut lines = text.trim_end_matches('\n').split('\n');
    let head: Vec<&str> = lines.next()?.split_whitespace().collect();
    let mut cfg = Cfg { name: head.get(1)?.to_string(), blocks: vec![], entry: head.get(2)?.split('=').nth(1)?.parse().ok()?, exit: head.get(3)?.split('=').nth(1)?.parse().ok()? };
    for ln in lines {
        if ln.starts_with('B') && ln.ends_with(':') {
            cfg.blocks.push(Block { id: ln[1..ln.len() - 1].parse().ok()?, ..Default::default() });
        } else if let Some(s) = ln.strip_prefix("  stmt ") {
            cfg.blocks.last_mut()?.stmts.push(s.to_string());
        } else if let Some(s) = ln.strip_prefix("  places ") {
            cfg.blocks.last_mut()?.places = s.split(", ").map(|x| x.to_string()).collect();
        } else if let Some(rest) = ln.strip_prefix("  -> B") {
            let (dst, label) = rest.split_once(" [")?;
            cfg.blocks.last_mut()?.edges.push((label[..label.len() - 1].to_string(), dst.parse().ok()?));
        }
    }
    Some(cfg)
}
