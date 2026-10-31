use fortis_testkit::Json;
use fortis_types::{
    types::{NamedT, Type},
};

fn text(data: &Json, key: &str) -> String {
    data.get(key).and_then(Json::as_str).unwrap_or("").into()
}

fn typ(data: &Json) -> Type {
    if data.get("name").is_some() {
        return Type::Named(NamedT {
            name: text(data, "name"),
            args: data
                .get("args")
                .and_then(Json::as_arr)
                .map(|xs| xs.iter().map(typ).collect())
                .unwrap_or_default(),
            package: text(data, "package"),
            origin: text(data, "origin"),
        });
    }
    if let Some(x) = data.get("optional") {
        return Type::Optional(Box::new(typ(x)));
    }
    if let Some(x) = data.get("tuple") {
        return Type::Tuple(x.as_arr().unwrap().iter().map(typ).collect());
    }
    if let Some(x) = data.get("record") {
        return Type::Record(
            x.as_arr()
                .unwrap()
                .iter()
                .map(|row| {
                    let pair = row.as_arr().unwrap();
                    (pair[0].as_str().unwrap().into(), typ(&pair[1]))
                })
                .collect(),
        );
    }
    if let Some(x) = data.get("fn") {
        let pair = x.as_arr().unwrap();
        return Type::Fn {
            params: pair[0].as_arr().unwrap().iter().map(typ).collect(),
            ret: Box::new(typ(&pair[1])),
            kind: "fn".into(),
            modifiers: vec![],
            origin: String::new(),
            dynamic: false,
            identity: String::new(),
            capture_origins: vec![],
            param_modes: vec![],
        };
    }
    if let Some(x) = data.get("loan") {
        return Type::Loan {
            inner: Box::new(typ(x)),
            mode: "alias".into(),
            origin: "owner".into(),
        };
    }
    if let Some(x) = data.get("dynamic") {
        return Type::Dynamic(typ(x).named_ref().unwrap().clone());
    }
    if let Some(x) = data.get("refined") {
        return Type::Refined {
            base: typ(x).named_ref().unwrap().clone(),
            lo: Some(0),
            hi: Some(10),
            kind: "range".into(),
        };
    }
    panic!("invalid type case: {data:?}");
}

fn check(input: &Json, expect: &Json) {
    let a = typ(input.get("a").unwrap());
    let b = typ(input.get("b").unwrap());
    let before = (a.show(), b.show());
    let shown = fortis_types::types::diagnostic_types(&[&a, &b]);
    let wanted: Vec<_> = expect.as_arr().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(shown, wanted);
    assert_eq!((a.show(), b.show()), before);
}
fortis_testkit::case_tests!("mopup/diagnostics.json", check;
    prelude_shadow,
    map_key,
    separate_modules,
    separate_versions,
    unambiguous,
    same_identity,
    core_modules,
    same_spelling_in_one,
    optional,
    function,
    loan,
    record,
    refinement,
    dynamic
);
