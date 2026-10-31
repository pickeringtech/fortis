use fortis_testkit::Json;
use fortis_types::types::*;

pub fn typ(j: &Json) -> Type {
    if let Some(s) = j.as_str() { return named(s); }
    let a = j.as_arr().unwrap();
    match a[0].as_str().unwrap() {
        "ref" => refined(nt(a[1].as_str().unwrap()), Some(a[2].as_i64().unwrap() as i128), Some(a[3].as_i64().unwrap() as i128), a.get(4).and_then(Json::as_str).unwrap_or("range")),
        "opt" => optional(typ(&a[1])),
        "tuple" => Type::Tuple(a[1..].iter().map(typ).collect()),
        "record" => Type::Record(a[1..].iter().map(|j| { let p = j.as_arr().unwrap(); (p[0].as_str().unwrap().into(), typ(&p[1])) }).collect()),
        "param" => Type::TypeParam(a[1].as_str().unwrap().into()),
        "named" => named_args(a[1].as_str().unwrap(), a[2..].iter().map(typ).collect()),
        tag => panic!("unknown type tag {tag}"),
    }
}
