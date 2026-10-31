//! LR 5.6: assert grouping, not merely acceptance of an operator sequence.
use fortis_syntax::{lex, Parser, Node, Child, Token};
use fortis_testkit::Json;
fn child(c: &Child, toks: &[Token]) -> Json {
    match c { Child::Node(n) => shape(n, toks), Child::Tok(i) => Json::str(&toks[*i].text) }
}
fn shape(n: &Node, toks: &[Token]) -> Json {
    let cs = &n.children;
    let ops = ["asExpr", "multiplicativeExpr", "additiveExpr", "shiftExpr", "relationalExpr",
        "equalityExpr", "bitAndExpr", "bitXorExpr", "bitOrExpr", "andExpr", "orExpr", "coalesceExpr", "unaryExpr"];
    if ops.contains(&n.name()) && cs.len() > 1 {
        if n.name() == "unaryExpr" { return Json::Arr(vec![child(&cs[0], toks), child(&cs[1], toks)]); }
        let mut result = child(&cs[0], toks);
        for i in (1..cs.len()).step_by(2) {
            result = Json::Arr(vec![child(&cs[i], toks), result, child(&cs[i+1], toks)]);
        }
        return result;
    }
    if n.name() == "primaryExpr" && matches!(&cs[0], Child::Tok(i) if toks[*i].text == "(") {
        return child(&cs[1], toks);
    }
    assert_eq!(cs.len(), 1, "unhandled shape {}", n.name());
    child(&cs[0], toks)
}
fn check(input: &Json, expect: &Json) {
    let toks = lex(input.as_str().unwrap()).unwrap();
    let (root, _, _) = Parser::new(&toks).parse("expr").unwrap();
    assert_eq!(&shape(&root, &toks), expect);
}
#[test]
fn lr56_parentheses_preserve_grouping() {
    fortis_testkit::forall(|r| r.between(1, 20), 40, None, |&n| {
        let source = format!("{}a + b * c{}", "(".repeat(n as usize), ")".repeat(n as usize));
        check(&Json::str(source), &Json::parse(r#"["+", "a", ["*", "b", "c"]]"#).unwrap());
    });
}
fortis_testkit::case_tests!("parser/precedence.json", check;
    lr56_tighter_left_0_1,
    lr56_tighter_right_0_1,
    lr56_tighter_left_0_2,
    lr56_tighter_right_0_2,
    lr56_tighter_left_0_3,
    lr56_tighter_right_0_3,
    lr56_tighter_left_0_4,
    lr56_tighter_right_0_4,
    lr56_tighter_left_0_5,
    lr56_tighter_right_0_5,
    lr56_tighter_left_0_6,
    lr56_tighter_right_0_6,
    lr56_tighter_left_0_7,
    lr56_tighter_right_0_7,
    lr56_tighter_left_1_2,
    lr56_tighter_right_1_2,
    lr56_tighter_left_1_3,
    lr56_tighter_right_1_3,
    lr56_tighter_left_1_4,
    lr56_tighter_right_1_4,
    lr56_tighter_left_1_5,
    lr56_tighter_right_1_5,
    lr56_tighter_left_1_6,
    lr56_tighter_right_1_6,
    lr56_tighter_left_1_7,
    lr56_tighter_right_1_7,
    lr56_tighter_left_2_3,
    lr56_tighter_right_2_3,
    lr56_tighter_left_2_4,
    lr56_tighter_right_2_4,
    lr56_tighter_left_2_5,
    lr56_tighter_right_2_5,
    lr56_tighter_left_2_6,
    lr56_tighter_right_2_6,
    lr56_tighter_left_2_7,
    lr56_tighter_right_2_7,
    lr56_tighter_left_3_4,
    lr56_tighter_right_3_4,
    lr56_tighter_left_3_5,
    lr56_tighter_right_3_5,
    lr56_tighter_left_3_6,
    lr56_tighter_right_3_6,
    lr56_tighter_left_3_7,
    lr56_tighter_right_3_7,
    lr56_tighter_left_4_5,
    lr56_tighter_right_4_5,
    lr56_tighter_left_4_6,
    lr56_tighter_right_4_6,
    lr56_tighter_left_4_7,
    lr56_tighter_right_4_7,
    lr56_tighter_left_5_6,
    lr56_tighter_right_5_6,
    lr56_tighter_left_5_7,
    lr56_tighter_right_5_7,
    lr56_tighter_left_6_7,
    lr56_tighter_right_6_7,
    lr56_left_00,
    lr56_left_01,
    lr56_left_02,
    lr56_left_03,
    lr56_left_04,
    lr56_left_05,
    lr56_left_06,
    lr56_left_07,
    lr56_left_08,
    lr56_left_09,
    lr56_left_10,
    lr56_left_11,
    lr56_coalesce,
    lr56_logical,
    lr56_parentheses,
    lr56_conversion,
    lr56_conversion_chain,
    lr56_unary
);
