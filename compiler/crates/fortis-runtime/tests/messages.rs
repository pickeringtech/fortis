//! LR 10.3: named substitutions are rendered once, with unknown names preserved.
use fortis_testkit::Json;
fn check(d: &Json, expect: &Json) {
    let Json::Obj(payload) = d.get("payload").unwrap() else {
        panic!("payload object")
    };
    let fields: Vec<_> = payload
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str().unwrap()))
        .collect();
    assert_eq!(
        Json::str(fortis_runtime::failure::render_message(
            d.get("template").unwrap().as_str().unwrap(),
            &fields
        )),
        *expect
    );
}

fortis_testkit::case_tests!("runtime/messages.json", check;
    lr10_3_message_0,
    lr10_3_message_1,
    lr10_3_message_2,
    lr10_3_message_3,
    lr10_3_message_4,
    lr10_3_message_5,
    lr10_3_message_6,
    lr10_3_message_7,
    lr10_3_message_8,
    lr10_3_message_9
);
