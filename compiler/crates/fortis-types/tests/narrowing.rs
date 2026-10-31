//! LR 6.3: narrowing computes a sound interval for every comparison.
use fortis_testkit::Json;
use fortis_types::intervals;
fn interval(j: &Json) -> (i128, i128) {
    let a = j.as_arr().unwrap(); (a[0].as_i64().unwrap() as i128, a[1].as_i64().unwrap() as i128)
}
fn check(input: &Json, expect: &Json) {
    let got = intervals::narrow(input.get("op").unwrap().as_str().unwrap(), interval(input.get("p").unwrap()), interval(input.get("c").unwrap()));
    let got = got.map(|(a,b)| Json::Arr(vec![Json::Int(a as i64), Json::Int(b as i64)])).unwrap_or(Json::Null);
    assert_eq!(&got, expect);
}
#[test]
fn lr63_all_comparisons_contain_satisfying_values() {
    fortis_testkit::forall(|r| (r.between(-8,0), r.between(0,8), r.between(-8,8)), 200, None, |&(lo,hi,c)| {
        for op in ["<", "<=", ">", ">=", "==", "!="] {
            let values: Vec<_> = (lo..=hi).filter(|&x| match op { "<"=>x<c,"<="=>x<=c,">"=>x>c,">="=>x>=c,"=="=>x==c,_=>x!=c }).collect();
            let expected = values.first().zip(values.last()).map(|(&a,&b)| (a as i128,b as i128));
            assert_eq!(intervals::narrow(op, (lo as i128,hi as i128), (c as i128,c as i128)), expected);
        }
    });
}
fortis_testkit::case_tests!("types/narrowing.json", check;
    lr63_narrow_lt_minus1,
    lr63_narrow_lt_0,
    lr63_narrow_lt_2,
    lr63_narrow_lt_4,
    lr63_narrow_lt_5,
    lr63_narrow_le_minus1,
    lr63_narrow_le_0,
    lr63_narrow_le_2,
    lr63_narrow_le_4,
    lr63_narrow_le_5,
    lr63_narrow_gt_minus1,
    lr63_narrow_gt_0,
    lr63_narrow_gt_2,
    lr63_narrow_gt_4,
    lr63_narrow_gt_5,
    lr63_narrow_ge_minus1,
    lr63_narrow_ge_0,
    lr63_narrow_ge_2,
    lr63_narrow_ge_4,
    lr63_narrow_ge_5,
    lr63_narrow_eq_minus1,
    lr63_narrow_eq_0,
    lr63_narrow_eq_2,
    lr63_narrow_eq_4,
    lr63_narrow_eq_5,
    lr63_narrow_ne_minus1,
    lr63_narrow_ne_0,
    lr63_narrow_ne_2,
    lr63_narrow_ne_4,
    lr63_narrow_ne_5
);
