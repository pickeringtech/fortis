//! The declaration IR's text form (DEL-31): every declaration a module makes, with its members' types and signatures.
//! checker/decl_ir.py writes the same text from the reference declaration checker.

use fortis_types::decls::DeclChecker;

fn b(x: bool) -> &'static str {
    if x {
        "True"
    } else {
        "False"
    }
}

pub fn dump(dc: &DeclChecker) -> String {
    let mut lines: Vec<String> = Vec::new();
    for (key, d) in dc.table.decls.iter() {
        if d.module != dc.module_name {
            continue;
        }
        lines.push(format!("{} {key} exported={} package={}", d.kind, b(d.exported), if d.package.is_empty() { "-" } else { &d.package }));
        if !d.type_params.is_empty() {
            lines.push(format!("  type_params {}", d.type_params.join(", ")));
        }
        if let Some(l) = &d.lifetime {
            lines.push(format!("  lifetime {l}"));
        }
        if d.trivial {
            lines.push("  trivial".into());
        }
        for (f, (t, e, h)) in d.fields.iter() {
            lines.push(format!("  field {f}: {} exported={} default={}", t.show(), b(*e), b(*h)));
        }
        for (m, s) in d.methods.iter() {
            lines.push(format!("  method {m}: {} receiver={} exported={}", s.show(), s.receiver.clone().unwrap_or_else(|| "None".into()), b(s.exported)));
        }
        for (v, p) in d.variants.iter() {
            lines.push(format!("  variant {v}({})", p.iter().map(|(k, t)| format!("{k}: {}", t.show())).collect::<Vec<_>>().join(", ")));
        }
        for (n, args) in &d.derives {
            if args.is_empty() {
                lines.push(format!("  derive {n}"));
            } else {
                lines.push(format!("  derive {n}[{}]", args.iter().map(|a| a.show()).collect::<Vec<_>>().join(", ")));
            }
        }
        for i in &d.implements {
            lines.push(format!("  implements {}", i.show()));
        }
        for i in &d.mixins {
            lines.push(format!("  with {}", i.show()));
        }
        for i in &d.requires {
            lines.push(format!("  requires {}", i.show()));
        }
        for (c, t) in d.constants.iter() {
            lines.push(format!("  const {c}: {}", t.show()));
        }
        if let Some(a) = &d.alias {
            lines.push(format!("  alias {}", a.show()));
        }
        for (tp, i) in &d.where_ {
            lines.push(format!("  where {tp} implements {}", i.show()));
        }
        if !d.code.is_empty() {
            lines.push(format!("  code {}", d.code));
        }
        for (k, t) in d.payload.iter() {
            lines.push(format!("  payload {k}: {}", t.show()));
        }
        if let Some(s) = &d.sig {
            lines.push(format!("  sig {}", s.show()));
        }
        for (f, (t, req)) in d.obs_fields.iter() {
            lines.push(format!("  observe {f}: {} required={req}", t.show()));
        }
    }
    for (iface, target, module, _) in &dc.table.implement_blocks {
        if *module == dc.module_name {
            lines.push(format!("implement {} for {}", iface.show(), target.show()));
        }
    }
    lines.join("\n") + "\n"
}
