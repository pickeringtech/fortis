//! The built-in conversion table of the `as` operator (Language Reference 6.13, X-475).
//!
//! `x as T` names exactly one conversion for the pair (type of `x`, `T`): a row of this table for two built-in
//! types, or a `convert` member declared for the pair. The table is the whole of what converts automatically: a
//! conversion that cannot lose information is total and has type `T`; one that could lose information is
//! fallible, has type `Outcome[T]` and fails when it would. Nothing here rounds: a conversion that needs a
//! rounding choice is a named function with the choice written (`Decimal.fromFloat(x, scale, mode)`), and the
//! table has no row for it.
//!
//! The reference checker has the same table (checker/conversions.py); the two agree row for row, and the backends
//! execute a row by its name, `as:<kind>:<source>:<target>`.

use crate::types::{int_types, is_int_type, Type};

/// The integer values a binary64 float holds exactly: every value of a type within this range converts totally.
pub const FLOAT_EXACT: (i128, i128) = (-(1 << 53), 1 << 53);
const TEXT: &[&str] = &["String", "StringView"];
const BYTES: &[&str] = &["Bytes", "BytesView"];
/// The types whose canonical text (what `Displayable` prints) a `String` conversion produces.
const DISPLAYED: &[&str] = &["Decimal", "Bool", "Scalar", "Float", "Float32"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conversion {
    /// the row: int_widen, int_narrow, int_to_decimal, decimal_to_int, int_to_float, to_string, parse_int,
    /// parse_decimal, parse_bool, view_to_string, bytes_to_string, string_to_bytes, scalar_to_int, int_to_scalar
    pub kind: &'static str,
    pub source: String,
    pub target: String,
    pub fallible: bool,
}

impl Conversion {
    /// The kinds whose result is a new `String` or `Bytes`: the conversion allocates (6.6).
    pub fn allocates(&self) -> bool {
        matches!(self.kind, "to_string" | "view_to_string" | "bytes_to_string" | "string_to_bytes")
    }

    /// The backends' name for the row.
    pub fn intrinsic(&self) -> String {
        format!("as:{}:{}:{}", self.kind, self.source, self.target)
    }
}

fn is_int(name: &str) -> bool {
    is_int_type(name)
}

/// The base name a conversion is keyed by: a refinement converts as its base (6.3, a refinement is a subtype).
pub fn base_of(t: &Type) -> Option<&str> {
    match t {
        Type::Refined { base, .. } => Some(base.name.as_str()),
        Type::Named(n) if n.args.is_empty() => Some(n.name.as_str()),
        _ => None,
    }
}

/// One built-in type under two names: `Int` and `Int64`, `UInt` and `UInt64`, `Byte` and `UInt8` (6.2).
pub fn same_builtin(a: &str, b: &str) -> bool {
    a == b || (is_int(a) && is_int(b) && int_types(a) == int_types(b))
}

/// The named function a `Float` source needs: a conversion would have to choose how to round (6.5).
pub fn rounding_hint(src: &str, dst: &str) -> Option<&'static str> {
    if (src == "Float" || src == "Float32") && (is_int(dst) || dst == "Decimal") {
        Some("a Float rounds by a written choice: `Decimal.fromFloat(x, scale, mode)` (6.5), and from the Decimal onward")
    } else {
        None
    }
}

/// The built-in conversion from `src` to `dst` (base names), or None. The identity is not a conversion: the caller
/// rejects it before asking (E-TYP-181).
pub fn resolve(src: &str, dst: &str) -> Option<Conversion> {
    let row = |kind: &'static str, fallible: bool| Some(Conversion { kind, source: src.to_string(), target: dst.to_string(), fallible });
    if is_int(src) && is_int(dst) {
        let ((slo, shi), (dlo, dhi)) = (int_types(src).unwrap(), int_types(dst).unwrap());
        return if dlo <= slo && shi <= dhi { row("int_widen", false) } else { row("int_narrow", true) };
    }
    if is_int(src) && dst == "Decimal" {
        return row("int_to_decimal", false);
    }
    if src == "Decimal" && is_int(dst) {
        return row("decimal_to_int", true);
    }
    if is_int(src) && dst == "Float" {
        let (lo, hi) = int_types(src).unwrap();
        let exact = FLOAT_EXACT.0 <= lo && hi <= FLOAT_EXACT.1;
        return row("int_to_float", !exact);
    }
    if (is_int(src) || DISPLAYED.contains(&src)) && dst == "String" {
        return row("to_string", false);
    }
    if TEXT.contains(&src) && is_int(dst) {
        return row("parse_int", true);
    }
    if TEXT.contains(&src) && dst == "Decimal" {
        return row("parse_decimal", true);
    }
    if TEXT.contains(&src) && dst == "Bool" {
        return row("parse_bool", true);
    }
    if src == "StringView" && dst == "String" {
        return row("view_to_string", false);
    }
    if BYTES.contains(&src) && dst == "String" {
        return row("bytes_to_string", true);
    }
    if TEXT.contains(&src) && dst == "Bytes" {
        return row("string_to_bytes", false);
    }
    if src == "Scalar" && dst == "Int" {
        return row("scalar_to_int", false);
    }
    if src == "Int" && dst == "Scalar" {
        return row("int_to_scalar", true);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widening_is_total_and_narrowing_fallible() {
        assert_eq!(resolve("Int8", "Int").unwrap().kind, "int_widen");
        assert!(!resolve("UInt8", "Int16").unwrap().fallible);
        assert!(resolve("Int", "Int8").unwrap().fallible);
        assert!(resolve("UInt64", "Int").unwrap().fallible);
        assert!(resolve("Int", "UInt").unwrap().fallible);
        assert!(same_builtin("Int", "Int64"));
        assert!(same_builtin("Byte", "UInt8"));
    }

    #[test]
    fn floats_round_by_choice() {
        assert!(resolve("Float", "Int").is_none());
        assert!(rounding_hint("Float", "Decimal").is_some());
        assert!(!resolve("Int32", "Float").unwrap().fallible);
        assert!(resolve("Int", "Float").unwrap().fallible);
    }

    #[test]
    fn text_rows() {
        assert_eq!(resolve("Int", "String").unwrap().kind, "to_string");
        assert_eq!(resolve("StringView", "Int8").unwrap().kind, "parse_int");
        assert_eq!(resolve("Bytes", "String").unwrap().intrinsic(), "as:bytes_to_string:Bytes:String");
        assert!(resolve("String", "Float").is_none());
        assert!(resolve("Bool", "Int").is_none());
    }
}
