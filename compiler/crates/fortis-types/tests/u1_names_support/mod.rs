use std::sync::Arc;
use fortis_names::{Library, library::{Libraries, LibraryModule}};
use fortis_syntax::parse_module;
use fortis_testkit::Json;
pub fn libraries() -> Libraries {
    let rows = fortis_testkit::read_cases("names/modules.json");
    let Json::Obj(sources) = &rows[0].input else { panic!("modules") };
    let modules = sources.iter().map(|(name,src)| {
        let parsed = Arc::new(parse_module(src.as_str().unwrap()).unwrap());
        (name.clone(), Arc::new(LibraryModule::new(name, "fortis", parsed, None)))
    }).collect();
    Libraries { libs: vec![Library { modules }] }
}
