"""The declaration IR's text form (DEL-31): every declaration a module makes, with its members' types and signatures,
as the declaration checker sees them. The production frontend (compiler/crates/fortis-ir) writes the same text, so the
two declaration tables are compared exactly on every fixture (conformance/tools/frontend_check.py --ir)."""
from __future__ import annotations


def b(x) -> str:
    return "True" if x else "False"


def dump(dc) -> str:
    lines = []
    for key, d in dc.table.decls.items():
        if d.module != dc.module_name:
            continue
        lines.append(f"{d.kind} {key} exported={b(d.exported)} package={d.package or '-'}")
        if d.type_params:
            lines.append("  type_params " + ", ".join(d.type_params))
        if d.lifetime:
            lines.append(f"  lifetime {d.lifetime}")
        if d.trivial:
            lines.append("  trivial")
        for f, (t, e, h) in d.fields.items():
            lines.append(f"  field {f}: {t.show()} exported={b(e)} default={b(h)}")
        for m, s in d.methods.items():
            lines.append(f"  method {m}: {s.show()} receiver={s.receiver} exported={b(s.exported)}")
        for v, p in d.variants.items():
            lines.append(f"  variant {v}(" + ", ".join(f"{k}: {t.show()}" for k, t in p) + ")")
        for n, args in d.derives:
            lines.append(f"  derive {n}" + (f"[{', '.join(a.show() for a in args)}]" if args else ""))
        for i in d.implements:
            lines.append(f"  implements {i.show()}")
        for i in d.mixins:
            lines.append(f"  with {i.show()}")
        for i in d.requires:
            lines.append(f"  requires {i.show()}")
        for c, t in d.constants.items():
            lines.append(f"  const {c}: {t.show()}")
        if d.alias is not None:
            lines.append(f"  alias {d.alias.show()}")
        for tp, i in d.where:
            lines.append(f"  where {tp} implements {i.show()}")
        if d.code:
            lines.append(f"  code {d.code}")
        for k, t in d.payload.items():
            lines.append(f"  payload {k}: {t.show()}")
        if d.sig is not None:
            lines.append(f"  sig {d.sig.show()}")
        for f, (t, req) in d.obs_fields.items():
            lines.append(f"  observe {f}: {t.show()} required={req}")
    for iface, target, module, _ in dc.table.implement_blocks:
        if module == dc.module_name:
            lines.append(f"implement {iface.show()} for {target.show()}")
    return "\n".join(lines) + "\n"
