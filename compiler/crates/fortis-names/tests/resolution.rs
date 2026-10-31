//! LR 4.3-4.5: resolve names against in-memory module signatures.
use std::collections::BTreeMap;
use std::sync::Arc;
use fortis_names::{Library, library::LibraryModule, resolver::Resolver};
use fortis_syntax::{parse_module, view::N};
use fortis_testkit::Json;
fn library() -> Library {
    let rows = fortis_testkit::read_cases("names/modules.json");
    let Json::Obj(sources) = &rows[0].input else { panic!("modules") };
    let modules: BTreeMap<_,_> = sources.iter().map(|(name,src)| {
        let parsed = Arc::new(parse_module(src.as_str().unwrap()).unwrap());
        (name.clone(), Arc::new(LibraryModule::new(name, "fortis", parsed, None)))
    }).collect();
    Library { modules }
}
fn check(input: &Json, expect: &Json) {
    let lib = library();
    let parsed = parse_module(input.as_str().unwrap()).unwrap();
    let mut resolver = Resolver::new(&lib, "unit.ft", false);
    let records = resolver.resolve_module(N::new(&parsed.root,&parsed.toks));
    let mut codes: Vec<_> = records.iter().map(|r| r.code.clone()).collect();
    codes.sort(); codes.dedup();
    assert_eq!(&Json::strs(codes), expect, "{records:?}");
}
#[test]
fn lr45_nested_let_cannot_shadow_parameter() {
    fortis_testkit::forall(|r| r.between(1,12), 40, None, |&depth| {
        let source = format!("func f(value: Int) {{ {}let value = 1{}", "{\n".repeat(depth as usize), "\n}".repeat(depth as usize+1));
        check(&Json::str(source), &Json::strs(["E-NAM-003"]));
    });
}
#[test]
fn lr45_signature_prelude_exports() {
    let lib = library();
    let prelude = &lib.modules["fortis.prelude"];
    let names = prelude.decls.iter().filter(|(_,d)| d.exported).map(|(n,_)| n.clone());
    assert_eq!(Json::strs(names), fortis_testkit::read_cases("names/modules.json")[0].expect);
}
fortis_testkit::case_tests!("names/resolution.json", check;
    lr4_plain,
    lr4_alias,
    lr4_select,
    lr4_select_type,
    lr4_qualified_type,
    lr4_select_no_alias,
    lr4_alias_no_old_name,
    lr4_duplicate_alias,
    lr4_duplicate_select,
    lr4_duplicate_same,
    lr4_collision,
    lr4_missing_select,
    lr4_two_aliases,
    lr4_module_type_shadow,
    lr4_forward,
    lr4_constant,
    lr4_duplicate_decl,
    lr4_missing_value,
    lr4_missing_type,
    lr4_bare_field,
    lr4_self_field,
    lr4_generic,
    lr4_class_generic,
    lr4_param,
    lr4_branch_scope,
    lr4_for_scope,
    lr4_match_binding,
    lr4_match_scope,
    lr45_param_shadow_depth_0,
    lr45_param_distinct_depth_0,
    lr45_const_shadow_depth_0,
    lr45_const_distinct_depth_0,
    lr45_local_shadow_depth_0,
    lr45_local_distinct_depth_0,
    lr45_param_shadow_depth_1,
    lr45_param_distinct_depth_1,
    lr45_const_shadow_depth_1,
    lr45_const_distinct_depth_1,
    lr45_local_shadow_depth_1,
    lr45_local_distinct_depth_1,
    lr45_param_shadow_depth_2,
    lr45_param_distinct_depth_2,
    lr45_const_shadow_depth_2,
    lr45_const_distinct_depth_2,
    lr45_local_shadow_depth_2,
    lr45_local_distinct_depth_2,
    lr45_param_shadow_depth_3,
    lr45_param_distinct_depth_3,
    lr45_const_shadow_depth_3,
    lr45_const_distinct_depth_3,
    lr45_local_shadow_depth_3,
    lr45_local_distinct_depth_3,
    lr45_prelude_int,
    lr45_prelude_bool,
    lr45_prelude_void,
    lr45_prelude_string,
    lr45_prelude_stringview,
    lr45_prelude_list,
    lr45_prelude_map,
    lr45_prelude_decimal,
    lr45_prelude_duration,
    lr45_import_required_item,
    lr45_import_required_other,
    lr36_interpolation_known,
    lr36_interpolation_missing,
    lr36_escaped_interpolation
);
