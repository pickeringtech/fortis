//! LR 6.7: direct intrinsic calls, preserving every intermediate result and final order.
use fortis_mir::model::Program;
use fortis_testkit::Json;
use fortis_vm::{
    intrinsics,
    machine::Vm,
    sync::Rc,
    value::{shared, Obj, Value},
};
fn value(j: &Json) -> Value {
    match j {
        Json::Int(n) => Value::Int(*n as i128),
        Json::Arr(xs) => Value::list(xs.iter().map(value).collect()),
        _ => panic!("integer/list input"),
    }
}
fn normal(v: &Value) -> Json {
    match v {
        Value::Void | Value::Null => Json::Null,
        Value::Int(n) => Json::Int(*n as i64),
        Value::Bool(b) => Json::Bool(*b),
        Value::Str(s) => Json::str(&**s),
        Value::List(xs) | Value::Array(xs) => Json::Arr(xs.borrow().iter().map(normal).collect()),
        Value::Map(m) => Json::Arr(
            m.borrow()
                .entries
                .iter()
                .map(|(k, v)| Json::Arr(vec![normal(k), normal(v)]))
                .collect(),
        ),
        Value::Set(m) => Json::Arr(
            m.borrow()
                .entries
                .iter()
                .map(|(k, _)| normal(k))
                .collect(),
        ),
        Value::Obj(o) => {
            let o = o.borrow();
            Json::Arr(vec![
                normal(o.get("front").unwrap()),
                normal(o.get("back").unwrap()),
            ])
        }
        Value::Failure(f) => {
            let f = f.borrow();
            Json::obj([
                ("code", Json::str(&*f.code)),
                (
                    "payload",
                    Json::obj(f.payload.iter().map(|(k, v)| (&**k, normal(v)))),
                ),
            ])
        }
        _ => panic!("unexpected value {v:?}"),
    }
}
fn check(d: &Json, expect: &Json) {
    let prog = Program::default();
    let mut vm = Vm::new(&prog, "unit");
    let typ = d.get("type").unwrap().as_str().unwrap();
    let init = d.get("initial").unwrap().as_arr().unwrap();
    let recv = match typ {
        "List" => Value::list(init.iter().map(value).collect()),
        "Array" => Value::Array(shared(init.iter().map(value).collect())),
        "Deque" => Value::Obj(shared(Obj::new(
            Rc::from("Deque"),
            vec![
                (Rc::from("front"), value(&init[0])),
                (Rc::from("back"), value(&init[1])),
            ],
        ))),
        _ => {
            let recv = intrinsics::call(&mut vm, &format!("{typ}.default"), vec![]).unwrap();
            for row in init {
                let mut args = vec![recv.clone()];
                if typ == "Map" {
                    args.extend(row.as_arr().unwrap().iter().map(value));
                } else {
                    args.push(value(row));
                }
                intrinsics::call(
                    &mut vm,
                    &format!("{typ}.{}", if typ == "Map" { "set" } else { "add" }),
                    args,
                )
                .unwrap();
            }
            recv
        }
    };
    let mut results = vec![];
    for op in d.get("ops").unwrap().as_arr().unwrap() {
        let args = op.as_arr().unwrap();
        let name = args[0].as_str().unwrap();
        let mut vals = vec![recv.clone()];
        vals.extend(args[1..].iter().map(value));
        let got = intrinsics::call(&mut vm, &format!("{typ}.{name}"), vals).unwrap();
        if name == "iterate" {
            let n = if typ == "Deque" {
                init.iter().map(|a| a.as_arr().unwrap().len()).sum()
            } else {
                init.len()
            };
            results.push(Json::Arr(
                (0..n + 2)
                    .map(|_| {
                        normal(
                            &intrinsics::call(&mut vm, "ListCursor.next", vec![got.clone()])
                                .unwrap(),
                        )
                    })
                    .collect(),
            ));
        } else {
            results.push(normal(&got));
        }
    }
    assert_eq!(
        &Json::obj([("results", Json::Arr(results)), ("items", normal(&recv))]),
        expect
    );
}

fortis_testkit::case_tests!("collections/intrinsics.json",check;
    lr6_7_list_empty_index_neg1,
    lr6_7_list_empty_index_0,
    lr6_7_list_empty_index_1,
    lr6_7_list_empty_push_pop,
    lr6_7_list_empty_drain,
    lr6_7_list_empty_clear,
    lr6_7_list_empty_cursor,
    lr6_7_array_empty_get_neg1,
    lr6_7_array_empty_get_0,
    lr6_7_list_one_index_neg1,
    lr6_7_list_one_index_0,
    lr6_7_list_one_index_1,
    lr6_7_list_one_index_2,
    lr6_7_list_one_push_pop,
    lr6_7_list_one_drain,
    lr6_7_list_one_clear,
    lr6_7_list_one_cursor,
    lr6_7_array_one_get_neg1,
    lr6_7_array_one_get_0,
    lr6_7_array_one_get_1,
    lr6_7_array_one_replace,
    lr6_7_list_duplicates_index_neg1,
    lr6_7_list_duplicates_index_0,
    lr6_7_list_duplicates_index_2,
    lr6_7_list_duplicates_index_3,
    lr6_7_list_duplicates_index_4,
    lr6_7_list_duplicates_push_pop,
    lr6_7_list_duplicates_drain,
    lr6_7_list_duplicates_clear,
    lr6_7_list_duplicates_cursor,
    lr6_7_array_duplicates_get_neg1,
    lr6_7_array_duplicates_get_0,
    lr6_7_array_duplicates_get_2,
    lr6_7_array_duplicates_get_3,
    lr6_7_array_duplicates_replace,
    lr6_7_list_ordered_index_neg1,
    lr6_7_list_ordered_index_0,
    lr6_7_list_ordered_index_2,
    lr6_7_list_ordered_index_3,
    lr6_7_list_ordered_index_4,
    lr6_7_list_ordered_push_pop,
    lr6_7_list_ordered_drain,
    lr6_7_list_ordered_clear,
    lr6_7_list_ordered_cursor,
    lr6_7_array_ordered_get_neg1,
    lr6_7_array_ordered_get_0,
    lr6_7_array_ordered_get_2,
    lr6_7_array_ordered_get_3,
    lr6_7_array_ordered_replace,
    lr6_7_map_empty_remove_1,
    lr6_7_map_empty_replace_1,
    lr6_7_map_empty_remove_2,
    lr6_7_map_empty_replace_2,
    lr6_7_map_empty_remove_9,
    lr6_7_map_empty_replace_9,
    lr6_7_map_one_remove_1,
    lr6_7_map_one_replace_1,
    lr6_7_map_one_remove_2,
    lr6_7_map_one_replace_2,
    lr6_7_map_one_remove_9,
    lr6_7_map_one_replace_9,
    lr6_7_map_ordered_remove_1,
    lr6_7_map_ordered_replace_1,
    lr6_7_map_ordered_remove_2,
    lr6_7_map_ordered_replace_2,
    lr6_7_map_ordered_remove_9,
    lr6_7_map_ordered_replace_9,
    lr6_7_map_remove_reinsert_moves_to_end,
    lr6_7_set_empty_add_1,
    lr6_7_set_empty_remove_1,
    lr6_7_set_empty_add_2,
    lr6_7_set_empty_remove_2,
    lr6_7_set_empty_add_9,
    lr6_7_set_empty_remove_9,
    lr6_7_set_one_add_1,
    lr6_7_set_one_remove_1,
    lr6_7_set_one_add_2,
    lr6_7_set_one_remove_2,
    lr6_7_set_one_add_9,
    lr6_7_set_one_remove_9,
    lr6_7_set_ordered_add_1,
    lr6_7_set_ordered_remove_1,
    lr6_7_set_ordered_add_2,
    lr6_7_set_ordered_remove_2,
    lr6_7_set_ordered_add_9,
    lr6_7_set_ordered_remove_9,
    lr6_7_set_remove_reinsert_moves_to_end,
    lr6_7_deque_cursor_empty,
    lr6_7_deque_cursor_front,
    lr6_7_deque_cursor_back,
    lr6_7_deque_cursor_both,
    lr6_7_deque_cursor_duplicates
);
