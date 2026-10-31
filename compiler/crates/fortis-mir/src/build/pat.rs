//! Patterns (9.5): each binding becomes a local of the enclosing arm or `let`, typed from the scrutinee's type.

use fortis_syntax::lexer::Kind;
use fortis_syntax::view::{C, N};
use fortis_types::decls::named_of_decl;
use fortis_types::types::*;

use super::expr::value_ty;
use super::{err, line_of, BResult, Builder};
use crate::model::*;

impl<'t, 'a> Builder<'t, 'a> {
    pub fn pattern(&mut self, p: N<'a>, t: &Type, moving: bool) -> BResult<Pattern> {
        let line = line_of(p);
        let kids = p.kids();
        let s = value_ty(t);
        let first = kids[0];
        match first {
            C::N(n) if n.name() == "literal" => {
                let e = self.literal_tok(n.tokens()[0], Some(s.clone()))?;
                match e.kind {
                    ExprKind::Const(c) => Ok(Pattern::Literal(c)),
                    _ => err(line, "an interpolated string as a pattern"),
                }
            }
            C::N(n) if n.name() == "rangePattern" => {
                // a range pattern (9.8, X-517): the bounds fold as a refinement's fold (X-512); the checker has
                // judged the form (E-MAT-005, E-TYP-040), so the MIR keeps the folded inclusive interval, its open
                // ends clipped to the base's own range (a value of the type always lies inside it, and every
                // backend then holds one closed interval)
                let base = match base_name(&s) {
                    Some(b) if int_types(b).is_some() => b,
                    _ => return err(line, "a range pattern over a non-integer type"),
                };
                let (lo, hi, why) = self.ty.dc.range_of(n, base);
                match why {
                    None => {
                        let full = int_types(base).unwrap();
                        Ok(Pattern::Range { lo: Some(lo.unwrap_or(full.0)), hi: Some(hi.unwrap_or(full.1)) })
                    }
                    Some(why) => err(line, format!("a range pattern whose bound does not fold: {why}")),
                }
            }
            C::N(n) => err(line, format!("pattern {}", n.name())),
            C::T(tk) => {
                let w = tk.text.as_str();
                match (tk.kind, w) {
                    (_, "_") => Ok(Pattern::Wildcard),
                    (Kind::Kw, "null") => Ok(Pattern::Null),
                    (Kind::Kw, "true") => Ok(Pattern::Literal(Const::Bool(true))),
                    (Kind::Kw, "false") => Ok(Pattern::Literal(Const::Bool(false))),
                    (_, "present") => {
                        let inner = match &s {
                            Type::Optional(i) => (**i).clone(),
                            _ => unknown("present"),
                        };
                        let ip = self.sub_pattern(p, 0, &inner, moving)?;
                        Ok(Pattern::Present(Box::new(ip)))
                    }
                    (_, "success") | (_, "failure") => {
                        let inner = if w == "success" {
                            match &s {
                                Type::Named(n) if n.package.is_empty() && n.name == "Outcome" && !n.args.is_empty() => n.args[0].clone(),
                                _ => unknown("success"),
                            }
                        } else {
                            named("Failure")
                        };
                        let ip = self.sub_pattern(p, 0, &inner, moving)?;
                        Ok(if w == "success" { Pattern::Success(Box::new(ip)) } else { Pattern::Failure(Box::new(ip)) })
                    }
                    (_, "otherwise") => {
                        let b = kids.iter().filter_map(|k| k.tok()).find(|t| t.kind == Kind::Lower).map(|t| t.text.clone());
                        let l = b.map(|b| self.fb().declare(&b, s.clone(), false, LocalKind::Binding));
                        Ok(Pattern::Otherwise(l))
                    }
                    (_, "is") => {
                        let tn = kids.iter().filter_map(|k| k.tok()).find(|t| t.kind == Kind::Upper).unwrap().text.clone();
                        let it = self.ty.dc.table.get(&tn).map(|d| named_of_decl(&d, vec![])).unwrap_or_else(|| named(&tn));
                        let fields = self.pattern_fields(p, &it, moving)?;
                        let words: Vec<String> = kids.iter().filter_map(|k| k.tok()).map(|t| t.text.clone()).collect();
                        let bind = match words.iter().position(|w| w == "as") {
                            Some(i) => Some(self.fb().declare(&words[i + 1], it.clone(), false, LocalKind::Binding)),
                            None => None,
                        };
                        let ty = self.ty.dc.table.get(&tn).map(|d| self.tname(&d.name, &d.type_identity())).unwrap_or(tn);
                        Ok(Pattern::Is { ty, fields, bind })
                    }
                    (_, "(") => {
                        let subs: Vec<N<'a>> = p.child_nodes("pattern");
                        let items = match &s {
                            Type::Tuple(ts) => ts.clone(),
                            _ => vec![],
                        };
                        let mut out = Vec::new();
                        for (i, sp) in subs.into_iter().enumerate() {
                            let it = items.get(i).cloned().unwrap_or_else(|| unknown("tuple item"));
                            out.push(self.pattern(sp, &it, moving)?);
                        }
                        Ok(Pattern::Tuple(out))
                    }
                    (_, "{") => {
                        let names: Vec<String> = kids.iter().filter_map(|k| k.tok()).filter(|t| t.kind == Kind::Lower).map(|t| t.text.clone()).collect();
                        let mut out = Vec::new();
                        for (n, sp) in names.iter().zip(p.child_nodes("pattern")) {
                            let ft = self.member_ty(&s, n);
                            out.push((n.clone(), self.pattern(sp, &ft, moving)?));
                        }
                        Ok(Pattern::Record(out))
                    }
                    (Kind::Lower, name) => {
                        let l = self.fb().declare(name, t.clone(), false, LocalKind::Binding);
                        Ok(Pattern::Bind(l))
                    }
                    (Kind::Upper, name) => {
                        // a variant of the scrutinee's enum, or a class destructured under move
                        if let Some(d) = self.ty.decl_of(&s) {
                            if d.kind == "enum" && d.variants.contains(name) {
                                let payload = d.variants.get(name).cloned().unwrap_or_default();
                                let fields = self.variant_pattern_fields(p, &payload, moving)?;
                                return Ok(Pattern::Variant { ty: self.enum_ir_name(&d), variant: name.to_string(), fields });
                            }
                            if d.kind == "class" && d.name == name {
                                let fields = self.pattern_fields(p, &s, moving)?;
                                return Ok(Pattern::Class { ty: self.tname(&d.name, &d.type_identity()), fields });
                            }
                        }
                        // a variant named through its enum type elsewhere (Optional of an enum, a failure's error type)
                        if let Some(d) = self.ty.dc.table.get(name).cloned() {
                            if d.kind == "class" {
                                let ct = named_of_decl(&d, vec![]);
                                let fields = self.pattern_fields(p, &ct, moving)?;
                                return Ok(Pattern::Class { ty: self.tname(&d.name, &d.type_identity()), fields });
                            }
                        }
                        let owner = self.ty.dc.table.decls.values().find(|d| d.kind == "enum" && d.variants.contains(name)).cloned();
                        match owner {
                            Some(d) => {
                                let payload = d.variants.get(name).cloned().unwrap_or_default();
                                let fields = self.variant_pattern_fields(p, &payload, moving)?;
                                Ok(Pattern::Variant { ty: self.enum_ir_name(&d), variant: name.to_string(), fields })
                            }
                            None => err(line, format!("pattern `{name}` names no variant or class")),
                        }
                    }
                    (Kind::Int, _) | (Kind::Str, _) | (Kind::Dec, _) | (Kind::Float, _) => {
                        let e = self.literal_tok(tk, Some(s.clone()))?;
                        match e.kind {
                            ExprKind::Const(c) => Ok(Pattern::Literal(c)),
                            _ => err(line, "an interpolated string as a pattern"),
                        }
                    }
                    _ => err(line, format!("pattern `{w}`")),
                }
            }
        }
    }

    fn sub_pattern(&mut self, p: N<'a>, i: usize, t: &Type, moving: bool) -> BResult<Pattern> {
        match p.child_nodes("pattern").get(i) {
            Some(sp) => self.pattern(*sp, t, moving),
            None => Ok(Pattern::Wildcard),
        }
    }

    /// `Name(a: p, q)` over a class or failure type: fields by name (positional ones in declaration order).
    fn pattern_fields(&mut self, p: N<'a>, t: &Type, moving: bool) -> BResult<Vec<(String, Pattern)>> {
        let mut out = Vec::new();
        let order: Vec<String> = match self.ty.decl_of(t) {
            Some(d) if d.kind == "error" => d.payload.keys().cloned().collect(),
            Some(d) => d.fields.keys().cloned().collect(),
            None => vec![],
        };
        if let Some(pf) = p.child_node("patternFields") {
            for (i, f) in pf.child_nodes("patternField").into_iter().enumerate() {
                let ft = f.direct_tokens();
                let sp = f.child_node("pattern").unwrap();
                let name = if ft.len() > 1 && ft[1].text == ":" { ft[0].text.clone() } else { order.get(i).cloned().unwrap_or_else(|| sp.tokens()[0].text.clone()) };
                let fty = self.member_ty(t, &name);
                out.push((name, self.pattern(sp, &fty, moving)?));
            }
        }
        Ok(out)
    }

    fn variant_pattern_fields(&mut self, p: N<'a>, payload: &[(String, Type)], moving: bool) -> BResult<Vec<(String, Pattern)>> {
        let mut out = Vec::new();
        if let Some(pf) = p.child_node("patternFields") {
            for (i, f) in pf.child_nodes("patternField").into_iter().enumerate() {
                let ft = f.direct_tokens();
                let sp = f.child_node("pattern").unwrap();
                let name = if ft.len() > 1 && ft[1].text == ":" {
                    ft[0].text.clone()
                } else {
                    // a bare binding names the field it binds when the names agree, else the position decides
                    let bare = sp.tokens()[0].text.clone();
                    if payload.iter().any(|(n, _)| *n == bare) && sp.tokens().len() == 1 {
                        bare
                    } else {
                        payload.get(i).map(|x| x.0.clone()).unwrap_or(bare)
                    }
                };
                let fty = payload.iter().find(|(n, _)| *n == name).map(|x| x.1.clone()).unwrap_or_else(|| unknown("payload"));
                out.push((name, self.pattern(sp, &fty, moving)?));
            }
        }
        Ok(out)
    }
}
