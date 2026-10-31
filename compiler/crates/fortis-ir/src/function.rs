//! The function IR (PROG-CMP-02): for every body the checker types, its control-flow graph and the facts of the typed
//! walk: loans with their regions, obligations with where and how each is closed, the effect summary, the required
//! observation operations, and the cleanup edges of its exits. One JSON object per function, described by
//! reference/schemas/ir.schema.json.

use std::collections::BTreeMap;

use fortis_diag::Json;
use fortis_syntax::view::{declarations, N};
use fortis_syntax::Parsed;
use fortis_types::journal::FnIr;

use crate::cfg;

fn cfg_json(c: &cfg::Cfg) -> Json {
    Json::obj([
        ("entry", Json::Int(c.entry as i64)),
        ("exit", Json::Int(c.exit)),
        (
            "blocks",
            Json::Arr(
                c.blocks
                    .iter()
                    .map(|b| {
                        Json::obj([
                            ("id", Json::Int(b.id as i64)),
                            ("stmts", Json::strs(b.stmts.clone())),
                            ("places", Json::strs(b.places.clone())),
                            ("edges", Json::Arr(b.edges.iter().map(|(l, d)| Json::obj([("label", Json::str(l)), ("to", Json::Int(*d as i64))])).collect())),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
}

/// The function bodies of a module by checker label ("function f", "method T.m", "cleanup T", "test \"name\""):
/// a `funcBody`, or the block itself for a cleanup, test or property.
fn bodies<'a>(root: N<'a>) -> BTreeMap<String, N<'a>> {
    let mut out = BTreeMap::new();
    for d in declarations(root) {
        let name = d.name.clone().unwrap_or_default();
        match d.kind {
            "func" => {
                if let Some(b) = d.node.find("funcBody") {
                    out.insert(format!("function {name}"), b);
                }
            }
            "test" | "property" => {
                if let (Some(t), Some(b)) = (d.token, d.node.child_nodes("block").last()) {
                    out.insert(format!("test {}", t.text), *b);
                }
            }
            "class" | "enum" | "mixin" => {
                for m in d.node.kids().into_iter().filter_map(|c| c.node()) {
                    if m.name() != "classMember" && m.name() != "enumMember" && m.name() != "mixinMember" {
                        continue;
                    }
                    if let Some(b) = m.find("cleanupDecl").and_then(|c| c.find("block")) {
                        out.insert(format!("cleanup {name}"), b);
                    }
                    if let (Some(head), Some(body)) = (m.find("methodHead"), m.find("funcBody")) {
                        let mn = head.find("memberName").map(|x| x.tokens()[0].text.clone()).unwrap_or_default();
                        out.insert(format!("method {name}.{mn}"), body);
                    }
                }
            }
            "implement" => {
                let ts = d.node.child_nodes("type");
                let target = ts.get(1).and_then(|t| t.tokens().into_iter().find(|x| x.kind == fortis_syntax::Kind::Upper).map(|x| x.text.clone())).unwrap_or_default();
                for m in d.node.child_nodes("implementMember") {
                    if let (Some(head), Some(body)) = (m.find("methodHead"), m.find("funcBody")) {
                        let mn = head.find("memberName").map(|x| x.tokens()[0].text.clone()).unwrap_or_default();
                        out.insert(format!("method {target}.{mn}"), body);
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// A loop body is walked twice (its fixpoint): keep each distinct fact once, in first-seen order.
fn dedup<T: Clone, K: PartialEq, F: Fn(&T) -> K>(items: &[T], key: F) -> Vec<T> {
    let mut seen: Vec<K> = Vec::new();
    let mut out = Vec::new();
    for x in items {
        let k = key(x);
        if !seen.contains(&k) {
            seen.push(k);
            out.push(x.clone());
        }
    }
    out
}

pub fn to_json(ir: &FnIr, cfgs: &BTreeMap<String, N>) -> Json {
    let ir_src = ir.clone();
    let ir_closes = |src: &FnIr, place: &str, opened: usize| -> Vec<(usize, String)> {
        src.obligations.iter().filter(|o| o.place == place && o.opened == opened).flat_map(|o| o.closes.clone()).collect()
    };
    let mut ir = ir.clone();
    ir.loans = dedup(&ir.loans, |l| (l.origin.clone(), l.owner.clone(), l.mode.clone(), l.holder.clone(), l.depth, l.line));
    ir.obligations = dedup(&ir.obligations, |o| (o.origin.clone(), o.place.clone(), o.opened));
    for o in ir.obligations.iter_mut() {
        // a loop body is walked to its fixpoint: each distinct closing event once, in first-seen order
        let all: Vec<(usize, String)> = ir_closes(&ir_src, &o.place, o.opened);
        o.closes = dedup(&all, |c| c.clone());
    }
    ir.observation = dedup(&ir.observation, |o| (o.origin.clone(), o.op.clone(), o.name.clone(), o.line, o.required.clone()));
    ir.cleanup = dedup(&ir.cleanup, |c| (c.origin.clone(), c.op.clone(), c.name.clone(), c.line, c.runs.clone()));
    let ir = &ir;
    let body_label = ir.source_function.as_deref().unwrap_or(&ir.function);
    let c = cfgs.get(body_label).and_then(|b| if b.name() == "block" { Some(cfg::build_block(*b, &ir.function)) } else { cfg::build_body(*b, &ir.function) });
    let eff = ir.effects.clone().unwrap_or_default();
    let mut effects: Vec<(String, String)> = eff.effects.clone();
    effects.sort();
    let mut json = Json::obj([
        ("function", Json::str(&ir.function)),
        ("line", Json::Int(ir.line as i64)),
        ("cfg", c.as_ref().map_or(Json::Null, cfg_json)),
        (
            "loans",
            Json::Arr(
                ir.loans
                    .iter()
                    .map(|l| {
                        Json::obj([
                            ("origin", l.origin.as_ref().map_or(Json::Null, Json::str)),
                            ("owner", Json::str(&l.owner)),
                            ("mode", Json::str(&l.mode)),
                            ("holder", l.holder.as_ref().map_or(Json::Null, Json::str)),
                            ("region_depth", Json::Int(l.depth)),
                            ("line", Json::Int(l.line as i64)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "obligations",
            Json::Arr(
                ir.obligations
                    .iter()
                    .map(|o| {
                        Json::obj([
                            ("origin", o.origin.as_ref().map_or(Json::Null, Json::str)),
                            ("place", Json::str(&o.place)),
                            ("opened", Json::Int(o.opened as i64)),
                            ("closes", Json::Arr(o.closes.iter().map(|(l, h)| Json::obj([("line", Json::Int(*l as i64)), ("how", Json::str(h))])).collect())),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "effects",
            Json::obj([
                ("effects", Json::Arr(effects.iter().map(|(e, p)| Json::obj([("effect", Json::str(e)), ("provenance", Json::str(p))])).collect())),
                ("footprint", Json::strs(eff.footprint.iter().cloned())),
                ("authorities", Json::strs(eff.authorities.iter().cloned())),
                ("needs_auth", Json::Bool(eff.needs_auth)),
            ]),
        ),
        (
            "observation",
            Json::Arr(ir.observation.iter().map(|o| Json::obj([("origin", o.origin.as_ref().map_or(Json::Null, Json::str)), ("op", Json::str(&o.op)), ("name", Json::str(&o.name)), ("line", Json::Int(o.line as i64)), ("required", Json::strs(o.required.clone()))])).collect()),
        ),
        (
            "cleanup",
            Json::Arr(
                ir.cleanup
                    .iter()
                    .map(|c| Json::obj([("origin", c.origin.as_ref().map_or(Json::Null, Json::str)), ("op", Json::str(&c.op)), ("name", Json::str(&c.name)), ("line", Json::Int(c.line as i64)), ("runs", Json::Arr(c.runs.iter().map(|r| Json::Int(*r as i64)).collect()))]))
                    .collect(),
            ),
        ),
    ]);
    if let Json::Obj(fields) = &mut json {
        if let Some(file) = &ir.source_file { fields.insert("source_file".into(), Json::str(file)); }
        if let Some(function) = &ir.source_function { fields.insert("source_function".into(), Json::str(function)); }
    }
    json
}

/// The function IR of every module of a package directory, by module file (None when the set does not resolve).
pub fn package_ir(dir: &std::path::Path, search: &std::path::Path) -> Option<Vec<(String, Vec<Json>)>> {
    let mut ps = fortis_names::resolve::PackageSet::load(dir, search).ok()?;
    if !ps.run().is_empty() {
        return None;
    }
    let (_, mods) = fortis_types::check::check_package_set_ir(&ps);
    // The application remains in its class's module, but its CFG comes from the declaring mixin's text.
    let module_bodies: BTreeMap<&str, BTreeMap<String, N>> = mods.iter().map(|(file, parsed, _)| {
        (file.as_str(), bodies(N::new(&parsed.root, &parsed.toks)))
    }).collect();
    Some(
        mods.iter()
            .map(|(file, _, irs)| {
                let functions = irs.iter().map(|ir| {
                    let source = ir.source_file.as_deref().unwrap_or(file);
                    let b = module_bodies.get(source).expect("a checked body's source belongs to the package set");
                    to_json(ir, b)
                }).collect();
                (file.clone(), functions)
            })
            .collect(),
    )
}

/// The function IR of every body of one module, checked against the reference library.
pub fn function_ir(parsed: &Parsed, file: &str) -> Vec<Json> {
    let (_, irs) = fortis_types::check::check_parsed_with(parsed, file, |ty| ty.ir_out.clone());
    let root = N::new(&parsed.root, &parsed.toks);
    let b = bodies(root);
    irs.iter().map(|ir| to_json(ir, &b)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applied_mixin_cfg_uses_its_declaring_body() {
        let parsed = fortis_syntax::parse_module(r#"
mixin Reads {
    export func read(self) Int {
        return self.value
    }
}
class Box with Reads {
    export value: Int
}
func ordinary() Int => 0
"#).expect("parse");
        let functions = function_ir(&parsed, "mixin.ft");
        let fields = |label: &str| functions.iter().find_map(|f| match f {
            Json::Obj(fields) if fields.get("function") == Some(&Json::str(label)) => Some(fields),
            _ => None,
        }).expect("typed function");
        let applied = fields("method Box.read");
        assert_eq!(applied.get("source_file"), Some(&Json::str("mixin.ft")));
        assert_eq!(applied.get("source_function"), Some(&Json::str("method Reads.read")));
        assert!(matches!(applied.get("cfg"), Some(Json::Obj(_))), "the applied block body has a CFG");
        let ordinary = fields("function ordinary");
        assert!(!ordinary.contains_key("source_file"));
        assert!(!ordinary.contains_key("source_function"));
    }
}
