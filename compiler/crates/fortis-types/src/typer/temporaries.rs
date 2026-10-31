//! Statement temporary provenance, kept separately from joined loan types (7.9; D-562).
use super::*;
use crate::literals::interp;

impl<'a> Typer<'a> {
    pub fn borrows_value(&self, t: &Type) -> bool {
        match t {
            Type::Loan { inner, .. } => self.is_bearing(inner) || self.borrows_value(inner),
            Type::Optional(inner) => self.borrows_value(inner),
            Type::Tuple(xs) => xs.iter().any(|x| self.borrows_value(x)),
            Type::Record(fs) => fs.iter().any(|(_, x)| self.borrows_value(x)),
            Type::Named(n) if n.package.is_empty() && ["Outcome", "Future", "Task"].contains(&n.name.as_str()) && !n.args.is_empty() => self.borrows_value(&n.args[0]) || n.name != "Outcome",
            Type::Fn { origin, .. } => !origin.is_empty(),
            _ => self.loan_carrying_or_cursor(t),
        }
    }

    fn temporary_given(&self, node: Option<N<'a>>, word: &str) -> bool {
        let Some(node) = node else { return false };
        for c in node.kids().iter().filter_map(|c| c.node()) {
            if c.name() == "statement" && c.direct_tokens().first().is_some_and(|t| t.text == word) {
                if self.temporary_value(c.find("expr"), None) { return true; }
            } else if !["lambda", "ifExpr", "matchExpr", "outcomeExpr"].contains(&c.name()) && self.temporary_given(Some(c), word) {
                return true;
            }
        }
        false
    }

    /// Branches are checked before their joined type: a loan type can conceal an owned fallback.
    pub fn temporary_value(&self, e: Option<N<'a>>, t: Option<&Type>) -> bool {
        let Some(e) = e else { return false };
        let n = self.core_expr(e);
        if self.temporary_values.contains(&n.id()) { return true; }
        let kids = n.kids();
        match n.name() {
            "coalesceExpr" if kids.len() == 3 => return self.temporary_value(kids[0].node(), None) || self.temporary_value(kids[2].node(), None),
            "outcomeExpr" if kids.len() > 1 => {
                return self.temporary_value(kids[0].node(), None)
                    || (kids.iter().any(|k| k.text_is("recover")) && self.temporary_value(kids.last().and_then(|k| k.node()), None))
                    || (kids.iter().any(|k| k.text_is("catch")) && self.temporary_given(n.find("block"), "recover"));
            }
            "ifExpr" => return kids.iter().filter_map(|c| c.node()).filter(|c| c.name() != "expr").any(|c| {
                if c.name() == "block" { self.temporary_given(Some(c), "give") } else { self.temporary_value(Some(c), None) }
            }),
            "matchExpr" => return kids.iter().filter_map(|c| c.node()).filter(|c| c.name() == "matchArm").any(|arm| {
                arm.kids().iter().filter_map(|c| c.node()).any(|c| match c.name() {
                    "block" => self.temporary_given(Some(c), "give"),
                    "expr" => self.temporary_value(Some(c), None),
                    _ => false,
                })
            }),
            "primaryExpr" if kids.len() == 4 && kids[0].text_is("present") => return self.temporary_value(kids[2].node(), None),
            _ => {}
        }
        let ts = n.tokens();
        if self.path_of_expr(Some(n)).is_some() && !(n.name() == "markerExpr" && ts.first().is_some_and(|t| t.text == "move" || t.text == "copy")) { return false; }
        if ts.len() == 1 && matches!(ts[0].kind, Kind::Str | Kind::Raw | Kind::Bytes) && expr::hole_sources(ts[0]).is_empty() { return false; }
        t.is_some_and(|t| (self.is_bearing(t) || *t == interp()) && !self.borrows_value(t))
    }

    pub fn check_temporary_borrows(&mut self) {
        self.check_temporary_borrows_in(None);
    }

    pub fn check_temporary_borrows_in(&mut self, e: Option<N<'a>>) {
        let span = e.map(|e| e.tokens()).unwrap_or_default();
        for tok in self.temporary_borrows.clone() {
            if !span.is_empty() && !((span[0].line, span[0].col) <= (tok.line, tok.col) && (tok.line, tok.col) <= (span.last().unwrap().line, span.last().unwrap().col)) { continue; }
            self.own_err("E-OWN-051", format!("the result of `{}` borrows from a possible owned temporary that ends with the statement: bind the receiver first (`let v = …`) and call `{}` on it (7.6, 7.9)", tok.text, tok.text), &tok, &tok.text, "loan of a temporary receiver", "LANG-07-06", "");
        }
    }
}
