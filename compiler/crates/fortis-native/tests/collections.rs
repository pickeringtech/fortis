//! LR 6.7: the shared operation sequences against the typed native helpers.
use fortis_native::{
    collections::*,
    failure::{Dyn, Failure},
};
use fortis_testkit::Json;
fn ints(j: &Json) -> Vec<i64> {
    j.as_arr()
        .unwrap()
        .iter()
        .map(|x| x.as_i64().unwrap())
        .collect()
}
fn arr(xs: &[i64]) -> Json {
    Json::Arr(xs.iter().copied().map(Json::Int).collect())
}
fn optional(x: Option<i64>) -> Json {
    x.map(Json::Int).unwrap_or(Json::Null)
}
fn failure(f: Failure) -> Json {
    Json::obj([
        ("code", Json::str(f.code)),
        (
            "payload",
            Json::obj(f.payload.into_iter().map(|(k, v)| {
                (
                    k,
                    match v {
                        Dyn::Int(n) => Json::Int(n as i64),
                        _ => panic!("index payload"),
                    },
                )
            })),
        ),
    ])
}
fn check(d: &Json, expect: &Json) {
    let typ = d.get("type").unwrap().as_str().unwrap();
    let init = d.get("initial").unwrap();
    let mut xs = if typ == "List" || typ == "Array" {
        ints(init)
    } else {
        vec![]
    };
    let mut map = OrderedMap::<i64, i64>::new();
    let mut set = OrderedSet::<i64>::new();
    if typ == "Map" {
        for p in init.as_arr().unwrap() {
            let p = ints(p);
            map.set(p[0], p[1]);
        }
    }
    if typ == "Set" {
        for x in ints(init) {
            set.add(x);
        }
    }
    let mut results = vec![];
    for op in d.get("ops").unwrap().as_arr().unwrap() {
        let a = op.as_arr().unwrap();
        let op = a[0].as_str().unwrap();
        let n = |i: usize| a[i].as_i64().unwrap();
        let result = match (typ, op) {
            ("List" | "Array", "get") => {
                let owned = list_get(&xs, n(1));
                assert_eq!(owned, list_get_ref(&xs, n(1)).copied());
                optional(owned)
            }
            ("List", "at") => list_at(&xs, n(1)).map(Json::Int).unwrap_or_else(failure),
            ("List" | "Array", "set") => list_set(&mut xs, n(1), n(2))
                .map(|_| Json::Null)
                .unwrap_or_else(failure),
            ("List" | "Array", "count") => Json::Int(count(&xs)),
            ("List", "push") => {
                xs.push(n(1));
                Json::Null
            }
            ("List", "pop") => optional(xs.pop()),
            ("List", "clear") => {
                xs.clear();
                Json::Null
            }
            ("List", "iterate") => {
                let mut c = Cursor::new(&xs);
                Json::Arr(
                    (0..xs.len() + 2)
                        .map(|_| optional(c.next().copied()))
                        .collect(),
                )
            }
            ("Deque", "iterate") => {
                let p = init.as_arr().unwrap();
                let front = ints(&p[0]);
                let back = ints(&p[1]);
                let mut c = Cursor::joined(&front, &back);
                Json::Arr(
                    (0..front.len() + back.len() + 2)
                        .map(|_| optional(c.next().copied()))
                        .collect(),
                )
            }
            ("Map", "set") => {
                map.set(n(1), n(2));
                Json::Null
            }
            ("Map", "get") => optional(map.get(&n(1)).copied()),
            ("Map", "containsKey") => Json::Bool(map.contains_key(&n(1))),
            ("Map", "remove") => optional(map.remove(&n(1))),
            ("Map", "count") => Json::Int(map.count()),
            ("Set", "add") => Json::Bool(set.add(n(1))),
            ("Set", "contains") => Json::Bool(set.contains(&n(1))),
            ("Set", "remove") => Json::Bool(set.remove(&n(1))),
            ("Set", "count") => Json::Int(set.count()),
            _ => panic!("unsupported {typ}.{op}"),
        };
        results.push(result);
    }
    let items = match typ {
        "Map" => Json::Arr(map.iter().map(|(k, v)| arr(&[*k, *v])).collect()),
        "Set" => arr(&set.into_items()),
        "Deque" => init.clone(),
        _ => arr(&xs),
    };
    assert_eq!(
        &Json::obj([("results", Json::Arr(results)), ("items", items)]),
        expect
    );
}
#[test]
fn lr6_7_map_replacement_order_property() {
    fortis_testkit::forall(
        |r| (0..30).map(|_| r.between(-4, 4)).collect::<Vec<_>>(),
        200,
        None,
        |values| {
            let mut m = OrderedMap::new();
            let mut first = vec![];
            for (i, n) in values.iter().enumerate() {
                if !first.contains(n) {
                    first.push(*n);
                }
                m.set(*n, i);
            }
            assert_eq!(m.iter().map(|(k, _)| *k).collect::<Vec<_>>(), first);
            for n in first {
                assert_eq!(m.remove(&n), values.iter().rposition(|x| *x == n));
            }
            assert_eq!(m.count(), 0);
        },
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
