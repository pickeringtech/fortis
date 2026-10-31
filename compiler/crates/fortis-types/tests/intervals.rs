use fortis_testkit::{forall, Json, Rng};
use fortis_types::{
    intervals as iv,
    literals::assignable,
    types::{named, nt, refined, Type},
};

fn interval(j: &Json) -> iv::Interval {
    let a = j.as_arr().unwrap();
    (
        a[0].as_i64().unwrap() as i128,
        a[1].as_i64().unwrap() as i128,
    )
}
fn typ(j: &Json) -> Type {
    if *j == Json::Null {
        named("Int")
    } else {
        let (lo, hi) = interval(j);
        refined(nt("Int"), Some(lo), Some(hi), "range")
    }
}
fn check(input: &Json, expect: &Json) {
    let op = input.get("op").unwrap().as_str().unwrap();
    let a = input.get("a").unwrap();
    let b = input.get("b").unwrap();
    let actual = if op == "subtype" {
        assignable(&typ(a), &typ(b))
            .0
            .map(Json::Bool)
            .unwrap_or(Json::Null)
    } else {
        let result = if op == "meet" {
            iv::meet(interval(a), interval(b))
        } else {
            iv::arith(op, interval(a), interval(b), 64)
        };
        result
            .map(|(lo, hi)| {
                Json::Arr(vec![
                    Json::Int(lo.try_into().unwrap()),
                    Json::Int(hi.try_into().unwrap()),
                ])
            })
            .unwrap_or(Json::Null)
    };
    assert_eq!(&actual, expect);
}
fn operands(rng: &mut Rng) -> (iv::Interval, iv::Interval) {
    let mut one = || {
        let a = rng.between(-20, 20) as i128;
        let b = rng.between(-20, 20) as i128;
        (a.min(b), a.max(b))
    };
    (one(), one())
}
fn contains_results(op: &str) {
    forall(operands, 200, None, |&(a, b)| {
        let (lo, hi) = iv::arith(op, a, b, 64).unwrap();
        for x in a.0..=a.1 {
            for y in b.0..=b.1 {
                let result = match op {
                    "+" => x + y,
                    "-" => x - y,
                    "*" => x * y,
                    _ => unreachable!(),
                };
                assert!(
                    lo <= result && result <= hi,
                    "{x} {op} {y} = {result} outside [{lo}, {hi}]"
                );
            }
        }
    });
}
#[test]
fn add_contains_concrete_results() {
    contains_results("+");
}
#[test]
fn subtract_contains_concrete_results() {
    contains_results("-");
}
#[test]
fn multiply_contains_concrete_results() {
    contains_results("*");
}

fortis_testkit::case_tests!("intervals/arithmetic.json", check;
    add_positive,
    add_negative,
    add_cross_zero,
    add_identity,
    subtract_positive,
    subtract_negative,
    subtract_cross_zero,
    subtract_singletons,
    multiply_positive,
    multiply_negative,
    multiply_opposite_signs,
    multiply_cross_zero,
    multiply_zero,
    containment_nested,
    containment_touching,
    containment_disjoint,
    subtype_narrower,
    subtype_equal,
    subtype_wider_unproved,
    subtype_disjoint_unproved,
    subtype_to_base,
    subtype_from_base_unproved
);
