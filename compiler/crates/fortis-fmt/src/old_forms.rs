//! The v0.0.2 rewrite of old forms (X-345): `fortis fmt` drops what the reader can already see, a `Void` return
//! type, an `editable` receiver marker the binding already shows, and the `move` of `return move local`
//! (fortis_types::redundant). The old forms stay accepted; this turns them into the new ones.
//!
//! The rewrite is checked before it is kept: the result must parse, check with the same records (code and line),
//! and leave nothing more to drop. A rewrite that would change the check is a formatter defect, never a changed
//! program.

use fortis_types::redundant::{self, Deletion, Unit};

use crate::FormatError;

/// The source with its old forms rewritten, and what was dropped.
pub fn rewrite(src: &str, file: &str) -> Result<(String, Vec<Deletion>), FormatError> {
    rewrite_in(src, Unit::Alone(file))
}

/// `rewrite` for a module checked as `unit` says: the markers of a package module whose imports do not resolve alone
/// are found, and the rewrite verified, in its package.
pub fn rewrite_in(src: &str, unit: Unit) -> Result<(String, Vec<Deletion>), FormatError> {
    let Ok(parsed) = fortis_syntax::parse_module(src) else { return Ok((src.to_string(), vec![])) };
    let mut dels = redundant::void_returns(&parsed);
    let (records, markers) = redundant::redundant_markers_in(&parsed, src, unit);
    dels.extend(markers);
    if dels.is_empty() {
        return Ok((src.to_string(), dels));
    }
    let text = redundant::apply(src, &dels);
    let again = fortis_syntax::parse_module(&text).map_err(|e| FormatError::Changed(format!("the rewritten old forms do not parse: {e:?}")))?;
    let (after, left) = redundant::redundant_markers_in(&again, &text, unit);
    if redundant::seen(&records) != redundant::seen(&after) || !left.is_empty() || !redundant::void_returns(&again).is_empty() {
        return Err(FormatError::Changed(format!(
            "rewriting the old forms changes the check: {} records before, {} after, {} markers still redundant",
            records.len(),
            after.len(),
            left.len()
        )));
    }
    Ok((text, dels))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rejected_module_loses_only_void() {
        let src = "func f() Void {\n  let mut xs = List.of(1)\n  editable xs.push(true)\n}\n";
        let (out, dels) = rewrite(src, "t.ft").unwrap();
        assert_eq!(out, "func f() {\n  let mut xs = List.of(1)\n  editable xs.push(true)\n}\n");
        assert_eq!(dels.len(), 1);
    }

    #[test]
    fn an_immutable_receiver_keeps_its_place_in_the_error() {
        let src = "func f() Int {\n  let xs = List.of(1)\n  return xs.count()\n}\n";
        assert_eq!(rewrite(src, "t.ft").unwrap().0, src);
    }
}
