//! Declaration table and declaration-level checks (DEL-10): a port of checker/decls.py.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::rc::Rc;

use fortis_diag::{class_of, Json, Record};
use fortis_names::{Libraries, LibraryModule};
use fortis_syntax::lexer::{Kind, Token};
use fortis_syntax::view::{declarations, imports, DeclInfo, C, N};

use crate::literals::parse_int;
use crate::types::*;

pub fn builtin_implements(name: &str, iface: &str) -> bool {
    let full: &[&str] = &["Equatable", "Hashable", "Comparable", "Displayable", "Encodable", "Decodable", "Default", "Copyable"];
    let set: &[&str] = match name {
        "Int" | "Bool" | "String" => full,
        n if is_int_type(n) => full,
        "StringView" => &["Equatable", "Hashable", "Comparable", "Displayable"],
        "Bytes" => &["Equatable", "Hashable", "Encodable", "Decodable", "Default", "Copyable"],
        "Float" | "Float32" => &["Displayable", "Encodable", "Decodable", "Default", "Copyable"],
        "Secret" => &["Displayable"],
        "Decimal" => &["Equatable", "Hashable", "Comparable", "Displayable", "Encodable", "Decodable", "Copyable"],
        "Duration" => &["Equatable", "Hashable", "Comparable", "Displayable", "Encodable", "Decodable", "Default", "Copyable"],
        "Timestamp" => &["Equatable", "Hashable", "Comparable", "Displayable", "Encodable", "Decodable", "Copyable"],
        "Instant" => &["Equatable", "Hashable", "Comparable", "Copyable"],
        "Scalar" => &["Equatable", "Hashable", "Comparable", "Displayable", "Encodable", "Decodable", "Copyable"],
        "Void" => &["Equatable", "Copyable"],
        "Money" | "Currency" => &["Equatable", "Hashable", "Displayable", "Encodable", "Decodable", "Copyable"],
        "Path" => &["Equatable", "Hashable", "Displayable", "Copyable"],
        "Report" => &["Equatable", "Copyable"],
        _ => &[],
    };
    set.contains(&iface)
}
/// Whether a package or module name is the standard package's (`fortis`, `fortis.prelude`, `fortis.http.server`): a
/// user package may be named `fortissimo`, which is not (D-133).
pub fn is_standard(name: &str) -> bool {
    name == "fortis" || name.starts_with("fortis.") || name.starts_with("fortis@")
}

/// The declaring module recorded on a core type. A prelude-only type stays anonymous. A prelude declaration of a
/// name one other standard module also declares carries that module (`core_home`): the two copies are one type.
pub fn core_origin_of(d: &Decl) -> String {
    if !d.type_identity().is_empty() || !d.module.starts_with("fortis") {
        String::new()
    } else if d.module != "fortis.prelude" {
        d.module.clone()
    } else {
        d.core_home.clone()
    }
}

/// The type a declaration is. A non-prelude core type keeps its module as origin, the stamp `named_of` gives a
/// type reference, so a constructor, a variant and a signature name one type.
pub fn named_of_decl(d: &Decl, args: Vec<Type>) -> Type {
    Type::Named(NamedT { name: d.name.clone(), args, package: d.type_identity(), origin: core_origin_of(d) })
}

pub fn has_builtin_implements(name: &str) -> bool {
    builtin_implements(name, "Copyable") || builtin_implements(name, "Displayable") || builtin_implements(name, "Equatable")
}
pub const BUILTIN_IFACES: &[&str] = &[
    "Equatable", "Hashable", "Comparable", "Displayable", "Iterable", "Iterator", "Consumable", "Consumer", "Copyable", "Shareable", "Abandonable", "Default", "Encodable", "Decodable", "Codec", "Authority", "From",
];
pub const DERIVABLE: &[&str] = &["Equatable", "Hashable", "Comparable", "Displayable", "Encodable", "Decodable", "Codec", "Default", "From", "convert"];
pub fn iface_requires(i: &str) -> &'static [&'static str] {
    match i {
        "Hashable" | "Comparable" => &["Equatable"],
        "Codec" => &["Encodable", "Decodable"],
        _ => &[],
    }
}
pub const NOT_ENCODABLE: &[&str] = &[
    "Secret", "File", "FileSystem", "Network", "Clock", "Random", "Environment", "Process", "Supervisor", "Console", "SecretAccess", "RawSql", "Future", "Task", "Shared", "Weak", "Mutex", "MutexGuard", "AtomicInt", "AtomicBool", "StringView",
    "BytesView", "ListView", "Failure", "Outcome",
];
pub fn builtin_members(iface: &str) -> &'static [&'static str] {
    match iface {
        "Equatable" => &["equals"],
        "Hashable" => &["hash"],
        "Comparable" => &["compare"],
        "Displayable" => &["display"],
        "Encodable" => &["encode"],
        "Decodable" => &["decode"],
        "Codec" => &["encode", "decode"],
        "Default" => &["default"],
        "From" => &["from"],
        "Abandonable" => &["abandon"],
        _ => &[],
    }
}

#[derive(Clone, Debug)]
pub struct Sig<'a> {
    pub name: String,
    pub params: Vec<(String, Type, String)>,
    pub ret: Type,
    pub receiver: Option<String>,
    pub modifiers: Vec<String>,
    pub contracts: Vec<String>,
    pub exported: bool,
    pub node: Option<N<'a>>,
    pub body: Option<N<'a>>,
    pub type_params: Vec<String>,
    pub where_: Vec<(String, NamedT)>,
    /// the type parameters this signature itself declares `X: linear` (6.12, 10.7)
    pub linear: Vec<String>,
    /// the `@ignorable` application on the declaration, if any (5.5, X-345)
    pub ignorable: Option<&'a Token>,
}

impl<'a> Sig<'a> {
    pub fn simple(name: &str, params: Vec<(String, Type, String)>, ret: Type, modifiers: Vec<String>) -> Sig<'a> {
        Sig { name: name.into(), params, ret, receiver: None, modifiers, contracts: vec![], exported: true, node: None, body: None, type_params: vec![], where_: vec![], linear: vec![], ignorable: None }
    }
    pub fn has_mod(&self, m: &str) -> bool {
        self.modifiers.iter().any(|x| x == m)
    }
    pub fn show(&self) -> String {
        let types: Vec<String> = self.params.iter().map(|p| p.1.show()).chain(std::iter::once(self.ret.show())).collect();
        self.show_types(&types)
    }
    fn show_types(&self, types: &[String]) -> String {
        let recv = self.receiver.as_ref().map_or(String::new(), |r| format!("{r}, "));
        let ps: Vec<String> = self.params.iter().zip(types).map(|((n, _, m), t)| format!("{n}: {}{}", if m.is_empty() { String::new() } else { format!("{m} ") }, t)).collect();
        format!("{} func {}({}{}) {}", self.modifiers.join(" "), self.name, recv, ps.join(", "), types.last().unwrap()).trim().to_string()
    }
}

fn diagnostic_signatures(sigs: &[&Sig<'_>]) -> Vec<String> {
    let types: Vec<&Type> = sigs.iter().flat_map(|s| s.params.iter().map(|p| &p.1).chain(std::iter::once(&s.ret))).collect();
    let shown = diagnostic_types(&types);
    let mut offset = 0;
    sigs.iter().map(|s| {
        let end = offset + s.params.len() + 1;
        let display = s.show_types(&shown[offset..end]);
        offset = end;
        display
    }).collect()
}

#[derive(Debug, Default)]
pub struct AspectInfo<'a> {
    pub params: Vec<(String, Type, bool)>,
    pub type_param: Option<String>,
    pub needs: Vec<(String, Type, String)>,
    pub needs_contracts: Vec<String>,
    pub before: Vec<N<'a>>,
    pub after: Option<(String, Type, Option<N<'a>>)>,
    pub on_failure: Option<(String, Type, Option<N<'a>>, Token)>,
    pub metadata: bool,
    pub base: Option<(String, Option<N<'a>>, Token)>,
    pub rest: bool,
    pub base_decl: RefCell<Option<Rc<Decl<'a>>>>,
}

/// A `convert` member of a class or enum body (Language Reference 6.13, X-551): the head is the what, the body the
/// how. `convert to T` converts a value of the declaring type into a `T`; `convert from S(s)` converts an `S` into a
/// value of the declaring type. The member is checked as a method (`sig`: a receiver `self` for `to`, one parameter
/// `s` for `from`, the return type the target or the declaring type), and `x as T` resolves to it by the pair
/// (source, target), keyed by the whole type as `implement` coherence keys its pairs (9.5).
#[derive(Clone, Debug)]
pub struct Convert<'a> {
    /// "to" | "from"
    pub direction: String,
    /// the nominal type the head names (the target, or the source)
    pub other: Type,
    /// `try`: the body may fail, and `x as T` is an `Outcome`
    pub fallible: bool,
    /// `owned self` / `owned s`: the conversion consumes, `move x as T`
    pub owned: bool,
    pub exported: bool,
    /// the member as a method: its synthetic name (`convert:to:T`) keys the body's summary
    pub sig: Sig<'a>,
    pub node: N<'a>,
    pub derived: bool,
    pub legacy: bool,
}

#[derive(Debug, Default)]
pub struct Decl<'a> {
    pub kind: String,
    pub name: String,
    pub exported: bool,
    pub module: String,
    pub package: String,
    /// When `module` is `fortis.prelude` and one other standard module declares this name, that module. Both copies
    /// then carry it as origin, so they are one type and a user's class of the same name is not.
    pub core_home: String,
    pub type_params: Vec<String>,
    pub linear: Vec<String>,
    pub fields: OMap<(Type, bool, bool)>,
    pub methods: OMap<Sig<'a>>,
    /// the declared conversions of a class or enum (6.13, X-551)
    pub converts: Vec<Convert<'a>>,
    pub variants: OMap<Vec<(String, Type)>>,
    pub derives: Vec<(String, Vec<Type>)>,
    pub implements: Vec<NamedT>,
    pub mixins: Vec<NamedT>,
    pub requires: Vec<NamedT>,
    pub constants: OMap<Type>,
    pub trivial: bool,
    pub lifetime: Option<String>,
    pub alias: Option<Type>,
    pub where_: Vec<(String, NamedT)>,
    pub code: String,
    pub payload: OMap<Type>,
    pub sig: Option<Sig<'a>>,
    pub aspects: Vec<N<'a>>,
    pub items: BTreeMap<String, BTreeSet<String>>,
    pub obs_items: BTreeMap<String, String>,
    pub obs_fields: OMap<(Type, String)>,
    pub aspect: Option<AspectInfo<'a>>,
    pub node: Option<N<'a>>,
    pub has_cleanup: bool,
}

impl<'a> Decl<'a> {
    /// A type's nominal identity (LANG-04-04): its declaring package@version and module, so two modules' types of
    /// one name are different types; empty for the core library's types and a single-file program's.
    pub fn type_identity(&self) -> String {
        if is_standard(&self.package) || is_standard(&self.module) {
            String::new()
        } else {
            format!("{}/{}", if self.package.is_empty() { "<single>" } else { &self.package }, self.module)
        }
    }

    pub fn new(kind: &str, name: &str, exported: bool, module: &str, package: &str) -> Decl<'a> {
        Decl { kind: kind.into(), name: name.into(), exported, module: module.into(), package: package.into(), ..Default::default() }
    }
    pub fn implements_named(&self, n: &str) -> bool {
        self.implements.iter().any(|i| i.interface_key() == n)
    }
}

pub type DeclRef<'a> = Rc<Decl<'a>>;

#[derive(Default, Clone)]
pub struct Table<'a> {
    pub decls: OMap<DeclRef<'a>>,
    pub implement_blocks: Vec<(NamedT, NamedT, String, N<'a>)>,
    /// implement blocks whose target is not a named type (`implement I for Int?`): rejected in 1.0 (E-TYP-171)
    pub other_implement_targets: Vec<(Type, String, N<'a>)>,
}

impl<'a> Table<'a> {
    pub fn get(&self, n: &str) -> Option<&DeclRef<'a>> {
        self.decls.get(n)
    }
}

/// The libraries of one check and the declaration tables of package modules, built once per (library, module).
pub struct Session<'a> {
    pub libs: &'a Libraries,
    pub loaded: RefCell<HashMap<(usize, String), Rc<Table<'a>>>>,
    /// package modules whose tables are being built, with their heads: an import cycle that leads back to one sees
    /// its declarations' heads (`heads`), so no member's table depends on which member the session entered first
    pub loading: RefCell<HashMap<(usize, String), Rc<Table<'a>>>>,
    /// (type identity, class, field, path) -> the one lambda that position holds (8.4). The identity is the
    /// declaring package and module, so the class name keeps two classes in one module apart. Shared by every
    /// module of the check and both typing passes: the second pass re-notes the same identity.
    pub field_closures: RefCell<BTreeMap<(String, String, String, String), String>>,
    /// an unannotated constant's type, keyed `module\0name`: the initialiser's type, shared with later modules of
    /// the same check and with the typed-IR builder (D-54.4). `Decl.alias` stays the declared type; it is not
    /// interior-mutable through the shared `Rc`.
    pub inferred_consts: RefCell<BTreeMap<String, Type>>,
}

impl<'a> Session<'a> {
    pub fn new(libs: &'a Libraries) -> Session<'a> {
        Session {
            libs,
            loaded: RefCell::new(HashMap::new()),
            loading: RefCell::new(HashMap::new()),
            field_closures: RefCell::new(BTreeMap::new()),
            inferred_consts: RefCell::new(BTreeMap::new()),
        }
    }
}

/// A module's declarations as heads: kind, name, visibility, identity and lifetime parameter, with no types.
///
/// Building a declaration reads only these of the declarations its types name (`named_of`, `loan_carrying_inner`),
/// so a module of an import cycle that is met again while its own table is being built is seen as its heads. Every
/// member's table is then the same whichever member the session enters first, and one module's import order never
/// changes another module's records (D-38, D-35).
pub fn heads<'a>(mtree: N<'a>, module: &str, package: &str) -> Table<'a> {
    let mut t = Table::default();
    for d in declarations(mtree) {
        let Some(name) = &d.name else { continue };
        if ["head", "test", "property", "implement"].contains(&d.kind) {
            continue;
        }
        let mut h = Decl::new(d.kind, name, d.exported, module, package);
        h.node = Some(d.node);
        if let Some(lp) = d.node.find("lifetimeParam") {
            h.lifetime = lp.tokens().get(2).map(|t| t.text.clone());
        }
        // as in a library load, a later declaration of the name replaces an earlier one
        t.decls.insert(name, Rc::new(h));
    }
    t
}

#[derive(Clone)]
pub struct DeclChecker<'a> {
    pub sess: &'a Session<'a>,
    pub lib: usize,
    pub file: String,
    pub module_name: String,
    pub package: String,
    pub records: Vec<Record>,
    pub table: Table<'a>,
    pub prelude_decls: Table<'a>,
    pub type_params: BTreeSet<String>,
    pub tparam_bounds: BTreeMap<String, BTreeSet<String>>,
    pub linear_params: BTreeSet<String>,
    /// the module key of the program's entry module (4.6): a single file is its own entry (`main`); a package set's is
    /// its root package's `entry`, and a set without one has none. Only that module's `main` is the entry `main`, the one
    /// function that creates the capabilities (11.6, LANG-11-07)
    pub entry_module: Option<String>,
    /// heads of this module's own declarations, consulted by `named_of` only while the module is loading and only for
    /// a name the table does not already hold, so a forward reference is stamped with this module's identity
    /// (LANG-04-04) without being inserted early (that would trip E-TYP-070)
    pub pending_heads: HashMap<String, DeclRef<'a>>,
    /// the module whose declarations are being parsed. A core signature may name a core type that is not in the
    /// table yet (`Scope.detach`'s `Supervisor`, written in the prelude and declared in `fortis.host`).
    pub loading_module: String,
    /// bare name -> the one non-prelude standard module that declares it. Empty until the first miss.
    pub core_modules: RefCell<Option<HashMap<String, String>>>,
}

/// One edit of a correction: replace columns [col, end_col) of a line (1-based, characters) with `replacement`.
pub fn edit(file: &str, line: usize, col: usize, end_col: usize, replacement: &str) -> Json {
    Json::obj([
        ("file", Json::str(file)),
        ("line", Json::Int(line as i64)),
        ("column", Json::Int(col as i64)),
        ("end_line", Json::Int(line as i64)),
        ("end_column", Json::Int(end_col as i64)),
        ("replacement", Json::str(replacement)),
    ])
}

pub fn correction(kind: &str, edits: Vec<Json>) -> Json {
    Json::obj([("kind", Json::str(kind)), ("edits", Json::Arr(edits))])
}

/// The column just past a token.
pub fn tok_end(t: &Token) -> usize {
    t.col + t.text.chars().count()
}

fn type_kind(kind: &str) -> bool {
    matches!(kind, "class" | "enum" | "error" | "interface" | "mixin" | "observation" | "type")
}

pub fn tok_of<'a>(d: &Decl<'a>) -> Token {
    let n = d.node.unwrap();
    let ts = n.tokens();
    ts.iter().find(|t| matches!(t.kind, Kind::Upper | Kind::Lower)).copied().unwrap_or(ts[0]).clone()
}

/// Where a declaration's field types are resolved during a fact walk.
enum NextHome<'a, 'b> {
    /// Another module's table, copied out of the session.
    Owned(Table<'a>),
    /// A table the caller already holds (not this checker's own).
    Borrowed(&'b Table<'a>),
    /// This checker's table. The walk passes `None` and the callee reads `self.table`.
    Checker,
}

impl<'a, 'b> NextHome<'a, 'b> {
    /// `None` means this checker's own table.
    fn get(&self) -> Option<&Table<'a>> {
        match self {
            NextHome::Owned(t) => Some(t),
            NextHome::Borrowed(t) => Some(*t),
            NextHome::Checker => None,
        }
    }
}

impl<'a> DeclChecker<'a> {
    pub fn new(sess: &'a Session<'a>, lib: usize, file: &str, module_name: &str, package: &str) -> DeclChecker<'a> {
        DeclChecker {
            sess,
            lib,
            file: file.into(),
            module_name: module_name.into(),
            package: package.into(),
            records: vec![],
            table: Table::default(),
            prelude_decls: Table::default(),
            type_params: BTreeSet::new(),
            tparam_bounds: BTreeMap::new(),
            linear_params: BTreeSet::new(),
            entry_module: Some("main".into()),
            pending_heads: HashMap::new(),
            loading_module: String::new(),
            core_modules: RefCell::new(None),
        }
    }

    pub fn library(&self) -> &'a fortis_names::Library {
        &self.sess.libs.libs[self.lib]
    }

    /// The lexical scope of a declaration's body. An applied mixin keeps this scope while `self` names the
    /// applying class (9.6, X-476). Keep the applying module's completed table available by identity, even when
    /// the mixin's module does not import it; adding its bare names here would shadow the mixin's own names.
    pub fn body_context(&self, d: &Decl<'a>) -> Self {
        if d.module == self.module_name {
            let mut home = self.clone();
            home.records.clear();
            return home;
        }
        self.sess.loaded.borrow_mut().insert((self.lib, self.module_name.clone()), Rc::new(self.table.clone()));
        let lm = self.library().modules.get(&d.module).expect("an imported declaration has a library module");
        let mut home = Self::new(self.sess, lm.library.unwrap_or(self.lib), &fortis_names::resolve::rel(std::path::Path::new(&lm.path)), &d.module, &d.package);
        home.entry_module = self.entry_module.clone();
        home.prelude_decls = self.prelude_decls.clone();
        if lm.library.is_some() {
            home.table = (*self.loaded(lm, &d.module)).clone();
        } else {
            home.load(lm.tree(), true);
            home.records.clear();
        }
        home
    }

    pub fn rec(&mut self, code: &str, msg: String, rule: &str, tok: Option<&Token>, fields: Json, status: &'static str) {
        self.rec_corr(code, msg, rule, tok, fields, status, None);
    }

    /// A record with a machine-applicable correction (Programme 7.1 correction field; DEL-03 criterion 4).
    #[allow(clippy::too_many_arguments)]
    pub fn rec_corr(&mut self, code: &str, msg: String, rule: &str, tok: Option<&Token>, fields: Json, status: &'static str, correction: Option<Json>) {
        let mut f = match fields {
            Json::Obj(m) => m,
            _ => BTreeMap::new(),
        };
        if let Some(cls) = class_of(code) {
            for need in cls.fields {
                f.entry(need.to_string()).or_insert(Json::str(""));
            }
        }
        let (line, col) = tok.map_or((1, 1), |t| (t.line, t.col));
        if code == "E-OBL-001" && crate::journal::active() {
            if let (Some(Json::Str(place)), Some(Json::Str(origin))) = (f.get("unconsumed_path"), f.get("obligation_origin")) {
                // a temporary is journaled under its description ("statement result", "argument o")
                let o = origin.split(':').next().and_then(|x| x.parse().ok()).unwrap_or(line);
                crate::journal::unconsumed(place, o, line);
            }
        }
        let mut r = Record::new(code, msg, rule, &self.file, line, col, Json::Obj(f));
        r.status = status;
        r.correction = correction;
        if !self.records.iter().any(|x| x.code == r.code && x.line == r.line && x.message == r.message) {
            self.records.push(r);
        }
    }

    pub fn typ_fields(expected: &str, actual: &str, span: &str) -> Json {
        Json::obj([("expected_type", Json::str(expected)), ("actual_type", Json::str(actual)), ("primary_span", Json::str(span))])
    }

    // ---- loading --------------------------------------------------------------
    pub fn load(&mut self, mtree: N<'a>, is_library: bool) {
        let lib = self.library();
        if let Some(p) = lib.prelude() {
            if self.module_name != "fortis.prelude" {
                let p: &'a LibraryModule = p;
                self.loading_module = "fortis.prelude".into();
                self.load_module_tree(p.tree(), "fortis.prelude", "fortis", true, None, None);
            self.prelude_decls = self.table.clone();
                self.loading_module.clear();
            }
        }
        for imp in imports(mtree) {
            let mut path = imp.path.join(".");
            let lm: Option<&'a LibraryModule> = lib.modules.get(&path).map(|a| &**a);
            // a package's root module is registered under the package's name too; its declarations belong to the
            // module's own key (`acme.accounts.accounts`), which is where they are built
            if let Some(a) = lib.modules.get(&path) {
                if let Some((k, _)) = lib.modules.iter().find(|(k, v)| std::sync::Arc::ptr_eq(v, a) && k.len() > path.len() && k.starts_with(&format!("{path}."))) {
                    path = k.clone();
                }
            }
            match lm {
                Some(lm) if lm.library.is_some() && lm.package != "fortis" => {
                    let sub = self.loaded(lm, &path);
                    let only: Option<BTreeSet<String>> = imp.members.as_ref().map(|m| m.iter().cloned().collect());
                    let alias = if imp.members.is_none() { Some(imp.alias.clone().unwrap_or_else(|| imp.path.last().unwrap().clone())) } else { None };
                    for (nm, d) in sub.decls.iter() {
                        if d.module != path || only.as_ref().map_or(false, |o| !o.contains(nm)) {
                            continue;
                        }
                        let key = match &alias {
                            Some(a) => format!("{a}.{nm}"),
                            None => nm.clone(),
                        };
                        self.table.decls.insert(&key, d.clone());
                    }
                    for blk in &sub.implement_blocks {
                        if !self.table.implement_blocks.iter().any(|b| b.0 == blk.0 && b.1 == blk.1 && b.2 == blk.2 && b.3.same(blk.3)) {
                            self.table.implement_blocks.push(blk.clone());
                        }
                    }
                }
                Some(lm) => {
                    // a standard-library module is loaded into this table, not as its own checker, so its heads are
                    // not pending. A method parsed before its class is inserted would otherwise name that class with
                    // no origin, while a later signature that mentions it would keep the module.
                    let pkg = lm.package.clone();
                    let lib_heads = heads(lm.tree(), &path, &pkg);
                    let saved = std::mem::take(&mut self.pending_heads);
                    self.pending_heads = lib_heads.decls.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                    for (k, v) in &saved {
                        self.pending_heads.insert(k.clone(), v.clone());
                    }
                    let only: Option<BTreeSet<String>> = imp.members.as_ref().map(|m| m.iter().cloned().collect());
                    let alias = if imp.members.is_none() { Some(imp.alias.clone().unwrap_or_else(|| imp.path.last().unwrap().clone())) } else { None };
                    let saved_loading = std::mem::replace(&mut self.loading_module, path.clone());
                    self.load_module_tree(lm.tree(), &path, &pkg, true, only, alias);
                    self.pending_heads = saved;
                    self.loading_module = saved_loading;
                }
                None => {
                    for m in imp.members.clone().unwrap_or_default() {
                        self.table.decls.set_default(&m, Rc::new(Decl::new("unknown", &m, true, &path, &path)));
                    }
                }
            }
        }
        let (mn, pk) = (self.module_name.clone(), self.package.clone());
        self.pending_heads = heads(mtree, &mn, &pk).decls.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        self.loading_module = mn.clone();
        self.load_module_tree(mtree, &mn, &pk, is_library, None, None);
        self.pending_heads.clear();
        self.loading_module.clear();
    }

    fn loaded(&self, lm: &'a LibraryModule, path: &str) -> Rc<Table<'a>> {
        let key = (lm.library.unwrap(), path.to_string());
        if let Some(t) = self.sess.loaded.borrow().get(&key) {
            return t.clone();
        }
        if let Some(h) = self.sess.loading.borrow().get(&key) {
            return h.clone();
        }
        self.sess.loading.borrow_mut().insert(key.clone(), Rc::new(heads(lm.tree(), path, &lm.package)));
        let mut sub = DeclChecker::new(self.sess, lm.library.unwrap(), &lm.path, path, &lm.package);
        sub.load(lm.tree(), true);
        let t = Rc::new(std::mem::take(&mut sub.table));
        self.sess.loading.borrow_mut().remove(&key);
        self.sess.loaded.borrow_mut().insert(key, t.clone());
        t
    }

    pub fn load_module_tree(&mut self, mtree: N<'a>, module: &str, package: &str, library: bool, only: Option<BTreeSet<String>>, alias: Option<String>) {
        let all = declarations(mtree);
        let only = only.map(|only| {
            let by_name: HashMap<String, &DeclInfo<'a>> = all.iter().filter_map(|d| d.name.clone().map(|n| (n, d))).collect();
            let mut want = only.clone();
            let mut frontier: Vec<String> = want.iter().cloned().collect();
            while let Some(f) = frontier.pop() {
                if let Some(d0) = by_name.get(&f) {
                    for t in d0.node.tokens() {
                        if t.kind == Kind::Upper && by_name.contains_key(&t.text) && !want.contains(&t.text) {
                            want.insert(t.text.clone());
                            frontier.push(t.text.clone());
                        }
                    }
                }
            }
            want
        });
        let mut local_aspects: BTreeMap<String, Rc<Decl<'a>>> = BTreeMap::new();
        for d in &all {
            if d.kind == "aspect" {
                if let Some(n) = &d.name {
                    local_aspects.insert(n.clone(), Rc::new(self.load_decl(d, module, package)));
                }
            }
        }
        for a in local_aspects.values() {
            if let Some(info) = &a.aspect {
                if let Some((bname, _, _)) = &info.base {
                    *info.base_decl.borrow_mut() = local_aspects.get(bname).cloned();
                }
            }
        }
        // `@ignorable` is the core aspect only where the name resolves to the prelude's: a module that declares or
        // imports an aspect `ignorable` applies its own, which exempts no result (5.5, 11.8; D-314)
        let core_ignorable = module == "fortis.prelude"
            || (!local_aspects.contains_key("ignorable")
                && !imports(mtree).iter().any(|i| i.path.join(".") != "fortis.prelude" && i.members.as_ref().map_or(false, |m| m.iter().any(|x| x == "ignorable"))));
        for d in &all {
            if ["head", "test", "property", "implement"].contains(&d.kind) {
                if d.kind == "implement" {
                    // a package module loaded as a library keeps its blocks too, so its importers see them (D-132)
                    self.load_implement(d.node, module);
                }
                continue;
            }
            let name = match &d.name {
                Some(n) => n.clone(),
                None => continue,
            };
            if let Some(o) = &only {
                if !o.contains(&name) {
                    continue;
                }
            }
            let key = match &alias {
                Some(a) => format!("{a}.{name}"),
                None => name.clone(),
            };
            if !library {
                if let Some(existing) = self.table.decls.get(&key) {
                    if existing.module == module {
                        self.rec(
                            "E-TYP-070",
                            format!("`{key}` is declared twice in this module"),
                            "LANG-09-01",
                            d.token,
                            Self::typ_fields("one declaration", "two", &key),
                            "violated",
                        );
                        continue;
                    }
                }
            }
            let decl = if d.kind == "aspect" {
                local_aspects[&name].clone()
            } else {
                let mut decl = self.load_decl(d, module, package);
                if !core_ignorable {
                    if let Some(sig) = decl.sig.as_mut() {
                        sig.ignorable = None;
                    }
                    for m in decl.methods.keys().cloned().collect::<Vec<_>>() {
                        if let Some(sig) = decl.methods.get_mut(&m) {
                            sig.ignorable = None;
                        }
                    }
                }
                Rc::new(decl)
            };
            self.table.decls.insert(&key, decl);
        }
    }

    pub fn load_decl(&mut self, d: &DeclInfo<'a>, module: &str, package: &str) -> Decl<'a> {
        let node = d.node;
        let kind = d.kind;
        let nm = d.name.clone().unwrap_or_default();
        let mut decl = Decl::new(kind, &nm, d.exported, module, package);
        if module == "fortis.prelude" {
            decl.core_home = self.core_module(&nm);
        }
        decl.node = Some(node);
        // the declaration's own type parameters are in its head (a direct child); a generic method's are its own
        if let Some(tp) = node.child_node("typeParams") {
            let first_is_head = node.kid(0).map_or(false, |c| c.is("funcHead"));
            if !first_is_head {
                decl.type_params = tp.tokens().iter().filter(|t| t.kind == Kind::Upper).map(|t| t.text.clone()).collect();
            }
        }
        self.type_params = decl.type_params.iter().cloned().collect();
        if let Some(wc) = node.child_node("whereClause").or_else(|| node.child_node("linearWhereClause")) {
            decl.where_ = self.where_of(wc);
            decl.linear = self.linear_of(wc);
            decl.linear.extend(wc.find_all("linearConstraint").iter().map(|c| c.tokens()[0].text.clone()));
        }
        if let Some(lp) = node.find("lifetimeParam") {
            decl.lifetime = lp.tokens().get(2).map(|t| t.text.clone());
        }
        if kind == "func" {
            decl.aspects = d.aspects.clone();
        }
        if kind == "contract" {
            let mut items: BTreeMap<String, BTreeSet<String>> = ["modifiers", "forbids", "requires", "excludes"].iter().map(|k| (k.to_string(), BTreeSet::new())).collect();
            for it in node.child_nodes("contractItem") {
                let ws: Vec<String> = it.tokens().iter().map(|t| t.text.clone()).collect();
                match ws[0].as_str() {
                    "forbids" => {
                        items.get_mut("forbids").unwrap().insert(ws[1].clone());
                    }
                    "requires" => {
                        items.get_mut("requires").unwrap().insert(ws.last().unwrap().clone());
                    }
                    "excludes" => {
                        items.get_mut("excludes").unwrap().insert(ws.last().unwrap().clone());
                    }
                    _ => {
                        items.get_mut("modifiers").unwrap().insert(ws[0].clone());
                    }
                }
            }
            decl.items = items;
        }
        if kind == "observation" {
            for it in node.child_nodes("observationItem") {
                let ws: Vec<String> = it.direct_tokens().iter().map(|t| t.text.clone()).collect();
                if ws[0] == "version" || ws[0] == "encoding" {
                    decl.obs_items.insert(ws[0].clone(), ws.get(2).cloned().unwrap_or_default());
                    continue;
                }
                let ft = self.type_of(it.find("type"));
                decl.obs_fields.insert(&ws[0], (ft, ws.last().unwrap().clone()));
            }
        }
        if kind == "aspect" {
            let mut a = AspectInfo { type_param: decl.type_params.first().cloned(), ..Default::default() };
            if let Some(ap) = node.find("aspectParams") {
                for p in ap.child_nodes("param") {
                    if p.find("restParam").is_some() {
                        a.rest = true;
                        continue;
                    }
                    let ts = p.direct_tokens();
                    let nm2 = ts.iter().find(|t| t.kind == Kind::Lower).unwrap();
                    let pt = p.find("paramType");
                    let t = match pt {
                        Some(pt) => self.type_of(pt.find("type")),
                        None => unknown("aspect param"),
                    };
                    a.params.push((nm2.text.clone(), t, p.find("constExpr").is_some()));
                }
            }
            for part in node.child_nodes("aspectPart") {
                let ws: Vec<String> = part.direct_tokens().iter().map(|t| t.text.clone()).collect();
                let blk = part.find("block");
                if ws[0] == "needs" && ws.len() > 2 {
                    let pt = part.find("paramType").unwrap();
                    let t = self.type_of(pt.find("type"));
                    let mode = pt.direct_tokens().iter().find(|t| t.text == "editable" || t.text == "owned").map_or(String::new(), |t| t.text.clone());
                    a.needs.push((ws[1].clone(), t, mode));
                } else if ws[0] == "needs" {
                    a.needs_contracts.push(ws[1].clone());
                } else if ws[0] == "before" {
                    if let Some(b) = blk {
                        a.before.push(b);
                    }
                } else if ws[0] == "after" {
                    let t = self.type_of(part.find("type"));
                    a.after = Some((ws[2].clone(), t, blk));
                } else if ws[0] == "on" {
                    let t = self.type_of(part.find("type"));
                    a.on_failure = Some((ws[3].clone(), t, blk, part.direct_tokens()[3].clone()));
                } else if ws[0] == "metadata" {
                    a.metadata = true;
                }
            }
            let kids = node.kids();
            if let Some(eq) = kids.iter().position(|c| c.text_is("=")) {
                if let Some(base) = kids[eq + 1..].iter().filter_map(|c| c.tok()).find(|t| t.kind == Kind::Lower) {
                    a.base = Some((base.text.clone(), node.find("argList"), base.clone()));
                }
            }
            decl.aspect = Some(a);
        }
        if kind == "class" || kind == "enum" {
            decl.trivial = node.kids().iter().any(|c| c.text_is("trivial"));
            let kids = node.kids();
            for (i, c) in kids.iter().enumerate() {
                if c.text_is("derive") {
                    if let Some(C::N(ul)) = kids.get(i + 1) {
                        let items = self.upper_list(*ul);
                        decl.derives.extend(items);
                    }
                }
                if c.text_is("implements") || c.text_is("with") {
                    if let Some(C::N(tl)) = kids.get(i + 1) {
                        for t in tl.child_nodes("type") {
                            if let Type::Named(n) = self.type_of(Some(t)) {
                                if c.text_is("implements") {
                                    decl.implements.push(n);
                                } else {
                                    decl.mixins.push(n);
                                }
                            }
                        }
                    }
                }
            }
            for m in node.kids().into_iter().filter_map(|c| c.node()) {
                if m.name() != "classMember" && m.name() != "enumMember" {
                    continue;
                }
                let exp = m.kids().iter().any(|c| c.text_is("export"));
                if let Some(v) = m.find("variant") {
                    if m.name() == "enumMember" {
                        let vt = v.direct_tokens();
                        let fl = v.find("fieldList");
                        let fields = match fl {
                            Some(fl) => self.field_list(fl),
                            None => vec![],
                        };
                        decl.variants.insert(&vt[0].text, fields);
                        continue;
                    }
                }
                if let Some(fd) = m.find("fieldDecl") {
                    let ft = fd.direct_tokens();
                    let fname = ft[0].text.clone();
                    if decl.fields.contains(&fname) || decl.methods.contains(&fname) || decl.constants.contains(&fname) {
                        self.rec("E-TYP-070", format!("member `{fname}` of {nm} is declared twice"), "LANG-09-01", Some(ft[0]), Self::typ_fields("one member", "two", &fname), "violated");
                    }
                    let t = self.type_of(fd.find("type"));
                    let has_default = fd.kids().iter().any(|c| c.is("constExpr"));
                    decl.fields.insert(&fname, (t, exp, has_default));
                    continue;
                }
                if let Some(cd) = m.find("constDecl") {
                    if m.find("methodHead").is_none() {
                        let ct = cd.direct_tokens();
                        let cname = &ct[1].text;
                        if decl.constants.contains(cname) || decl.fields.contains(cname) || decl.methods.contains(cname) {
                            self.rec("E-TYP-070", format!("member `{cname}` of {nm} is declared twice"), "LANG-09-01", Some(ct[1]), Self::typ_fields("one member", "two", cname), "violated");
                        }
                        let t = match cd.find("type") {
                            Some(t) => self.type_of(Some(t)),
                            None => unknown("untyped const"),
                        };
                        decl.constants.insert(&ct[1].text, t);
                        continue;
                    }
                }
                if m.find("cleanupDecl").is_some() {
                    decl.has_cleanup = true;
                    continue;
                }
                if let Some(cd) = m.find("convertDecl") {
                    let cv = self.convert_of(cd, exp, &decl);
                    decl.converts.push(cv);
                    continue;
                }
                if let Some(head) = m.find("methodHead") {
                    let mut sig = self.signature(head, exp, m.find("funcBody"));
                    sig.ignorable = ignorable_use(&m.child_nodes("aspectUse"));
                    if decl.methods.contains(&sig.name) || decl.fields.contains(&sig.name) || decl.constants.contains(&sig.name) {
                        let t0 = head.tokens()[0];
                        self.rec("E-TYP-070", format!("member `{}` of {nm} is declared twice", sig.name), "LANG-09-01", Some(t0), Self::typ_fields("one member", "two", &sig.name), "violated");
                    }
                    let n = sig.name.clone();
                    decl.methods.insert(&n, sig);
                }
            }
        } else if kind == "interface" {
            let kids = node.kids();
            for (i, c) in kids.iter().enumerate() {
                if c.text_is("requires") {
                    if let Some(C::N(tl)) = kids.get(i + 1) {
                        for t in tl.child_nodes("type") {
                            if let Type::Named(n) = self.type_of(Some(t)) {
                                decl.requires.push(n);
                            }
                        }
                    }
                }
            }
            if let Some(wc) = node.find("whereClause") {
                decl.where_ = self.where_of(wc);
            }
            for m in node.child_nodes("interfaceMember") {
                if let Some(head) = m.find("methodHead") {
                    let sig = self.signature(head, true, None);
                    let n = sig.name.clone();
                    decl.methods.insert(&n, sig);
                } else {
                    let ts = m.direct_tokens();
                    let t = self.type_of(m.find("type"));
                    decl.constants.insert(&ts[1].text, t);
                }
            }
        } else if kind == "mixin" {
            for m in node.child_nodes("mixinMember") {
                let ts = m.direct_tokens();
                let head = m.find("methodHead");
                if ts.first().map_or(false, |t| t.text == "requires") {
                    if let Some(head) = head {
                        let sig = self.signature(head, true, None);
                        decl.requires.push(nt(&format!("__method__:{}", sig.name)));
                        decl.constants.insert(&sig.name, unknown("required method"));
                        let n = format!("requires:{}", sig.name);
                        decl.methods.insert(&n, sig);
                    }
                } else if let Some(head) = head {
                    let exp = m.kids().iter().any(|c| c.text_is("export"));
                    let sig = self.signature(head, exp, m.find("funcBody"));
                    let n = sig.name.clone();
                    decl.methods.insert(&n, sig);
                }
            }
        } else if kind == "type" {
            let alias = self.type_of(node.find("type"));
            if let Type::Refined { base, .. } = &alias {
                if base.package.is_empty() && FLOAT_TYPES.contains(&base.name.as_str()) {
                    self.rec(
                        "E-TYP-020",
                        format!("float refinement in `type {nm}`"),
                        "LANG-06-05",
                        d.token,
                        Self::typ_fields("an integer, text, byte or collection base", &alias.show(), &nm),
                        "violated",
                    );
                }
            }
            decl.alias = Some(alias);
        } else if kind == "error" {
            let ts = node.direct_tokens();
            let code = ts.iter().find(|t| t.kind == Kind::Str).map(|t| t.text.clone()).unwrap_or_default();
            let cs: Vec<char> = code.chars().collect();
            decl.code = if cs.len() >= 2 { cs[1..cs.len() - 1].iter().collect() } else { String::new() };
            if let Some(fl) = node.find("fieldList") {
                for (k, v) in self.field_list(fl) {
                    decl.payload.insert(&k, v);
                }
            }
        } else if kind == "func" {
            let mut sig = self.signature(node.find("funcHead").unwrap(), d.exported, node.find("funcBody"));
            sig.ignorable = ignorable_use(&d.aspects);
            if nm == "main" && self.entry_module.as_deref() == Some(module) {
                // the entry `main` owns the capabilities the runtime supplies to its parameters (4.6, X-345, Q7)
                for p in sig.params.iter_mut() {
                    if p.2.is_empty() && matches!(&p.1, Type::Named(n) if n.package.is_empty() && fortis_names::reading::CAPABILITY_TYPES.contains(&n.name.as_str())) {
                        p.2 = "owned".into();
                    }
                }
            }
            decl.sig = Some(sig);
        } else if kind == "const" {
            decl.alias = Some(match node.child_node("type") {
                Some(t) => self.type_of(Some(t)),
                None => unknown("inferred const"),
            });
        }
        self.type_params = BTreeSet::new();
        decl
    }

    fn load_implement(&mut self, node: N<'a>, module: &str) {
        let ts = node.child_nodes("type");
        if ts.len() < 2 {
            return;
        }
        let iface = self.type_of(Some(ts[0]));
        let target = self.type_of(Some(ts[1]));
        match (iface, target) {
            (Type::Named(i), Type::Named(t)) => self.table.implement_blocks.push((i, t, module.into(), node)),
            (Type::Named(_), t) if !t.is_unknown() => self.table.other_implement_targets.push((t, module.into(), node)),
            _ => {}
        }
    }

    fn upper_list(&mut self, ul: N<'a>) -> Vec<(String, Vec<Type>)> {
        let mut out = Vec::new();
        let kids = ul.kids();
        let mut i = 0;
        while i < kids.len() {
            if let C::T(k) = kids[i] {
                if k.text == "convert" {
                    if let Some(C::N(tr)) = kids.get(i + 2) {
                        out.push(("convert".into(), vec![self.type_of_ref(*tr)]));
                    }
                    i += 2;
                } else if k.kind == Kind::Upper {
                    let mut args = vec![];
                    if let Some(C::N(ta)) = kids.get(i + 1) {
                        if ta.name() == "typeArgs" {
                            args = ta.child_nodes("typeArg").into_iter().map(|a| self.type_arg(a)).collect();
                            i += 1;
                        }
                    }
                    let owner = self.pending_heads.get(&k.text).cloned().or_else(|| self.table.get(&k.text).cloned());
                    let spelling = if let Some(owner) = owner.filter(|_| DERIVABLE.contains(&k.text.as_str())) { if is_standard(&owner.module) { k.text.clone() } else { format!("{} (not prelude)", k.text) } } else { k.text.clone() };
                    out.push((spelling, args));
                }
            }
            i += 1;
        }
        out
    }

    pub fn field_list(&mut self, fl: N<'a>) -> Vec<(String, Type)> {
        let kids = fl.kids();
        let mut out = Vec::new();
        for (i, k) in kids.iter().enumerate() {
            if let C::T(t) = k {
                if t.kind == Kind::Lower && i + 2 < kids.len() && kids[i + 2].is("type") {
                    let ty = self.type_of(kids[i + 2].node());
                    out.push((t.text.clone(), ty));
                }
            }
        }
        out
    }

    pub fn where_of(&mut self, wc: N<'a>) -> Vec<(String, NamedT)> {
        let mut out = Vec::new();
        for c in wc.find_all("constraint") {
            let ts = c.direct_tokens();
            if let Some(t) = c.find("type") {
                if let Type::Named(n) = self.type_of(Some(t)) {
                    out.push((ts[0].text.clone(), n));
                }
            }
        }
        out
    }

    /// The type parameters a `where` clause declares `X: linear` (6.12, 10.7): a constraint with no interface type.
    pub fn linear_of(&self, wc: N<'a>) -> Vec<String> {
        let mut out = Vec::new();
        for c in wc.find_all("constraint") {
            if c.find("type").is_none() {
                out.push(c.direct_tokens()[0].text.clone());
            }
        }
        out
    }

    // ---- signatures and types -------------------------------------------------
    /// A `convert` member as the method signature its body is checked with (6.13, X-551): `convert to T` has the
    /// receiver `self` and returns `T`; `convert from S(s)` has the one parameter `s: S` and returns the declaring
    /// type. The synthetic name (`convert:to:T`) is one no spelled member can hold, since `:` is no identifier.
    pub fn convert_of(&mut self, node: N<'a>, exported: bool, decl: &Decl<'a>) -> Convert<'a> {
        let kids = node.kids();
        let tr_i = kids.iter().position(|c| c.is("typeRef")).unwrap_or(1);
        let direction = kids.get(tr_i.wrapping_sub(1)).and_then(|c| c.tok()).map_or("to".to_string(), |t| t.text.clone());
        let fallible = kids[..tr_i].iter().filter_map(|c| c.tok()).any(|t| t.text == "try");
        let owned = kids[tr_i + 1..].iter().filter_map(|c| c.tok()).any(|t| t.text == "owned");
        let other = self.type_of_ref(kids[tr_i].node().unwrap());
        let pname = if direction == "from" {
            kids[tr_i + 1..].iter().filter_map(|c| c.tok()).find(|t| t.kind == Kind::Lower).map_or("s".to_string(), |t| t.text.clone())
        } else {
            "self".to_string()
        };
        let body = node.find("funcBody");
        let modifiers = if fallible { vec!["try".to_string()] } else { vec![] };
        let qual = match &other {
            Type::Named(n) => {
                if n.package.is_empty() {
                    n.origin.clone()
                } else {
                    n.package.clone()
                }
            }
            _ => String::new(),
        };
        let suffix = if qual.is_empty() { String::new() } else { format!("@{qual}") };
        let position = if matches!(&other, Type::Named(n) if !n.args.is_empty()) { let at = node.tokens()[0]; format!(":{}:{}", at.line, at.col) } else { String::new() };
        let synth = format!("convert:{direction}:{}{suffix}{position}", other.show());
        let sig = if direction == "to" {
            Sig { name: synth, params: vec![], ret: other.clone(), receiver: Some(if owned { "owned self".into() } else { "self".into() }), modifiers, contracts: vec![], exported, node: Some(node), body, type_params: vec![], where_: vec![], linear: vec![], ignorable: None }
        } else {
            Sig { name: synth, params: vec![(pname, other.clone(), if owned { "owned".into() } else { String::new() })], ret: named_of_decl(decl, vec![]), receiver: None, modifiers, contracts: vec![], exported, node: Some(node), body, type_params: vec![], where_: vec![], linear: vec![], ignorable: None }
        };
        Convert { direction, other, fallible, owned, exported, sig, node, derived: false, legacy: false }
    }

    pub fn signature(&mut self, head: N<'a>, exported: bool, body: Option<N<'a>>) -> Sig<'a> {
        let mods = head.find("modifiers");
        let words: Vec<String> = mods.map(|m| m.tokens().iter().map(|t| t.text.clone()).collect()).unwrap_or_default();
        let contracts: Vec<String> = words.iter().filter(|w| w.chars().next().map_or(false, |c| c.is_uppercase())).cloned().collect();
        let modifiers: Vec<String> = words.iter().filter(|w| !w.chars().next().map_or(false, |c| c.is_uppercase())).cloned().collect();
        let fname = match head.find("memberName") {
            Some(mn) => mn.tokens()[0].text.clone(),
            None => {
                let dt = head.direct_tokens();
                dt[dt.iter().position(|t| t.text == "func").unwrap() + 1].text.clone()
            }
        };
        let sig = head.find("funcSignature");
        let tparams: Vec<String> = sig.and_then(|s| s.find("typeParams")).map(|tp| tp.tokens().iter().filter(|t| t.kind == Kind::Upper).map(|t| t.text.clone()).collect()).unwrap_or_default();
        let saved = self.type_params.clone();
        self.type_params.extend(tparams.iter().cloned());
        let mut params = Vec::new();
        let mut receiver = None;
        if let Some(pl) = sig.and_then(|s| s.find("paramList")) {
            for p in pl.child_nodes("param") {
                if let Some(sp) = p.find("selfParam") {
                    receiver = Some(sp.tokens().iter().map(|t| t.text.clone()).collect::<Vec<_>>().join(" "));
                    continue;
                }
                let ts = p.direct_tokens();
                let pt = match p.find("paramType") {
                    Some(pt) => pt,
                    None => {
                        self.rec(
                            "E-TYP-150",
                            "`...rest` forwards an aspect's remaining arguments and is only permitted in an aspect's parameter list (11.8)".into(),
                            "LANG-11-09",
                            Some(p.tokens()[0]),
                            Self::typ_fields("a typed parameter", "...rest", &fname),
                            "violated",
                        );
                        continue;
                    }
                };
                let mode = pt.direct_tokens().iter().find(|t| t.text == "editable" || t.text == "owned").map_or(String::new(), |t| t.text.clone());
                let variadic = ts.first().map_or(false, |t| t.text == "...");
                let nm = ts.iter().find(|t| t.kind == Kind::Lower).unwrap();
                let t = self.type_of(pt.find("type"));
                params.push((nm.text.clone(), t, if variadic { format!("...{mode}") } else { mode }));
            }
        }
        // a head without a return type returns Void (8.1, X-345)
        let ret = match sig.map(|s| s.find("returnType")) {
            Some(None) => void(),
            rt => self.type_of(rt.flatten().and_then(|r| r.find("type"))),
        };
        self.type_params = saved;
        let where_ = match sig.and_then(|s| s.find("whereClause")) {
            Some(wc) => self.where_of(wc),
            None => vec![],
        };
        let linear = match sig.and_then(|s| s.find("whereClause")) {
            Some(wc) => self.linear_of(wc),
            None => vec![],
        };
        Sig { name: fname, params, ret, receiver, modifiers, contracts, exported, node: Some(head), body, type_params: tparams, where_, linear, ignorable: None }
    }

    pub fn type_arg(&mut self, ta: N<'a>) -> Type {
        if let Some(t) = ta.find("type") {
            return self.type_of(Some(t));
        }
        named(&format!("#{}", ta.direct_tokens()[0].text))
    }

    /// Convert a `type` node to a Type.
    pub fn type_of(&mut self, tnode: Option<N<'a>>) -> Type {
        let tnode = match tnode {
            Some(t) => t,
            None => return unknown("missing type"),
        };
        let kids = tnode.kids();
        self.type_of_parts(&kids)
    }

    /// A `type` node's children (also used for the synthetic `type` of a function result).
    pub fn type_of_parts(&mut self, kids: &[C<'a>]) -> Type {
        let first = kids[0];
        match first.name() {
            Some("fnType") => self.fn_type(first.node().unwrap()),
            Some("loanType") => {
                let f = first.node().unwrap();
                let words: Vec<String> = f.kids().iter().filter_map(|k| k.tok()).map(|t| t.text.clone()).collect();
                if words.iter().any(|w| w == "dynamic") {
                    let tr = f.find("typeRef").unwrap();
                    let inner = match self.named_of(tr) {
                        Type::Named(n) => Type::Dynamic(n),
                        Type::Refined { base, .. } => Type::Dynamic(base),
                        other => other,
                    };
                    return if words.iter().any(|w| w == "?") { optional(inner) } else { inner };
                }
                let mode = if words.iter().any(|w| w == "editable") { "editable" } else { "alias" };
                let mut inner = self.named_type(f.find("namedType").unwrap());
                if words.iter().any(|w| w == "?") {
                    inner = optional(inner);
                }
                let org = f.find("origin").map(|o| o.tokens()[0].text.clone()).unwrap_or_default();
                Type::Loan { inner: Box::new(inner), mode: mode.into(), origin: org }
            }
            Some("namedType") => {
                let t = self.named_type(first.node().unwrap());
                if kids.len() > 1 && kids[1].text_is("?") {
                    optional(t)
                } else {
                    t
                }
            }
            _ => unknown("type form"),
        }
    }

    pub fn named_type(&mut self, ntn: N<'a>) -> Type {
        let kids = ntn.kids();
        if let C::T(first) = kids[0] {
            if first.text == "Self" {
                return named("Self");
            }
            if first.text == "(" {
                let ts: Vec<Type> = kids.iter().filter(|k| k.is("type")).map(|k| self.type_of(k.node())).collect();
                return if ts.len() == 1 { ts.into_iter().next().unwrap() } else { Type::Tuple(ts) };
            }
            if first.text == "{" {
                let fl = ntn.find("fieldList").unwrap();
                return Type::Record(self.field_list(fl));
            }
        }
        self.named_of(kids[0].node().unwrap())
    }

    pub fn type_of_ref(&mut self, tr: N<'a>) -> Type {
        self.named_of(tr)
    }

    pub fn named_of(&mut self, tr: N<'a>) -> Type {
        let ts = tr.direct_tokens();
        let (nm, base_tok) = if ts[0].kind == Kind::Lower { (format!("{}.{}", ts[0].text, ts[2].text), ts[2]) } else { (ts[0].text.clone(), ts[0]) };
        let args: Vec<Type> = match tr.child_node("typeArgs") {
            Some(ta) => ta.child_nodes("typeArg").into_iter().map(|a| self.type_arg(a)).collect(),
            None => vec![],
        };
        if self.type_params.contains(&nm) {
            if tr.child_node("refinement").is_some() {
                let shown: String = tr.tokens().iter().map(|t| t.text.as_str()).collect();
                self.rec("E-TYP-020", format!("`{shown}` is not a refinement of {nm}: a type parameter has no refinement"), "LANG-06-02", Some(base_tok), Self::typ_fields("a refinement 6.3 lists", &shown, &shown), "violated");
            }
            return Type::TypeParam(nm);
        }
        // a type is named by its declaration, however the annotation spells it (`one.Node` is `Node` of its module).
        // A name this module declares later is not in the table yet: its head supplies the identity. While the
        // module is being parsed, that head wins over a prelude binding of the same name already in the table:
        // `fortis.text` redeclares `Builder`, and `create() Builder` must name text's Builder, not the prelude one.
        // An import already bound wins over a head from a different module.
        let table_d = self.table.get(&nm).filter(|d| d.kind != "unknown").cloned();
        let head = self.pending_heads.get(&nm).filter(|d| d.kind != "unknown").cloned();
        let own = head.as_ref().filter(|h| h.module == self.loading_module && table_d.as_ref().map_or(true, |d| d.module != self.loading_module)).cloned();
        // a program's own type (a newtype, a class) has no refinement (6.3, X-510)
        let declared = own.as_ref().or(table_d.as_ref()).or(head.as_ref()).map_or(false, |d| d.kind != "unknown" && !d.module.starts_with("fortis"));
        let (canonical, pkg, origin) = if let Some(d) = own.or(table_d).or(head) {
            (d.name.clone(), d.type_identity(), self.origin_of(&d))
        } else {
            // the prelude names Supervisor before fortis.host is loaded. Only a core module being parsed may
            // take that module from the standard library; a user annotation that has not bound the name stays
            // unresolved.
            let origin = if nm.contains('.') || !self.loading_module.starts_with("fortis") { String::new() } else { self.core_module(&nm) };
            (nm.clone(), String::new(), origin)
        };
        let n = NamedT { name: canonical, args, package: pkg, origin };
        if let Some(r) = tr.child_node("refinement") {
            if FLOAT_TYPES.contains(&nm.as_str()) {
                self.rec("E-TYP-020", format!("float refinement `{nm}<…>`"), "LANG-06-05", Some(base_tok), Self::typ_fields("an integer, text, byte or collection base", &nm, &nm), "violated");
                return Type::Named(n);
            }
            let rb = r.find("refineBody").unwrap();
            let mut kind = "range".to_string();
            if let Some(C::T(t0)) = rb.kid(0) {
                if t0.kind == Kind::Lower {
                    kind = t0.text.clone();
                }
            }
            let (lo, hi, bound_why) = match rb.find("range") {
                Some(rg) => self.range_of(rg, &nm),
                None => (None, None, None),
            };
            if let Some(why) = Self::refinement_why(&nm, &kind, lo, hi, bound_why, declared) {
                // not a refinement of this base (6.3, X-510): the type is read as its base, so one record names the form
                let shown: String = tr.tokens().iter().map(|t| if t.text == ":" || t.text == "," { format!("{} ", t.text) } else { t.text.clone() }).collect();
                self.rec("E-TYP-020", format!("`{shown}` is not a refinement of {nm}: {why}"), "LANG-06-02", Some(base_tok), Self::typ_fields("a refinement 6.3 lists", &shown, &shown), "violated");
                return Type::Named(n);
            }
            return Type::Refined { base: n, lo, hi, kind };
        }
        Type::Named(n)
    }

    /// Why `nm<kind: lo..hi>` is not a refinement 6.3 lists, or None when it is (X-510): each measure refines the bases
    /// 6.3 names for it, a bound is an integer literal, the range is not empty and lies within the base's own range (a
    /// count is never negative).
    pub fn refinement_why(nm: &str, kind: &str, lo: Option<Int>, hi: Option<Int>, bound_why: Option<String>, declared: bool) -> Option<String> {
        const SCALARS: &[&str] = &["String", "StringView"];
        const LENGTH: &[&str] = &["Bytes", "BytesView", "List", "ListView"];
        let in_base = match kind {
            "range" => is_int_type(nm),
            "scalars" => SCALARS.contains(&nm),
            "length" => LENGTH.contains(&nm),
            _ => return Some(format!("`{kind}` is not a measure; a refinement is a range of an integer type, `scalars` of text or `length` of bytes or a list")),
        };
        if !in_base || declared {
            if kind == "range" {
                return Some(if SCALARS.contains(&nm) || LENGTH.contains(&nm) { "only an integer type has a range refinement".to_string() } else { format!("{nm} is not a built-in integer, text, byte or list type") });
            }
            return Some(format!("`{kind}` measures {}, not {nm}", if kind == "scalars" { "text" } else { "bytes or a list" }));
        }
        if let Some(w) = bound_why {
            return Some(w);
        }
        let full = if kind == "range" { int_types(nm).unwrap() } else { (0, 9_223_372_036_854_775_807) };
        if let (Some(l), Some(h)) = (lo, hi) {
            if l > h {
                return Some("the range is empty".into());
            }
        }
        for b in [lo, hi].into_iter().flatten() {
            if b < full.0 || b > full.1 {
                return Some(if kind == "range" { format!("{b} is outside {nm}'s own range [{}, {}]", full.0, full.1) } else { format!("a {kind} count is not {b}") });
            }
        }
        None
    }

    /// (lo, hi, why): the folded bounds of a refinement of `base` or of a range pattern over `base`, and why a bound
    /// could not be folded (6.3, 9.8, X-510, X-512, X-517): it is not an integer literal or a `const` of the base type
    /// resolved at compile time. `hi` is stored inclusive, so `lo..<hi` folds to hi - 1; a leading `..` or `..<` (a
    /// range pattern's one-sided exclusive form, 9.8) leaves `lo` open.
    pub fn range_of(&mut self, rg: N<'a>, base: &str) -> (Option<Int>, Option<Int>, Option<String>) {
        let kids = rg.kids();
        let (mut lo, mut hi) = (None, None);
        let mut why: Option<String> = None;
        let rest: &[C<'a>] = if !(kids[0].text_is("..") || kids[0].text_is("..<")) {
            let (v, w) = self.bound_of(kids[0], base);
            lo = v;
            why = w;
            &kids[1..]
        } else {
            &kids[..]
        };
        if rest.len() >= 2 && rest[1].is("constExpr") {
            let (v, w) = self.bound_of(rest[1], base);
            hi = v;
            if why.is_none() {
                why = w;
            }
            if rest[0].text_is("..<") {
                hi = hi.map(|h| h - 1);
            }
        }
        (lo, hi, why)
    }

    /// (value, why): a bound's compile-time value, or why it has none (6.3, X-512).
    pub fn bound_of(&self, node: C<'a>, base: &str) -> (Option<Int>, Option<String>) {
        let shown: String = node.tokens().iter().map(|t| t.text.as_str()).collect();
        let mut bases: BTreeSet<String> = BTreeSet::new();
        let Some(v) = self.const_int(node, &[], Some(&mut bases)) else {
            return (None, Some(format!("the bound `{shown}` is not an integer literal or a `const` resolved at compile time")));
        };
        if let Some(other) = bases.iter().find(|b| b.as_str() != base) {
            return (None, Some(format!("the bound `{shown}` has type {other}, not {base}: a `const` bound has the base type")));
        }
        (Some(v), None)
    }

    /// The compile-time value of a constant expression (6.3, X-512): an integer literal; a `const` (possibly
    /// module-qualified) whose initialiser folds the same way, its declared type's base noted in `bases`; `-x`;
    /// parentheses; and `+ - * / %` over them, products first, division truncating toward zero and the remainder taking
    /// the dividend's sign (6.4). Anything else, a divisor of zero and a constant that names itself have no value.
    pub fn const_int(&self, node: C<'a>, seen: &[String], bases: Option<&mut BTreeSet<String>>) -> Option<Int> {
        let ts = node.tokens();
        if ts.len() == 1 && ts[0].kind == Kind::Int {
            return Some(parse_int(&ts[0].text));
        }
        if ts.len() == 2 && ts[0].text == "-" && ts[1].kind == Kind::Int {
            return Some(-parse_int(&ts[1].text));
        }
        let Some(n) = node.node() else { return None };
        let kids = n.kids();
        let mut bases = bases;
        if n.name() == "constExpr" {
            if kids.len() == 1 {
                return self.const_int(kids[0], seen, bases);
            }
            let mut terms: Vec<Int> = Vec::new();
            let mut ops: Vec<String> = Vec::new();
            for k in &kids {
                if let Some(t) = k.tok() {
                    ops.push(t.text.clone());
                    continue;
                }
                let v = self.const_int(*k, seen, bases.as_deref_mut())?;
                terms.push(v);
            }
            let mut vals: Vec<Int> = vec![terms[0]];
            let mut sums: Vec<String> = Vec::new();
            for (op, t) in ops.iter().zip(terms[1..].iter()) {
                let t = *t;
                if op == "+" || op == "-" {
                    sums.push(op.clone());
                    vals.push(t);
                    continue;
                }
                let a = vals.pop().unwrap();
                if op == "*" {
                    vals.push(a.checked_mul(t)?);
                    continue;
                }
                if t == 0 {
                    return None;
                }
                let q = a.checked_div(t)?; // truncates toward zero
                vals.push(if op == "/" { q } else { a.checked_sub(q.checked_mul(t)?)? });
            }
            let mut total = vals[0];
            for (op, v) in sums.iter().zip(vals[1..].iter()) {
                total = if op == "+" { total.checked_add(*v)? } else { total.checked_sub(*v)? };
            }
            return Some(total);
        }
        if n.name() == "constTerm" && !kids.is_empty() {
            if kids[0].text_is("-") && kids.len() == 2 {
                return self.const_int(kids[1], seen, bases).map(|v| -v);
            }
            if kids[0].text_is("(") && kids.len() >= 2 {
                return self.const_int(kids[1], seen, bases);
            }
            if kids[0].tok().map_or(false, |t| t.kind == Kind::Lower) {
                let key: String = ts.iter().map(|t| t.text.as_str()).collect();
                return self.const_value(&key, seen, bases);
            }
            if kids[0].is("typeRef") && kids.len() == 3 && kids[2].is("memberName") {
                // `Type.member`, an associated constant of a class (9.3), folded from its own initialiser
                let trs = kids[0].node().unwrap().direct_tokens();
                let tname = if trs[0].kind == Kind::Lower && trs.len() > 2 { format!("{}.{}", trs[0].text, trs[2].text) } else { trs[0].text.clone() };
                let member = kids[2].node().unwrap().tokens()[0].text.clone();
                return self.const_value(&format!("{tname}.{member}"), seen, bases);
            }
        }
        if kids.len() == 1 && kids[0].node().is_some() {
            return self.const_int(kids[0], seen, bases);
        }
        None
    }

    /// The value of the `const` named `key` (a declaration of this module, or `alias.name` of an import), folded from
    /// its initialiser; its declared type's base, or `Int` for an unannotated constant (4.2), is noted.
    fn const_value(&self, key: &str, seen: &[String], bases: Option<&mut BTreeSet<String>>) -> Option<Int> {
        if seen.iter().any(|s| s == key) {
            return None;
        }
        let d = self.table.get(key).cloned().or_else(|| self.pending_heads.get(key).cloned());
        let node: N<'a> = match d {
            Some(d) if d.kind == "const" => d.node?,
            Some(_) => return None,
            None => {
                // `Type.member`: the `const` member of a class or interface declared here or imported (9.3)
                let (tname, member) = key.rsplit_once('.')?;
                let td = self.table.get(tname).cloned().or_else(|| self.pending_heads.get(tname).cloned())?;
                if td.kind != "class" && td.kind != "interface" {
                    return None;
                }
                td.node?.find_all("constDecl").into_iter().find(|cd| cd.direct_tokens().get(1).map_or(false, |t| t.text == member))?
            }
        };
        let ce = node.child_node("constExpr")?;
        let mut seen2: Vec<String> = seen.to_vec();
        seen2.push(key.to_string());
        let mut bases = bases;
        let v = self.const_int(C::N(ce), &seen2, bases.as_deref_mut())?;
        if let Some(b) = bases {
            let base = node.child_node("type").and_then(|t| t.tokens().into_iter().find(|x| x.kind == Kind::Upper).map(|x| x.text.clone())).unwrap_or_else(|| "Int".to_string());
            b.insert(base);
        }
        Some(v)
    }

    pub fn fn_type(&mut self, ft: N<'a>) -> Type {
        let kids = ft.kids();
        let words: Vec<&str> = kids.iter().filter_map(|k| k.tok()).map(|t| t.text.as_str()).collect();
        let kind = if words.contains(&"once") {
            "fn once"
        } else if words.contains(&"mut") {
            "fn mut"
        } else {
            "fn"
        };
        let modifiers: Vec<String> = ft.find("modifiers").map(|m| m.tokens().iter().map(|t| t.text.clone()).collect()).unwrap_or_default();
        let param_modes = kids.iter().filter(|k| k.is("paramType")).map(|k| k.node().unwrap().direct_tokens().into_iter().find(|t| matches!(t.text.as_str(), "editable" | "owned")).map(|t| t.text.clone()).unwrap_or_default()).collect();
        let params: Vec<Type> = kids.iter().filter(|k| k.is("paramType")).map(|k| { let n = k.node().unwrap(); self.type_of(n.find("type")) }).collect();
        let ret_nodes: Vec<N<'a>> = kids.iter().filter(|k| k.is("type")).map(|k| k.node().unwrap()).collect();
        let mut ret = match ret_nodes.last() {
            Some(r) => self.type_of(Some(*r)),
            None => void(),
        };
        let mut origin = String::new();
        let n = kids.len();
        if n >= 2 && kids[n - 2].text_is("from") && matches!(kids[n - 1], C::T(_)) {
            origin = kids[n - 1].tok().unwrap().text.clone();
        } else if let Type::Loan { inner, origin: o, .. } = &ret {
            if !self.loan_carrying_inner(inner) {
                origin = o.clone();
                ret = (**inner).clone();
            }
        }
        Type::Fn { capture_origins: Vec::new(), params, ret: Box::new(ret), kind: kind.into(), modifiers, origin, dynamic: words.contains(&"dynamic"), identity: String::new(), param_modes }
    }

    fn pick_identity(table: &Table<'a>, name: &str, identity: &str) -> Option<DeclRef<'a>> {
        if let Some(d) = table.get(name) {
            if type_kind(&d.kind) && d.name == name && d.type_identity() == identity {
                return Some(d.clone());
            }
        }
        table.decls.values().find(|x| x.name == name && x.type_identity() == identity && type_kind(&x.kind)).cloned()
    }

    /// The type declaration `identity` names (package@version/module plus the type's name, LANG-04-04).
    ///
    /// The module that declares it decides its fields, derives and lifetime. A finished table in this check's
    /// session wins over a head an importer copied in while the module was still loading; an identity never falls
    /// back to a same-named declaration, nor to a prelude declaration whose identity is empty.
    pub fn decl_by_identity(&self, name: &str, identity: &str) -> Option<DeclRef<'a>> {
        let Some((_, module)) = identity.split_once('/') else { return None };
        if module == self.module_name {
            if let Some(d) = Self::pick_identity(&self.table, name, identity) {
                return Some(d);
            }
            if let Some(h) = self.pending_heads.get(name) {
                if type_kind(&h.kind) && h.type_identity() == identity {
                    return Some(h.clone());
                }
            }
        }
        {
            let loaded = self.sess.loaded.borrow();
            for ((_, path), sub) in loaded.iter() {
                if path == module {
                    if let Some(d) = Self::pick_identity(sub, name, identity) {
                        return Some(d);
                    }
                }
            }
        }
        {
            let loading = self.sess.loading.borrow();
            for ((_, path), sub) in loading.iter() {
                if path == module {
                    if let Some(d) = Self::pick_identity(sub, name, identity) {
                        return Some(d);
                    }
                }
            }
        }
        if let Some(d) = Self::pick_identity(&self.table, name, identity) {
            return Some(d);
        }
        let loaded = self.sess.loaded.borrow();
        for sub in loaded.values() {
            if let Some(d) = Self::pick_identity(sub, name, identity) {
                return Some(d);
            }
        }
        None
    }

    /// A type of this name and identity in a module the check has already loaded (D-54.11).
    ///
    /// The declaration is that module's own, so it carries the identity. A same-named declaration in the module
    /// being checked is not this one.
    pub fn decl_from_loaded(&self, name: &str, package: &str) -> Option<DeclRef<'a>> {
        let loaded = self.sess.loaded.borrow();
        for table in loaded.values() {
            for d in table.decls.values() {
                if d.name == name && d.type_identity() == package && type_kind(&d.kind) {
                    return Some(d.clone());
                }
            }
        }
        None
    }

    /// The core module a declaration's type carries. A user type and a prelude-only type carry none. A name the
    /// prelude and exactly one other standard module both declare carries that other module on both copies. A head
    /// has no `core_home` yet; the library index answers for it.
    fn origin_of(&self, d: &Decl<'a>) -> String {
        if d.kind == "unknown" || !d.type_identity().is_empty() || !d.module.starts_with("fortis") {
            return String::new();
        }
        if d.module != "fortis.prelude" {
            return d.module.clone();
        }
        if !d.core_home.is_empty() {
            return d.core_home.clone();
        }
        self.core_module(&d.name)
    }

    /// The one non-prelude standard module that declares `name`, or empty when none does or several do.
    ///
    /// The prelude's own copy of that name is the same type (`Scalar` in `fortis.text`).
    pub(crate) fn core_module(&self, name: &str) -> String {
        if self.core_modules.borrow().is_none() {
            let mut found: HashMap<String, BTreeSet<String>> = HashMap::new();
            for (path, lm) in self.library().modules.iter() {
                let pkg = lm.package.split('@').next().unwrap_or(lm.package.as_str());
                if pkg != "fortis" || !path.starts_with("fortis") {
                    continue;
                }
                for (nm, d) in lm.decls.iter() {
                    if type_kind(d.kind) {
                        found.entry(nm.clone()).or_default().insert(path.clone());
                    }
                }
            }
            let mut index = HashMap::new();
            for (nm, mods) in found {
                let mut rest: Vec<String> = mods.into_iter().filter(|m| m != "fortis.prelude").collect();
                if rest.len() == 1 {
                    index.insert(nm, rest.pop().unwrap());
                }
            }
            *self.core_modules.borrow_mut() = Some(index);
        }
        self.core_modules.borrow().as_ref().and_then(|m| m.get(name).cloned()).unwrap_or_default()
    }

    fn pick_module(table: &Table<'a>, name: &str, module: &str) -> Option<DeclRef<'a>> {
        if let Some(d) = table.get(name) {
            if d.name == name && d.module == module && d.type_identity().is_empty() && type_kind(&d.kind) {
                return Some(d.clone());
            }
        }
        table.decls.values().find(|d| d.name == name && d.module == module && d.type_identity().is_empty() && type_kind(&d.kind)).cloned()
    }

    /// The core declaration `module` exports under `name`. Its package identity is empty, so the bare name in
    /// the module being checked is a different declaration.
    pub fn decl_by_module(&self, name: &str, module: &str, home: Option<&Table<'a>>) -> Option<DeclRef<'a>> {
        if let Some(d) = Self::pick_module(&self.prelude_decls, name, module) { return Some(d); }
        if let Some(table) = home {
            if let Some(d) = Self::pick_module(table, name, module) {
                return Some(d);
            }
        }
        if let Some(d) = Self::pick_module(&self.table, name, module) {
            return Some(d);
        }
        if let Some(h) = self.pending_heads.get(name) {
            if h.name == name && h.module == module && h.type_identity().is_empty() && type_kind(&h.kind) {
                return Some(h.clone());
            }
        }
        {
            let loaded = self.sess.loaded.borrow();
            for sub in loaded.values() {
                if let Some(d) = Self::pick_module(sub, name, module) {
                    return Some(d);
                }
            }
        }
        let loading = self.sess.loading.borrow();
        for sub in loading.values() {
            if let Some(d) = Self::pick_module(sub, name, module) {
                return Some(d);
            }
        }
        drop(loading);
        self.prelude_twin(name, module)
    }

    /// The prelude declaration of a name whose one other standard home is `module`. Facts of that type still resolve
    /// when the home module was not imported. A user class of the same name is not this declaration.
    fn prelude_twin(&self, name: &str, module: &str) -> Option<DeclRef<'a>> {
        if self.core_module(name) != module {
            return None;
        }
        let pick = |table: &Table<'a>| {
            if let Some(d) = table.get(name) {
                if d.name == name && d.module == "fortis.prelude" && d.type_identity().is_empty() && type_kind(&d.kind) {
                    return Some(d.clone());
                }
            }
            table.decls.values().find(|d| d.name == name && d.module == "fortis.prelude" && d.type_identity().is_empty() && type_kind(&d.kind)).cloned()
        };
        if let Some(d) = pick(&self.table) {
            return Some(d);
        }
        if let Some(h) = self.pending_heads.get(name) {
            if h.name == name && h.module == "fortis.prelude" && h.type_identity().is_empty() && type_kind(&h.kind) {
                return Some(h.clone());
            }
        }
        let loaded = self.sess.loaded.borrow();
        for sub in loaded.values() {
            if let Some(d) = pick(sub) {
                return Some(d);
            }
        }
        drop(loaded);
        let loading = self.sess.loading.borrow();
        for sub in loading.values() {
            if let Some(d) = pick(sub) {
                return Some(d);
            }
        }
        None
    }

    /// A core declaration a fallback found, as the module its origin names.
    ///
    /// An alias-qualified core type (D-140) and a core class found in a loaded module carry that module, the same
    /// stamp a type reference gets. A declaration that already carries a package identity is unchanged.
    fn stamped_core(&self, d: DeclRef<'a>, home: Option<&Table<'a>>) -> DeclRef<'a> {
        if !d.type_identity().is_empty() || !d.module.starts_with("fortis") {
            return d;
        }
        let module = self.origin_of(&d);
        let module = if module.is_empty() { d.module.clone() } else { module };
        self.decl_by_module(&d.name, &module, home).unwrap_or(d)
    }

    /// Resolve a declaration by identity. An empty package identity belongs to the core library; its origin
    /// selects a non-prelude module. Neither can fall back to a same-named program declaration.
    pub fn resolve_named(&self, n: &NamedT, home: Option<&Table<'a>>) -> Option<DeclRef<'a>> {
        if !n.package.is_empty() {
            // the loaded-module search is the same identity (D-54.11), never a same-named local declaration
            return self.decl_by_identity(&n.name, &n.package).or_else(|| self.decl_from_loaded(&n.name, &n.package));
        }
        if !n.origin.is_empty() {
            return self.decl_by_module(&n.name, &n.origin, home);
        }
        if let Some(d) = self.prelude_decls.get(&n.name) { return Some(d.clone()); }
        let table = home.unwrap_or(&self.table);
        if let Some(d) = table.get(&n.name) {
            if d.kind != "unknown" && d.type_identity().is_empty() {
                return Some(d.clone());
            }
        }
        // a core type reached only through a module alias (`import fortis.host`, then `host.Host`; D-140),
        // in the module whose table is being read. The declaration is the core one, stamped with its module.
        if let Some(d) = table.decls.iter().find(|(k, d)| k.contains('.') && d.name == n.name && d.type_identity().is_empty() && d.module.starts_with("fortis") && type_kind(&d.kind)).map(|(_, d)| d.clone()) {
            return Some(self.stamped_core(d, Some(table)));
        }
        self.decl_from_loaded(&n.name, "").map(|d| self.stamped_core(d, Some(table)))
    }

    pub fn resolve_type(&self, t: &Type, home: Option<&Table<'a>>) -> Option<DeclRef<'a>> {
        match t {
            Type::Refined { base, .. } => self.resolve_named(base, home),
            Type::Named(n) => self.resolve_named(n, home),
            _ => None,
        }
    }

    /// The table `d`'s field and alias types were written against: the finished table of its own module.
    fn home_table(&self, d: &Decl<'a>) -> Table<'a> {
        if d.module == self.module_name {
            return self.table.clone();
        }
        {
            let loaded = self.sess.loaded.borrow();
            for ((_, path), sub) in loaded.iter() {
                if path == &d.module {
                    if let Some(hit) = sub.get(&d.name) {
                        if hit.type_identity() == d.type_identity() {
                            return (**sub).clone();
                        }
                    }
                }
            }
        }
        {
            let loading = self.sess.loading.borrow();
            for ((_, path), sub) in loading.iter() {
                if path == &d.module {
                    if let Some(hit) = sub.get(&d.name) {
                        if hit.type_identity() == d.type_identity() {
                            return (**sub).clone();
                        }
                    }
                }
            }
        }
        self.table.clone()
    }

    fn fact_key(n: &NamedT, d: &Decl<'a>) -> (String, String) {
        let ident = if !n.package.is_empty() {
            n.package.clone()
        } else if !n.origin.is_empty() {
            n.origin.clone()
        } else {
            d.type_identity()
        };
        (ident, n.name.clone())
    }

    /// The table the declaration's own field types resolve in.
    ///
    /// `Checker` is this module's table: the recursive walk passes `None` and reads `self.table`, so the walk does
    /// not clone that table or hold it borrowed across the call. A home already in hand is forwarded. Only another
    /// module's table is copied out of the session.
    fn next_home<'b>(&self, d: &Decl<'a>, home: Option<&'b Table<'a>>) -> NextHome<'a, 'b> {
        if !d.type_identity().is_empty() && d.module != self.module_name {
            return NextHome::Owned(self.home_table(d));
        }
        if d.type_identity().is_empty() {
            if let Some(h) = home {
                if !std::ptr::eq(h, &self.table) {
                    return NextHome::Borrowed(h);
                }
            }
        }
        NextHome::Checker
    }

    /// A direct ListView newtype keeps its representation's loans (LR 6.9, D-555).
    pub fn view_newtype(&self, t: &Type) -> bool {
        let t = if let Type::Loan { inner, .. } = t { inner.as_ref() } else { t };
        let Some(d) = self.resolve_type(t, None) else { return false; };
        if d.kind != "type" { return false; }
        let Some(rep) = &d.alias else { return false; };
        self.resolve_type(rep, None).map_or(false, |rd| is_standard(&rd.module) && rd.name == "ListView")
    }

    fn loan_carrying_inner(&self, t: &Type) -> bool {
        if self.view_newtype(t) { return true; }
        let s = t.opt_inner().unwrap_or(t);
        match s {
            Type::Named(n) => {
                if n.package.is_empty() && ["StringView", "BytesView", "ListView", "ListCursor"].contains(&n.name.as_str()) {
                    return true;
                }
                self.resolve_named(n, None).map_or(false, |d| d.kind == "class" && d.lifetime.is_some())
            }
            Type::Loan { .. } => true,
            _ => false,
        }
    }

    // ---- sets (6.11, 6.12) ---------------------------------------------------------
    pub fn is_trivial(&self, t: &Type) -> bool {
        self.is_trivial_in(t, &self.table)
    }
    /// Triviality. A type that carries an identity is judged from the module that declares it; a name with no
    /// identity means what it means in `table` (an observation's field types, read in the module that declares the
    /// observation, which is not always the module that carries it).
    pub fn is_trivial_in(&self, t: &Type, table: &Table<'a>) -> bool {
        self.is_trivial_seen(t, &mut Vec::new(), Some(table))
    }
    fn is_trivial_seen(&self, t: &Type, seen: &mut Vec<(String, String)>, home: Option<&Table<'a>>) -> bool {
        match t {
            Type::Optional(i) => self.is_trivial_seen(i, seen, home),
            Type::Tuple(items) => items.iter().all(|x| self.is_trivial_seen(x, seen, home)),
            Type::Record(fs) => fs.iter().all(|(_, x)| self.is_trivial_seen(x, seen, home)),
            Type::Refined { base, .. } => self.is_trivial_seen(&base.as_type(), seen, home),
            Type::Named(n) => {
                if n.package.is_empty() && is_trivial_builtin(&n.name) {
                    return true;
                }
                let Some(d) = self.resolve_named(n, home) else { return false };
                let key = Self::fact_key(n, &d);
                if seen.contains(&key) {
                    return false;
                }
                seen.push(key);
                let next = self.next_home(&d, home);
                let nxt = next.get();
                let r = match d.kind.as_str() {
                    "type" => d.alias.as_ref().map_or(false, |a| self.is_trivial_seen(a, seen, nxt)),
                    "enum" => d.variants.values().all(|v| v.iter().all(|(_, ft)| self.is_trivial_seen(ft, seen, nxt))),
                    "class" => d.trivial,
                    _ => false,
                };
                seen.pop();
                r
            }
            _ => false,
        }
    }

    /// Whether destroying a value of this type can be observed (7.9): a class with `cleanup`, or a type whose
    /// destruction reaches one through a field, a payload, an item or a `Shared`/`Mutex` value. Loans, futures,
    /// tasks, closures and failures are out: the definitional interpreter destroys none of them, and the
    /// temporaries a launched task takes are v0.3 (D-150 item 3).
    pub fn has_drop(&self, t: &Type) -> bool {
        self.has_drop_seen(t, &mut vec![])
    }
    fn has_drop_seen(&self, t: &Type, seen: &mut Vec<String>) -> bool {
        match t {
            Type::Optional(i) => self.has_drop_seen(i, seen),
            Type::Tuple(items) => items.iter().any(|x| self.has_drop_seen(x, seen)),
            Type::Record(fs) => fs.iter().any(|(_, x)| self.has_drop_seen(x, seen)),
            Type::Refined { base, .. } => self.has_drop_seen(&base.as_type(), seen),
            Type::Named(n) => {
                if n.package.is_empty() && ["Future", "Task", "Failure", "Scope", "Weak", "Supervisor", "ListCursor", "Secret"].contains(&n.name.as_str()) {
                    return false;
                }
                // `Outcome[T]` owns its success value: discarding one destroys that value (7.9). A failure does not.
                if n.package.is_empty() && ["List", "Array", "Map", "OrderedMap", "Set", "FixedList", "FixedMap", "Shared", "Mutex", "Outcome"].contains(&n.name.as_str()) {
                    return n.args.iter().filter(|a| !matches!(a, Type::Named(x) if x.name.starts_with('#'))).any(|a| self.has_drop_seen(a, seen));
                }
                if n.package.is_empty() && is_trivial_builtin(&n.name) {
                    return false;
                }
                // the same nominal type with different arguments is a different instantiation: `Box[Box[Handle]]`
                // reaches `Box[Handle]`, while `Node[Handle]` whose field is `Node[T]` does not
                let key = format!("{n:?}");
                let d = match self.resolve_named(n, None) {
                    Some(d) if !seen.contains(&key) => d.clone(),
                    _ => return false,
                };
                seen.push(key);
                let subst: BTreeMap<String, Type> = d.type_params.iter().cloned().zip(n.args.iter().cloned()).collect();
                let r = match d.kind.as_str() {
                    "type" => d.alias.as_ref().map_or(false, |a| self.has_drop_seen(&self.subst(a, &subst), seen)),
                    "enum" => d.variants.values().any(|v| v.iter().any(|(_, ft)| self.has_drop_seen(&self.subst(ft, &subst), seen))),
                    "class" => d.has_cleanup || d.fields.values().any(|(ft, _, _)| self.has_drop_seen(&self.subst(ft, &subst), seen)),
                    _ => false,
                };
                seen.pop();
                r
            }
            _ => false,
        }
    }

    pub fn is_copyable(&self, t: &Type) -> bool {
        self.is_copyable_seen(t, &mut Vec::new(), None)
    }
    fn is_copyable_seen(&self, t: &Type, seen: &mut Vec<(String, String)>, home: Option<&Table<'a>>) -> bool {
        if self.is_linear_seen(t, &mut Vec::new(), home, &self.linear_params) {
            return false;
        }
        if self.is_trivial_seen(t, seen, Some(home.unwrap_or(&self.table))) {
            return true;
        }
        match t {
            Type::Optional(i) => self.is_copyable_seen(i, seen, home),
            Type::Tuple(items) => items.iter().all(|x| self.is_copyable_seen(x, seen, home)),
            Type::Record(fs) => fs.iter().all(|(_, x)| self.is_copyable_seen(x, seen, home)),
            Type::Refined { base, .. } => self.is_copyable_seen(&base.as_type(), seen, home),
            Type::TypeParam(n) => self.tparam_bounds.get(n).map_or(false, |b| b.contains("Copyable")),
            Type::Named(n) => {
                if n.package.is_empty() && NOT_COPYABLE.contains(&n.name.as_str()) {
                    return false;
                }
                if n.package.is_empty() && COPYABLE_HEAP.contains(&n.name.as_str()) {
                    return true;
                }
                if n.package.is_empty() && COPYABLE_CONTAINERS.contains(&n.name.as_str()) {
                    return n.args.iter().filter(|a| !matches!(a, Type::Named(x) if x.name.starts_with('#'))).all(|a| self.is_copyable_seen(a, seen, home));
                }
                let Some(d) = self.resolve_named(n, home) else { return false };
                let key = Self::fact_key(n, &d);
                if seen.contains(&key) {
                    return false;
                }
                seen.push(key);
                let next = self.next_home(&d, home);
                let nxt = next.get();
                let r = match d.kind.as_str() {
                    "type" => d.alias.as_ref().map_or(false, |a| self.is_copyable_seen(a, seen, nxt)),
                    "enum" => d.variants.values().all(|v| v.iter().all(|(_, ft)| self.is_copyable_seen(ft, seen, nxt))),
                    "class" => {
                        if (d.type_identity().is_empty() && d.name == "Capabilities") || self.implements_core_authority(&d) {
                            false
                        } else {
                            d.fields.values().all(|(ft, _, _)| self.is_copyable_seen(ft, seen, nxt))
                        }
                    }
                    _ => false,
                };
                seen.pop();
                r
            }
            _ => false,
        }
    }

    pub fn is_linear(&self, t: &Type) -> bool {
        self.is_linear_with(t, &BTreeSet::new())
    }
    pub fn is_linear_with(&self, t: &Type, linear: &BTreeSet<String>) -> bool {
        self.is_linear_seen(t, &mut Vec::new(), None, linear)
    }
    fn is_linear_seen(&self, t: &Type, seen: &mut Vec<(String, String)>, home: Option<&Table<'a>>, linear: &BTreeSet<String>) -> bool {
        match t {
            Type::TypeParam(p) => linear.contains(p),
            Type::Optional(i) => self.is_linear_seen(i, seen, home, linear),
            Type::Tuple(items) => items.iter().any(|x| self.is_linear_seen(x, seen, home, linear)),
            Type::Record(fs) => fs.iter().any(|(_, x)| self.is_linear_seen(x, seen, home, linear)),
            Type::Refined { base, .. } => self.is_linear_seen(&base.as_type(), seen, home, linear),
            Type::Fn { kind, .. } => kind == "fn once",
            Type::Named(n) => {
                if n.package.is_empty() && LINEAR_BUILTINS.contains(&n.name.as_str()) {
                    return true;
                }
                if n.package.is_empty() && COPYABLE_CONTAINERS.contains(&n.name.as_str()) {
                    return n.args.iter().any(|a| self.is_linear_seen(a, seen, home, linear));
                }
                let Some(d) = self.resolve_named(n, home) else { return false };
                if d.type_params.iter().zip(&n.args).any(|(tp, arg)| d.linear.contains(tp) && self.is_linear_seen(arg, seen, home, linear)) { return true; }
                let key = Self::fact_key(n, &d);
                if seen.contains(&key) {
                    return false;
                }
                seen.push(key);
                let next = self.next_home(&d, home);
                let nxt = next.get();
                let sub: BTreeMap<String, Type> = d.type_params.iter().cloned().zip(n.args.iter().cloned()).collect();
                let r = match d.kind.as_str() {
                    "type" => d.alias.as_ref().map_or(false, |a| self.is_linear_seen(&self.subst(a, &sub), seen, nxt, linear)),
                    "enum" => d.variants.values().any(|v| v.iter().any(|(_, ft)| self.is_linear_seen(&self.subst(ft, &sub), seen, nxt, linear))),
                    "class" => d.fields.values().any(|(ft, _, _)| self.is_linear_seen(&self.subst(ft, &sub), seen, nxt, linear)),
                    _ => false,
                };
                seen.pop();
                r
            }
            _ => false,
        }
    }

    pub fn linear_instantiations(&self, t: &Type, linear: &BTreeSet<String>) -> Vec<(String, String, String)> {
        let mut out = Vec::new();
        match t {
            Type::Named(n) => {
                if let Some(d) = self.resolve_named(n, None) {
                    let intrinsic = is_standard(&d.module) && (COPYABLE_CONTAINERS.contains(&n.name.as_str()) || NOT_COPYABLE.contains(&n.name.as_str()) || LINEAR_BUILTINS.contains(&n.name.as_str()) || ["Iterator", "Consumer", "ListCursor", "MapCursor", "SetCursor"].contains(&n.name.as_str()));
                    if matches!(d.kind.as_str(), "class" | "enum" | "type") && !intrinsic {
                        for (tp, arg) in d.type_params.iter().zip(&n.args) {
                            if !d.linear.contains(tp) && self.is_linear_with(arg, linear) {
                                out.push((n.show(), tp.clone(), arg.show()));
                            }
                        }
                    }
                }
                for arg in &n.args { out.extend(self.linear_instantiations(arg, linear)); }
            }
            Type::Optional(i) | Type::Loan { inner: i, .. } => out.extend(self.linear_instantiations(i, linear)),
            Type::Refined { base, .. } => out.extend(self.linear_instantiations(&base.as_type(), linear)),
            Type::Tuple(xs) => for x in xs { out.extend(self.linear_instantiations(x, linear)); },
            Type::Record(fs) => for (_, x) in fs { out.extend(self.linear_instantiations(x, linear)); },
            Type::Fn { params, ret, .. } => {
                for p in params { out.extend(self.linear_instantiations(p, linear)); }
                out.extend(self.linear_instantiations(ret, linear));
            }
            _ => {}
        }
        out
    }

    pub fn check_linear_instantiation(&mut self, t: &Type, linear: &BTreeSet<String>, tok: &Token) {
        for (instance, tp, arg) in self.linear_instantiations(t, linear) {
            self.rec("E-OBL-003", format!("{instance} cannot be instantiated with the linear type {arg} for `{tp}` without declaring `{tp}: linear` (6.12)"), "LANG-10-02", Some(tok),
                Json::obj([("obligation_origin", Json::str(format!("{}:{}", tok.line, tok.col))), ("unconsumed_path", Json::str(tp))]), "violated");
        }
    }

    // ---- implementation lookups --------------------------------------------------
    pub fn interface_decl(&self, key: &str) -> Option<DeclRef<'a>> {
        let (name, package) = key.split_once('@').unwrap_or((key, ""));
        self.resolve_type(&named_pkg(name, vec![], package), None)
    }

    pub fn implements(&self, t: &Type, iface: &str) -> bool {
        self.implements_seen(t, iface, &mut Vec::new(), None)
    }
    /// An `implement` block in this module applies to `n` only when it names that same declaration.
    /// Origin is not part of nominal identity, so it is compared on its own: a user's class and a core
    /// type of the same name are different declarations when both package identities are empty.
    fn block_implements(&self, n: &NamedT, iface: &str) -> bool {
        self.table.implement_blocks.iter().any(|(i, tt, _, _)| {
            i.interface_key() == iface && tt.name == n.name && tt.origin == n.origin && if n.package.is_empty() { tt.package.is_empty() } else { tt.package == n.package }
        })
    }
    fn implements_seen(&self, t: &Type, iface: &str, seen: &mut Vec<(String, String, String)>, home: Option<&Table<'a>>) -> bool {
        if iface == "Copyable" {
            return self.is_copyable_seen(t, &mut Vec::new(), home);
        }
        if iface == "Shareable" {
            return self.is_shareable_seen(t, &mut Vec::new(), home);
        }
        match t {
            // a refined type has its representation's default only when its refinement admits it (6.10): 0, or empty
            Type::Refined { base, lo, hi, .. } => (iface != "Default" || (lo.map_or(true, |l| l <= 0) && hi.map_or(true, |h| h >= 0))) && self.implements_seen(&base.as_type(), iface, seen, home),
            Type::Optional(i) => {
                if iface == "Default" {
                    return true;
                }
                if (iface == "Encodable" || iface == "Decodable") && i.is_optional() {
                    return false;
                }
                ["Equatable", "Hashable", "Encodable", "Decodable", "Displayable"].contains(&iface) && self.implements_seen(i, iface, seen, home)
            }
            Type::Tuple(items) => ["Equatable", "Hashable", "Comparable", "Encodable", "Decodable"].contains(&iface) && items.iter().all(|x| self.implements_seen(x, iface, seen, home)),
            Type::Record(fs) => ["Equatable", "Hashable", "Encodable", "Decodable"].contains(&iface) && fs.iter().all(|(_, x)| self.implements_seen(x, iface, seen, home)),
            Type::TypeParam(_) | Type::Unknown(_) => true,
            Type::Named(n) => {
                if n.package.is_empty() && builtin_implements(&n.name, iface) {
                    return true;
                }
                if n.package.is_empty() && COPYABLE_CONTAINERS.contains(&n.name.as_str()) && ["Equatable", "Hashable", "Encodable", "Decodable", "Displayable", "Default"].contains(&iface) {
                    if iface == "Default" {
                        return true;
                    }
                    return n.args.iter().filter(|a| !matches!(a, Type::Named(x) if x.name.starts_with('#'))).all(|a| self.implements_seen(a, iface, seen, home));
                }
                if n.package.is_empty() && ["Shared", "Weak", "Mutex"].contains(&n.name.as_str()) && iface == "Shareable" {
                    return true;
                }
                let Some(d) = self.resolve_named(n, home) else { return false };
                let key = (Self::fact_key(n, &d).0, n.name.clone(), iface.to_string());
                if seen.contains(&key) {
                    return false;
                }
                seen.push(key);
                let idecl = self.interface_decl(iface);
                let next = self.next_home(&d, home);
                let nxt = next.get();
                let r = {
                    if d.kind == "type" && ["Encodable", "Decodable", "Codec", "Equatable", "Hashable", "Comparable", "Displayable", "Default"].contains(&iface) {
                        d.alias.as_ref().map_or(false, |a| self.implements_seen(a, iface, seen, nxt))
                    } else if d.implements.iter().any(|i| i.interface_key() == iface) {
                        true
                    } else if d.derives.iter().any(|(n2, _)| n2 == iface) {
                        true
                    } else if (iface == "Encodable" || iface == "Decodable") && d.derives.iter().any(|(n2, _)| n2 == "Codec") {
                        true
                    } else if self.block_implements(n, iface) {
                        true
                    } else if let Some(id) = &idecl {
                        if id.kind == "interface" && id.methods.is_empty() && id.constants.is_empty() && !id.requires.is_empty() {
                            id.requires.iter().all(|r| self.implements_seen(t, &r.interface_key(), seen, home))
                        } else {
                            false
                        }
                    } else if iface == "Codec" {
                        self.implements_seen(t, "Encodable", seen, home) && self.implements_seen(t, "Decodable", seen, home)
                    } else {
                        false
                    }
                };
                seen.pop();
                r
            }
            _ => false,
        }
    }

    pub fn is_shareable(&self, t: &Type) -> bool {
        self.is_shareable_seen(t, &mut Vec::new(), None)
    }
    fn is_shareable_seen(&self, t: &Type, seen: &mut Vec<(String, String)>, home: Option<&Table<'a>>) -> bool {
        if self.is_trivial_seen(t, seen, Some(home.unwrap_or(&self.table))) {
            return true;
        }
        match t {
            Type::TypeParam(n) => self.tparam_bounds.get(n).map_or(false, |b| b.contains("Shareable")),
            Type::Loan { .. } | Type::Dynamic(_) | Type::Unknown(_) => false,
            Type::Optional(i) => self.is_shareable_seen(i, seen, home),
            Type::Refined { base, .. } => self.is_shareable_seen(&base.as_type(), seen, home),
            Type::Named(n) => {
                let nm = if n.package.is_empty() { n.name.as_str() } else { "" };
                if ["StringView", "BytesView", "ListView", "MutexGuard", "Future", "Task"].contains(&nm) {
                    return false;
                }
                if n.package.is_empty() && COPYABLE_HEAP.contains(&nm) {
                    return true;
                }
                if n.package.is_empty() && COPYABLE_CONTAINERS.contains(&nm) {
                    return n.args.iter().filter(|a| !matches!(a, Type::Named(x) if x.name.starts_with('#'))).all(|a| self.is_shareable_seen(a, seen, home));
                }
                if ["Shared", "Weak", "Mutex"].contains(&nm) {
                    return n.args.iter().all(|a| self.is_shareable_seen(a, seen, home));
                }
                if nm == "AtomicInt" || nm == "AtomicBool" {
                    return true;
                }
                let Some(d) = self.resolve_named(n, home) else { return false };
                let key = Self::fact_key(n, &d);
                if seen.contains(&key) {
                    return false;
                }
                seen.push(key);
                let next = self.next_home(&d, home);
                let nxt = next.get();
                let r = match d.kind.as_str() {
                    "class" => {
                        if d.lifetime.is_some() {
                            false
                        } else if self.implements_core_authority(&d) {
                            true
                        } else {
                            d.fields.values().all(|(ft, _, _)| self.is_shareable_seen(ft, seen, nxt))
                        }
                    }
                    "enum" => d.variants.values().all(|v| v.iter().all(|(_, ft)| self.is_shareable_seen(ft, seen, nxt))),
                    "type" => d.alias.as_ref().map_or(false, |a| self.is_shareable_seen(a, seen, nxt)),
                    _ => false,
                };
                seen.pop();
                r
            }
            Type::Tuple(items) => items.iter().all(|x| self.is_shareable_seen(x, seen, home)),
            Type::Record(fs) => fs.iter().all(|(_, x)| self.is_shareable_seen(x, seen, home)),
            Type::Fn { kind, .. } => kind == "fn" || kind == "fn once",
        }
    }

    // ---- checks ----------------------------------------------------------------------
    pub fn check(&mut self) {
        let decls: Vec<DeclRef<'a>> = self.table.decls.values().cloned().collect();
        for d in decls {
            if d.module != self.module_name {
                continue;
            }
            if d.kind == "observation" {
                for (fname, (ft, _)) in d.obs_fields.iter() {
                    if ft.is_named("String") {
                        self.rec(
                            "E-OBS-012",
                            format!("observation {}: String field `{fname}` must declare a scalar bound (`String<scalars: 0..n>`) so the frame stays bounded (13)", d.name),
                            "LANG-13-03",
                            Some(&tok_of(&d)),
                            Json::obj([("frame_field", Json::str(fname)), ("exit_path", Json::str("declaration"))]),
                            "violated",
                        );
                    }
                    if known(ft) && !self.encodable(ft) {
                        self.rec(
                            "E-TYP-090",
                            format!(
                                "observation {}: field `{fname}: {}` is not Encodable; secrets, authorities, loans, resources, closures, futures and tasks cannot be captured (13)",
                                d.name,
                                ft.show()
                            ),
                            "LANG-13-01",
                            Some(&tok_of(&d)),
                            Self::typ_fields("Encodable", &ft.show(), fname),
                            "violated",
                        );
                    }
                }
            }
            if d.kind == "class" || d.kind == "enum" {
                self.check_type_decl(&d);
            } else if d.kind == "interface" {
                self.check_interface(&d);

            }
            self.check_value_types(&d);
            if d.kind == "contract" {
                self.check_contract_decl(&d);
            }
            self.check_trusted(&d);
        }
        self.check_trusted_blocks();
        self.check_coherence();
        self.check_conversions();
    }

    /// A module of a standard package (the published compatibility set: the `fortis` package and its kits).
    /// A module of a package of the published compatibility set (library/fortis/compatibility.yaml): one the toolchain's
    /// standard library provides, of a package the set lists, checked from that library's own source. A package that
    /// merely names itself `fortis.*` is not one (D-146).
    pub fn std_module_name(&self, module: &str) -> bool {
        if module == "fortis.prelude" {
            return true;
        }
        let Some(lm) = self.library().modules.get(module) else { return false };
        let base = lm.package.split('@').next().unwrap_or("");
        lm.library.is_none() && fortis_names::compat::set().1.contains_key(base)
    }

    /// The module being checked is a standard library module, checked from that library's own source (D-146).
    pub fn standard_module(&self) -> bool {
        if !self.std_module_name(&self.module_name) {
            return false;
        }
        // checked from the standard library's own sources (the shipped library/ or its reference copy), not from a
        // package that merely names itself `fortis.*`
        let root = fortis_names::library::repo_root();
        let file = std::fs::canonicalize(root.join(&self.file)).unwrap_or_else(|_| root.join(&self.file));
        [root.join("library"), root.join("reference").join("library")]
            .iter()
            .any(|d| std::fs::canonicalize(d).map_or(false, |d| file.starts_with(d)))
    }

    /// An implementation block's members are functions too: none may be `trusted` outside the standard packages (D-146).
    fn check_trusted_blocks(&mut self) {
        if self.standard_module() {
            return;
        }
        let blocks: Vec<(NamedT, N<'a>)> = self.table.implement_blocks.iter().filter(|(_, _, m, _)| *m == self.module_name).map(|(_, t, _, n)| (t.clone(), *n)).collect();
        for (target, node) in blocks {
            for m in node.child_nodes("implementMember") {
                let Some(head) = m.find("methodHead") else { continue };
                let sig = self.signature(head, true, m.find("funcBody"));
                if sig.has_mod("trusted") {
                    let label = format!("{}.{}", target.name, sig.name);
                    let at = head.tokens()[0].clone();
                    self.rec(
                        "E-EFF-001",
                        format!("{label} is declared `trusted`, which only the standard packages may write: a program's effects are proved, never declared (11.3)"),
                        "LANG-11-04",
                        Some(&at),
                        Json::obj([("required_contract", Json::str("a standard package")), ("violating_effect", Json::str("trusted")), ("witness_path", Json::str(&label))]),
                        "violated",
                    );
                }
            }
        }
    }

    /// Does declaration `d` implement the core marker `Authority` (fortis.prelude), resolved as d's own module names it:
    /// a program's own `interface Authority` is not the marker (D-172).
    pub fn implements_core_authority(&self, d: &Decl<'a>) -> bool {
        let named = d.implements.iter().any(|i| i.name == "Authority") || self.table.implement_blocks.iter().any(|(i, tt, _, _)| i.name == "Authority" && tt.name == d.name);
        if !named {
            return false;
        }
        if self.std_module_name(&d.module) {
            return true;
        }
        let quals = self.authority_qualifiers(d);
        let loaded = self.sess.loaded.borrow();
        let table: Option<&Table<'a>> = if d.module == self.module_name { Some(&self.table) } else { loaded.iter().find(|((_, p), _)| *p == d.module).map(|(_, t)| &**t) };
        let Some(table) = table else { return true };
        quals.iter().any(|q| {
            let a = match q {
                Some(q) => table.decls.get(&format!("{q}.Authority")),
                None => table.get("Authority"),
            };
            a.map_or(false, |a| self.std_module_name(&a.module))
        })
    }

    /// How d's own declaration names each `Authority` it implements: None for the bare name, else the module alias that
    /// qualifies it (`implements pre.Authority`), in its `implements` list or an implementation block (D-172).
    fn authority_qualifiers(&self, d: &Decl<'a>) -> Vec<Option<String>> {
        fn qual(tr: N<'_>) -> Option<Option<String>> {
            let ts = tr.direct_tokens();
            if ts.len() >= 3 && ts[0].kind == Kind::Lower && ts[1].text == "." && ts[2].text == "Authority" {
                return Some(Some(ts[0].text.clone()));
            }
            if ts.first().map_or(false, |t| t.text == "Authority") {
                Some(None)
            } else {
                None
            }
        }
        let mut out = vec![];
        if let Some(node) = d.node {
            let mut after = false;
            for k in node.kids() {
                match k {
                    C::T(t) => after = t.text == "implements",
                    C::N(n) if n.name() == "typeList" && after => {
                        for ty in n.child_nodes("type") {
                            if let Some(q) = ty.find("typeRef").and_then(qual) {
                                out.push(q);
                            }
                        }
                        after = false;
                    }
                    _ => {}
                }
            }
        }
        for (i, tt, _, n) in self.table.implement_blocks.iter() {
            if i.name == "Authority" && tt.name == d.name {
                if let Some(q) = n.find("typeRef").and_then(qual) {
                    out.push(q);
                }
            }
        }
        out
    }

    /// `trusted` declares effects the checker does not prove, so only the standard library and its audited first-party
    /// adapters, the packages of the published compatibility set, may write it (11.3; D-146, the integrator's decision).
    fn check_trusted(&mut self, d: &DeclRef<'a>) {
        if self.standard_module() {
            return;
        }
        let tok = tok_of(d);
        let sigs: Vec<(String, Sig<'a>)> = if d.kind == "func" {
            d.sig.clone().map(|s| vec![(d.name.clone(), s)]).unwrap_or_default()
        } else {
            d.methods.iter().map(|(m, s)| (format!("{}.{m}", d.name), s.clone())).collect()
        };
        for (label, sig) in sigs {
            if sig.has_mod("trusted") {
                let at = sig.node.map(|n| n.tokens()[0].clone()).unwrap_or_else(|| tok.clone());
                self.rec(
                    "E-EFF-001",
                    format!("{label} is declared `trusted`, which only the standard packages may write: a program's effects are proved, never declared (11.3)"),
                    "LANG-11-04",
                    Some(&at),
                    Json::obj([("required_contract", Json::str("a standard package")), ("violating_effect", Json::str("trusted")), ("witness_path", Json::str(&label))]),
                    "violated",
                );
            }
        }
    }

    /// Alias-qualified type references in a declaration's own types (not its bodies, constants, `where` bounds,
    /// `implements`, `requires` or `with` lists, nor the name after `dynamic`) that name an interface whose bare name this
    /// module gives to another declaration. A type carries only the name, so the walk over types cannot tell
    /// `data.Connection` from a local `Connection` (D-21).
    pub fn shadowed_qualified_interfaces(&self, node: N<'a>) -> Vec<(String, String, Token)> {
        fn walk<'a>(dc: &DeclChecker<'a>, n: N<'a>, dynamic_ref: bool, out: &mut Vec<(String, String, Token)>) {
            let nm = n.name();
            if matches!(nm, "funcBody" | "block" | "constExpr" | "whereClause" | "typeList" | "aspectPart") {
                return;
            }
            if nm == "typeRef" && !dynamic_ref {
                let ts = n.direct_tokens();
                if ts.len() >= 3 && ts[0].kind == Kind::Lower && ts[1].text == "." {
                    let q = format!("{}.{}", ts[0].text, ts[2].text);
                    if let Some(d) = dc.table.decls.get(&q) {
                        if let Some(bare) = dc.table.get(&ts[2].text) {
                            if d.kind == "interface" && !Rc::ptr_eq(d, bare) {
                                out.push((q, ts[2].text.clone(), ts[2].clone()));
                            }
                        }
                    }
                }
            }
            let dynamic = matches!(nm, "loanType" | "fnType") && n.direct_tokens().iter().any(|t| t.text == "dynamic");
            for c in n.kids().into_iter().filter_map(|c| c.node()) {
                walk(dc, c, dynamic && c.name() == "typeRef", out);
            }
        }
        let mut out = Vec::new();
        walk(self, node, false, &mut out);
        out
    }

    /// The interfaces `t` names as value types, without `dynamic` (9.5): no value has such a type (D-21).
    pub fn bare_interfaces(&self, t: &Type) -> Vec<String> {
        let mut out = Vec::new();
        fn walk<'a>(dc: &DeclChecker<'a>, x: &Type, out: &mut Vec<String>) {
            match x {
                Type::Named(n) => {
                    // an interface reached only through a module alias (`data.Connection`; review of D-21)
                    if dc.resolve_named(n, None).map_or(false, |d| d.kind == "interface") {
                        out.push(n.name.clone());
                    }
                    for a in &n.args {
                        walk(dc, a, out);
                    }
                }
                Type::Optional(i) => walk(dc, i, out),
                Type::Loan { inner, .. } => walk(dc, inner, out),
                Type::Tuple(items) => items.iter().for_each(|i| walk(dc, i, out)),
                Type::Record(fs) => fs.iter().for_each(|(_, f)| walk(dc, f, out)),
                Type::Fn { params, ret, .. } => {
                    params.iter().for_each(|p| walk(dc, p, out));
                    walk(dc, ret, out);
                }
                _ => {}
            }
        }
        walk(self, t, &mut out);
        out
    }

    /// Does a signature type name `Self` (an interface's own type) anywhere?
    pub fn mentions_self(t: &Type) -> bool {
        match t {
            Type::TypeParam(p) => p == "Self",
            Type::Named(n) => n.name == "Self" || n.args.iter().any(Self::mentions_self),
            Type::Optional(i) | Type::Loan { inner: i, .. } => Self::mentions_self(i),
            Type::Dynamic(n) => n.args.iter().any(Self::mentions_self),
            Type::Tuple(ts) => ts.iter().any(Self::mentions_self),
            Type::Record(fs) => fs.iter().any(|(_, x)| Self::mentions_self(x)),
            Type::Fn { params, ret, .. } => params.iter().any(Self::mentions_self) || Self::mentions_self(ret),
            _ => false,
        }
    }

    /// Why an interface is not dynamic-eligible (9.5), or None: a generic method, or a method whose parameters or
    /// result name `Self` beyond its receiver (the other value's type could not be known through a `dynamic`). An
    /// associated function or constant is not reached through a `dynamic` value and does not count.
    pub fn dynamic_ineligible(&self, idecl: &Decl<'a>) -> Option<String> {
        for (mname, sig) in idecl.methods.iter() {
            if sig.receiver.is_none() {
                continue;
            }
            if !sig.type_params.is_empty() {
                return Some(format!("its method `{mname}` is generic"));
            }
            if sig.params.iter().any(|(_, pt, _)| Self::mentions_self(pt)) || Self::mentions_self(&sig.ret) {
                return Some(format!("its method `{mname}` takes or returns `Self`"));
            }
        }
        None
    }

    /// Every `dynamic I` written inside `t` that 9.5 refuses: (code, rule, the interface shown, why). `E-TYP-172`
    /// when `I` is not dynamic-eligible; `E-TYP-090` when `I`'s `where` bound does not hold for the written arguments
    /// (checked at every use, D-69).
    /// D-549: hash-container requirements hold for written and inferred types, including nested types.
    pub fn key_problems(&self, t: &Type, bounds: &BTreeMap<String, BTreeSet<String>>, unfixed: bool) -> Vec<(String, String, String)> {
        fn grants(dc: &DeclChecker<'_>, have: &str, want: &str, seen: &[String]) -> bool {
            if have == want { return true; }
            if seen.iter().any(|s| s == have) { return false; }
            let mut next = seen.to_vec();
            next.push(have.into());
            if let Some(d) = dc.interface_decl(have) {
                d.requires.iter().any(|r| grants(dc, &r.interface_key(), want, &next))
            } else {
                matches!(have, "Hashable" | "Comparable") && want == "Equatable"
            }
        }
        fn holds<'a>(dc: &DeclChecker<'a>, x: &Type, iface: &str, bounds: &BTreeMap<String, BTreeSet<String>>, unfixed: bool, seen: &[String], home: Option<&Table<'a>>) -> bool {
            match x {
                Type::Unknown(_) => true,
                Type::TypeParam(p) => (unfixed && !dc.type_params.contains(p)) || bounds.get(p).map_or(false, |bs| bs.iter().any(|b| grants(dc, b, iface, &[]))),
                Type::Refined { base, .. } => holds(dc, &base.as_type(), iface, bounds, unfixed, seen, home),
                Type::Optional(a) => holds(dc, a, iface, bounds, unfixed, seen, home),
                Type::Tuple(_) | Type::Record(_) => false, // structural conveniences do not satisfy interfaces (6.9)
                Type::Named(n) => {
                    let d = dc.resolve_named(n, home);
                    let core = d.as_ref().map_or(true, |d| d.module == "fortis" || d.module.starts_with("fortis."));
                    if core && COPYABLE_CONTAINERS.contains(&n.name.as_str()) {
                        return n.args.iter().filter(|a| !matches!(a, Type::Named(n) if n.name.starts_with('#'))).all(|a| holds(dc, a, iface, bounds, unfixed, seen, home));
                    }
                    let key = format!("{}:{}:{iface}", d.as_ref().map_or("", |d| &d.module), x.show());
                    if seen.contains(&key) { return d.as_ref().map_or(false, |d| d.derives.iter().any(|(i, _)| i == iface)); }
                    let mut path = seen.to_vec();
                    path.push(key);
                    if let Some(d) = d {
                        let next = dc.next_home(&d, home);
                        if d.kind == "type" {
                            return d.alias.as_ref().map_or(false, |a| holds(dc, a, iface, bounds, unfixed, &path, next.get()));
                        }
                        if !core {
                            if !(d.implements.iter().any(|i| i.interface_key() == iface) || d.derives.iter().any(|(i, _)| i == iface) || dc.block_implements(n, iface)) { return false; }
                            if d.derives.iter().any(|(i, _)| i == iface) {
                                let sub: BTreeMap<String, Type> = d.type_params.iter().cloned().zip(n.args.iter().cloned()).collect();
                                return d.fields.values().map(|(f, _, _)| f).chain(d.variants.values().flat_map(|v| v.iter().map(|(_, f)| f)))
                                    .all(|f| holds(dc, &dc.subst(f, &sub), iface, bounds, unfixed, &path, next.get()));
                            }
                            return true;
                        }
                    }
                    dc.implements_seen(x, iface, &mut Vec::new(), home)
                }
                _ => false,
            }
        }
        fn walk(dc: &DeclChecker<'_>, x: &Type, bounds: &BTreeMap<String, BTreeSet<String>>, unfixed: bool, out: &mut Vec<(String, String, String)>) {
            match x {
                Type::Named(n) => {
                    if ["Set", "Map"].contains(&n.name.as_str()) && !n.args.is_empty() && dc.resolve_named(n, None).as_ref().map_or(true, |d| d.module == "fortis" || d.module.starts_with("fortis.")) {
                        let key = &n.args[0];
                        if !["Hashable", "Equatable"].iter().all(|i| holds(dc, key, i, bounds, unfixed, &[], None)) {
                            let shown = diagnostic_types(&[x, key, &named("Hashable"), &named("Equatable")]);
                            let item = (shown[0].clone(), shown[1].clone(), format!("{} and {}", shown[2], shown[3]));
                            if !out.contains(&item) { out.push(item); }
                        }
                    }
                    n.args.iter().for_each(|a| walk(dc, a, bounds, unfixed, out));
                }
                Type::Optional(a) | Type::Loan { inner: a, .. } => walk(dc, a, bounds, unfixed, out),
                Type::Refined { base, .. } => walk(dc, &base.as_type(), bounds, unfixed, out),
                Type::Dynamic(n) => n.args.iter().for_each(|a| walk(dc, a, bounds, unfixed, out)),
                Type::Tuple(items) => items.iter().for_each(|a| walk(dc, a, bounds, unfixed, out)),
                Type::Record(fs) => fs.iter().for_each(|(_, a)| walk(dc, a, bounds, unfixed, out)),
                Type::Fn { params, ret, .. } => {
                    params.iter().for_each(|a| walk(dc, a, bounds, unfixed, out));
                    walk(dc, ret, bounds, unfixed, out);
                }
                _ => {}
            }
        }
        let mut out = Vec::new();
        walk(self, t, bounds, unfixed, &mut out);
        out
    }

    pub fn check_keys(&mut self, t: &Type, tok: &Token, bounds: Option<&BTreeMap<String, BTreeSet<String>>>, unfixed: bool) {
        for (container, key, required) in self.key_problems(t, bounds.unwrap_or(&self.tparam_bounds), unfixed) {
            self.rec("E-TYP-090", format!("{container} needs a key that implements {required}; {key} does not (6.7)"), "LANG-06-06", Some(tok), Self::typ_fields(&required, &key, &container), "violated");
        }
    }

    pub fn dynamic_problems(&self, t: &Type) -> Vec<(&'static str, &'static str, String, String)> {
        let mut out = Vec::new();
        fn walk<'a>(dc: &DeclChecker<'a>, x: &Type, out: &mut Vec<(&'static str, &'static str, String, String)>) {
            match x {
                Type::Dynamic(i) => {
                    let d = dc.resolve_named(i, None);
                    if let Some(d) = d {
                        if d.kind == "interface" {
                            if let Some(why) = dc.dynamic_ineligible(&d) {
                                out.push(("E-TYP-172", "LANG-09-07", x.show(), format!("`{}` is not dynamic-eligible: {why} (9.5); take the value as a type parameter bounded by {}", i.name, i.name)));
                            }
                            let sub: BTreeMap<String, Type> = d.type_params.iter().cloned().zip(i.args.iter().cloned()).collect();
                            for (tp, bound) in d.where_.iter() {
                                let Some(arg) = sub.get(tp) else { continue };
                                if matches!(arg, Type::TypeParam(_) | Type::Unknown(_)) {
                                    continue;
                                }
                                if !dc.implements(arg, &bound.interface_key()) {
                                    let displays = diagnostic_types(&[x, &Type::Named(bound.clone()), arg]);
                                    out.push(("E-TYP-090", "LANG-06-10", displays[0].clone(), format!("`{}` needs `{tp}` to implement {}; {} does not (9.5)", displays[0].clone(), displays[1], displays[2])));
                                }
                            }
                        }
                    }
                    i.args.iter().for_each(|a| walk(dc, a, out));
                }
                Type::Named(n) => n.args.iter().for_each(|a| walk(dc, a, out)),
                Type::Optional(i) | Type::Loan { inner: i, .. } => walk(dc, i, out),
                Type::Tuple(items) => items.iter().for_each(|i| walk(dc, i, out)),
                Type::Record(fs) => fs.iter().for_each(|(_, f)| walk(dc, f, out)),
                Type::Fn { params, ret, .. } => {
                    params.iter().for_each(|p| walk(dc, p, out));
                    walk(dc, ret, out);
                }
                _ => {}
            }
        }
        walk(self, t, &mut out);
        out
    }

    /// A contract's items must agree (11.5, E-EFF-050): `pure` with `requires capability` (a pure function receives no
    /// authority), a modifier that permits an effect beside `forbids` of that effect (`async` suspend, `try` fail,
    /// `recovers` recovers), and one authority both required and excluded. The type a `capability` clause names is an
    /// Authority type (11.6), or the clause is E-TYP-040: a type in scope by its declaration, a name not in scope by the
    /// core capabilities' names, as an authority parameter is judged.
    fn check_contract_decl(&mut self, d: &DeclRef<'a>) {
        let tok = tok_of(d);
        for c in crate::effects::contract_contradictions(&[(d.name.clone(), d.items.clone())], false) {
            let msg = match c.kind {
                "pure" => format!("contract {} is `pure` and requires capability {}: a pure function receives no authority, so the contract contradicts itself (11.5)", d.name, c.claim),
                "forbids" => format!("contract {} is `{}`, which permits `{}`, and forbids `{}`: the contract contradicts itself (11.5)", d.name, c.claim, c.effect, c.effect),
                _ => format!("contract {} both requires and excludes capability {}: the contract contradicts itself (11.5)", d.name, c.claim),
            };
            let fields = Json::obj([("required_contract", Json::str(&d.name)), ("violating_effect", Json::str(&c.effect)), ("witness_path", Json::str(&c.claim))]);
            self.rec("E-EFF-050", msg, "LANG-11-06", Some(&tok), fields, "violated");
        }
        let Some(node) = d.node else { return };
        for it in node.child_nodes("contractItem") {
            let ts = it.tokens();
            if !(ts[0].text == "requires" || ts[0].text == "excludes") {
                continue;
            }
            let a = (*ts.last().unwrap()).clone();
            let ok = match self.table.get(&a.text) {
                Some(ad) if ad.kind != "unknown" => matches!(ad.kind.as_str(), "class" | "enum" | "type") && self.implements_core_authority(ad),
                _ => crate::effects::AUTHORITIES.contains(&a.text.as_str()),
            };
            if !ok {
                self.rec("E-TYP-040", format!("contract {} names capability {}, which is not an Authority type (11.5, 11.6)", d.name, a.text), "LANG-11-06", Some(&a), Self::typ_fields("an Authority type", &a.text, &a.text), "violated");
            }
        }
    }

    /// A field, variant payload, parameter or result whose type is an interface itself is refused at its declaration
    /// (D-21): an interface is used through `dynamic I`, `Shared[dynamic I]` or a type parameter bounded by it (9.5).
    fn check_value_types(&mut self, d: &DeclRef<'a>) {
        let tok = tok_of(d);
        let mut bounds: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (tp, b) in &d.where_ { bounds.entry(tp.clone()).or_default().insert(b.interface_key()); }
        for ct in d.constants.values() { self.check_keys(ct, &tok, Some(&bounds), false); }
        for (ft, _, _) in d.fields.values() { self.check_keys(ft, &tok, Some(&bounds), false); }
        for payload in d.variants.values() { for (_, ft) in payload { self.check_keys(ft, &tok, Some(&bounds), false); } }
        if let Some(a) = &d.alias { self.check_keys(a, &tok, Some(&bounds), false); }
        let mut items: Vec<(Type, String, Token)> = Vec::new();
        if let Some(alias) = &d.alias { items.push((alias.clone(), format!("type of {}", d.name), tok.clone())); }
        for (fname, (ft, _, _)) in d.fields.iter() {
            items.push((ft.clone(), format!("field `{fname}` of {}", d.name), tok.clone()));
        }
        for (vname, payload) in d.variants.iter() {
            for (fname, ft) in payload {
                items.push((ft.clone(), format!("field `{fname}` of variant {}.{vname}", d.name), tok.clone()));
            }
        }
        let sigs: Vec<(String, Sig<'a>)> = if d.kind == "func" {
            d.sig.clone().map(|s| vec![(d.name.clone(), s)]).unwrap_or_default()
        } else {
            d.methods.iter().map(|(m, s)| (format!("{}.{m}", d.name), s.clone())).collect()
        };
        for (label, sig) in sigs {
            let linear: BTreeSet<String> = d.linear.iter().chain(sig.linear.iter()).cloned().collect();
            let at = sig.node.map(|n| n.tokens()[0].clone()).unwrap_or_else(|| tok.clone());
            let mut sb = bounds.clone();
            for (tp, b) in &sig.where_ { sb.entry(tp.clone()).or_default().insert(b.interface_key()); }
            for (_, pt, _) in &sig.params { self.check_keys(pt, &at, Some(&sb), false); }
            self.check_keys(&sig.ret, &at, Some(&sb), false);
            for (pn, pt, _) in &sig.params {
                self.check_linear_instantiation(pt, &linear, &at);
                items.push((pt.clone(), format!("parameter `{pn}` of {label}"), at.clone()));
            }
            self.check_linear_instantiation(&sig.ret, &linear, &at);
            items.push((sig.ret.clone(), format!("the result of {label}"), at.clone()));
        }
        for (t, what, at) in items {
            self.check_linear_instantiation(&t, &d.linear.iter().cloned().collect(), &at);
            for (code, rule, shown, why) in self.dynamic_problems(&t) {
                self.rec(code, format!("{what}: {why}"), rule, Some(&at), Self::typ_fields("a dynamic-eligible interface whose bounds hold", &shown, &what), "violated");
            }
            for i in self.bare_interfaces(&t) {
                self.rec(
                    "E-TYP-040",
                    format!("{what} has the interface type {i}, which no value has: write `dynamic {i}`, `Shared[dynamic {i}]` or a type parameter bounded by {i} (9.5)"),
                    "LANG-06-01",
                    Some(&at),
                    Self::typ_fields(&format!("dynamic {i}"), &i, &what),
                    "violated",
                );
            }
        }
        if d.module == self.module_name && matches!(d.kind.as_str(), "func" | "class" | "enum" | "interface") {
            if let Some(node) = d.node {
                for (q, n, qtok) in self.shadowed_qualified_interfaces(node) {
                    self.rec("E-TYP-040", format!("`{q}` is the interface {n}, which no value has: write `dynamic {q}`, `Shared[dynamic {q}]` or a type parameter bounded by it (9.5)"), "LANG-06-01", Some(&qtok), Self::typ_fields(&format!("dynamic {q}"), &n, &q), "violated");
                }
            }
        }
    }

    /// Whether `derive Default` can build a field of this type that has no initialiser (6.10; D-242): a type with a
    /// default that every backend builds. A type parameter has none, since a class has no `where` clause to require
    /// one; nor have the prelude's `Array`, `FixedList`, `FixedMap` and `OrderedMap`, known by their declaration and
    /// not their spelling, so a user's class named `Array` is judged as itself. A newtype is unfolded to its
    /// representation, at any depth. Elsewhere `implements` keeps treating a type parameter as implementing every
    /// interface.
    fn derivable_default(&self, t: &Type) -> bool {
        self.derivable_default_seen(t, &mut Vec::new(), None)
    }

    fn derivable_default_seen(&self, t: &Type, seen: &mut Vec<(String, String)>, home: Option<&Table<'a>>) -> bool {
        // the prelude's containers whose default no backend builds yet (6.10; D-242, X-345)
        const NO_DEFAULT_CONTAINERS: &[&str] = &["Array", "FixedList", "FixedMap", "OrderedMap"];
        match t {
            Type::TypeParam(_) => return false,
            Type::Refined { base, lo, hi, .. } => {
                if !(lo.map_or(true, |l| l <= 0) && hi.map_or(true, |h| h >= 0)) {
                    return false;
                }
                return self.derivable_default_seen(&base.as_type(), seen, home);
            }
            Type::Named(n) => {
                if let Some(d) = self.resolve_named(n, home).filter(|d| d.kind != "unknown") {
                    let key = Self::fact_key(n, &d);
                    if seen.contains(&key) {
                        return false;
                    }
                    let prelude = d.module == "fortis.prelude" && d.type_identity().is_empty();
                    if prelude && NO_DEFAULT_CONTAINERS.contains(&d.name.as_str()) {
                        return false;
                    }
                    if d.kind == "type" {
                        let Some(alias) = d.alias.clone() else { return false };
                        seen.push(key);
                        let next = self.next_home(&d, home);
                        let r = self.derivable_default_seen(&alias, seen, next.get());
                        seen.pop();
                        return r;
                    }
                    if !prelude && (COPYABLE_CONTAINERS.contains(&n.name.as_str()) || builtin_implements(&n.name, "Default")) {
                        // a declaration spelled as a built-in type is judged by what it declares (a user's `class Array`)
                        return d.implements.iter().any(|i| i.interface_key() == "Default") || d.derives.iter().any(|(x, _)| x == "Default") || self.block_implements(n, "Default");
                    }
                }
            }
            _ => {}
        }
        self.implements_seen(t, "Default", &mut Vec::new(), home)
    }

    /// The declarations a derived default would build again before it finished (6.10; D-335, D-336), or None. `path`
    /// holds the classes and newtypes whose defaults are being built, each as ((module, name), name): a newtype is
    /// unfolded to its representation and a class that derives Default to its fields without an initialiser. Reaching
    /// one already on the path is the cycle, named from that declaration back to itself. `T?` defaults to null, and a
    /// class that implements Default gives its own `default()`, so either ends the walk.
    fn default_cycle(&self, t: &Type, path: &mut Vec<((String, String), String)>, home: Option<&Table<'a>>) -> Option<Vec<String>> {
        let n = match t {
            Type::Refined { base, .. } => return self.default_cycle(&base.as_type(), path, home),
            Type::Named(n) => n,
            _ => return None,
        };
        let d = self.resolve_named(n, home).filter(|d| d.kind != "unknown")?;
        if d.module == "fortis.prelude" && d.type_identity().is_empty() {
            return None;
        }
        let key = (d.module.clone(), d.name.clone());
        if let Some(i) = path.iter().position(|(k, _)| *k == key) {
            let mut cycle: Vec<String> = path[i..].iter().map(|(_, n)| n.clone()).collect();
            cycle.push(d.name.clone());
            return Some(cycle);
        }
        let next = self.next_home(&d, home);
        path.push((key, d.name.clone()));
        let mut found = None;
        if d.kind == "type" {
            if let Some(alias) = d.alias.clone() {
                found = self.default_cycle(&alias, path, next.get());
            }
        } else if d.kind == "class" && d.derives.iter().any(|(x, _)| x == "Default") {
            for (ft, _, has_default) in d.fields.values() {
                if !has_default {
                    found = self.default_cycle(ft, path, next.get());
                    if found.is_some() {
                        break;
                    }
                }
            }
        }
        path.pop();
        found
    }

    fn check_type_decl(&mut self, d: &DeclRef<'a>) {
        let tok = tok_of(d);
        let self_t = named_of_decl(d, d.type_params.iter().map(|p| Type::TypeParam(p.clone())).collect());
        if d.trivial {
            for (fname, (ft, _, _)) in d.fields.iter() {
                if !self.is_trivial(ft) {
                    self.rec(
                        "E-OWN-030",
                        format!("trivial class {} has non-trivial field `{fname}: {}`", d.name, ft.show()),
                        "LANG-06-07",
                        Some(&tok),
                        Json::obj([("place", Json::str(fname)), ("conflict", Json::str("trivial")), ("spans", Json::str(fname)), ("loan_region", Json::str(""))]),
                        "violated",
                    );
                }
            }
        }
        let derived: BTreeSet<String> = d.derives.iter().map(|(n, _)| n.clone()).collect();
        let mixf = |what: &str, actual: &str, span: &str| Self::typ_fields(what, actual, span);
        for (iface, args) in &d.derives {
            if !DERIVABLE.contains(&iface.as_str()) {
                self.rec("E-TYP-090", format!("`{iface}` cannot be derived"), "LANG-06-07", Some(&tok), Self::typ_fields("a derivable interface", iface, &d.name), "violated");
                continue;
            }
            if d.implements_named(iface) {
                self.rec("E-MIX-001", format!("{} both derives and implements `{iface}`", d.name), "LANG-09-04", Some(&tok), mixf("derive or implement", "both", iface), "violated");
            }
            let mut hands: Vec<&str> = builtin_members(iface).iter().copied().filter(|m| d.methods.contains(m)).collect();
            hands.sort();
            for hand in hands {
                self.rec("E-MIX-001", format!("{} derives {iface} and also declares `{hand}`, which the derivation would generate", d.name), "LANG-09-04", Some(&tok), mixf("one source of the member", "derived and declared", hand), "violated");
            }
            if iface == "From" || iface == "convert" {
                if args.is_empty() {
                    self.rec("E-TYP-090", "derive From needs a source type".into(), "LANG-06-07", Some(&tok), Self::typ_fields("From[S]", "From", &d.name), "violated");
                } else if iface == "convert" && d.kind == "enum" {
                    let names: BTreeSet<String> = d.variants.iter().map(|(n, _)| n.to_ascii_lowercase()).collect();
                    if args[0] != string() || !self.is_core_type(&args[0]) || !d.type_params.is_empty()
                        || d.variants.iter().any(|(_, fields)| !fields.is_empty()) || names.len() != d.variants.len() {
                        self.rec("E-TYP-090", "enum text derivation needs String, a non-generic payload-free enum and names distinct ignoring ASCII case".into(), "LANG-06-07", Some(&tok), Self::typ_fields("a non-generic payload-free enum with distinct ASCII names", &d.name, &d.name), "violated");
                    }
                } else {
                    self.check_derive_from(d, &args[0], &tok);
                }
                continue;
            }
            if iface == "Default" {
                if d.kind == "enum" {
                    // an enum has no fields to build its default from, and no variant is declared the default (6.10; D-242)
                    self.rec(
                        "E-TYP-090",
                        format!("derive Default: enum {} has no default to derive, since nothing chooses its variant; implement Default and return the variant", d.name),
                        "LANG-06-07",
                        Some(&tok),
                        Self::typ_fields("a class whose fields have defaults", "an enum", &d.name),
                        "violated",
                    );
                    continue;
                }
                for (fname, (ft, _, has_default)) in d.fields.iter() {
                    let cycle = if *has_default { None } else { self.default_cycle(ft, &mut vec![((d.module.clone(), d.name.clone()), d.name.clone())], None) };
                    if let Some(cycle) = cycle {
                        // a default that holds itself has no end: no backend could build it (6.10; D-335, D-336)
                        let chain = cycle.join(" -> ");
                        self.rec(
                            "E-TYP-090",
                            format!("derive Default: field `{fname}: {}` builds the default of {} again before it is finished ({chain}); give the field an initialiser or an optional type", ft.show(), cycle[0]),
                            "LANG-06-07",
                            Some(&tok),
                            Self::typ_fields("a default that does not contain itself", &chain, fname),
                            "violated",
                        );
                    } else if !has_default && !self.derivable_default(ft) {
                        self.rec(
                            "E-TYP-090",
                            format!("derive Default: field `{fname}: {}` has no initialiser and its type has no Default", ft.show()),
                            "LANG-06-07",
                            Some(&tok),
                            Self::typ_fields("an initialiser or a Default type", &ft.show(), fname),
                            "violated",
                        );
                    }
                }
                continue;
            }
            let needed: Vec<&str> = if iface == "Codec" { vec!["Encodable", "Decodable"] } else { vec![iface.as_str()] };
            for need in needed {
                if need == "Encodable" || need == "Decodable" {
                    for (fname, (ft, exported, _)) in d.fields.iter() {
                        if !exported {
                            self.rec("E-TYP-090", format!("derive {need}: field `{fname}` is not exported"), "LANG-06-07", Some(&tok), Self::typ_fields("every field exported", &format!("private {fname}"), fname), "violated");
                        } else if !self.encodable(ft) {
                            let displays = diagnostic_types(&[&ft, &named(need)]);
                            self.rec("E-TYP-090", format!("derive {expected}: field `{fname}: {}` is not {expected}", displays[0], expected = displays[1]), "LANG-06-07", Some(&tok), Self::typ_fields(&displays[1], &displays[0], fname), "violated");
                        }
                    }
                    for (vname, payload) in d.variants.iter() {
                        for (fname, ft) in payload {
                            if !self.encodable(ft) {
                                let displays = diagnostic_types(&[&ft, &named(need)]);
                                self.rec("E-TYP-090", format!("derive {expected}: payload `{vname}.{fname}: {}` is not {expected}", displays[0], expected = displays[1]), "LANG-06-07", Some(&tok), Self::typ_fields(&displays[1], &displays[0], fname), "violated");
                            }
                        }
                    }
                } else {
                    let mut all: Vec<(String, Type)> = d.fields.iter().map(|(f, (t, _, _))| (f.clone(), t.clone())).collect();
                    for (v, p) in d.variants.iter() {
                        for (f, t) in p {
                            all.push((format!("{v}.{f}"), t.clone()));
                        }
                    }
                    for (fname, ft) in all {
                        if !self.implements(&ft, need) {
                            let displays = diagnostic_types(&[&ft, &named(need)]);
                            self.rec("E-TYP-090", format!("derive {expected}: field `{fname}: {}` does not implement {expected}", displays[0], expected = displays[1]), "LANG-06-07", Some(&tok), Self::typ_fields(&displays[1], &displays[0], &fname), "violated");
                        }
                    }
                    for req in iface_requires(need) {
                        if !(derived.contains(*req) || self.implements(&self_t, req)) {
                            self.rec("E-TYP-080", format!("derive {need} requires {req}, which {} neither derives nor implements", d.name), "LANG-09-03", Some(&tok), Self::typ_fields(req, &d.name, &d.name), "violated");
                        }
                    }
                }
            }
        }
        for iface in d.implements.clone() {
            // a mixin's method may satisfy an interface member (9.6): the class's own methods first, then its mixins'
            let mut members: OMap<Sig<'a>> = d.methods.clone();
            for mx in &d.mixins {
                if let Some(md) = self.resolve_named(mx, None).filter(|md| md.kind == "mixin") {
                    for (mname, msig) in md.methods.iter() {
                        if !mname.starts_with("requires:") && !members.contains(mname) {
                            members.insert(mname, msig.clone());
                        }
                    }
                }
            }
            self.check_implements(d, &iface, &members, &tok);
        }
        let mut provided: BTreeMap<String, String> = BTreeMap::new();
        for mx in &d.mixins {
            let md = match self.resolve_named(mx, None) {
                Some(md) if md.kind == "mixin" => md,
                _ => {
                    self.rec("E-TYP-080", format!("`{}` is not a mixin", mx.name), "LANG-09-04", Some(&tok), Self::typ_fields("a mixin", &mx.name, &d.name), "violated");
                    continue;
                }
            };
            for req in &md.requires {
                let mname = req.name.strip_prefix("__method__:").unwrap_or(&req.name).to_string();
                let Some(have) = d.methods.get(&mname).cloned() else {
                    if !self.interface_members(d).contains(&mname) {
                        self.rec("E-TYP-080", format!("mixin {} requires method `{mname}`, which {} lacks", mx.name, d.name), "LANG-09-04", Some(&tok), Self::typ_fields(&mname, "absent", &d.name), "violated");
                    }
                    continue;
                };
                // a requirement is a signature, compared as an interface member's is (9.6)
                if let Some(rsig) = md.methods.get(&format!("requires:{mname}")).cloned() {
                    for p in self.compatible(&rsig, &have, mx, d) {
                        let displays = diagnostic_signatures(&[&rsig, &have]);
                        self.rec("E-TYP-080", format!("{}.{mname} does not satisfy mixin {}'s requirement `{}`: {p}", d.name, mx.name, displays[0]), "LANG-09-04", Some(&tok), Self::typ_fields(&displays[0], &displays[1], &mname), "violated");
                    }
                }
            }
            for mname in md.methods.keys() {
                if mname.starts_with("requires:") || d.methods.contains(mname) {
                    continue;
                }
                if let Some(prev) = provided.get(mname) {
                    self.rec("E-MIX-001", format!("mixins {prev} and {} both provide `{mname}` and {} declares no `{mname}`", mx.name, d.name), "LANG-09-04", Some(&tok), mixf("one source of the member", "two mixins", mname), "violated");
                }
                provided.insert(mname.clone(), mx.name.clone());
            }
        }
    }

    pub fn interface_members(&self, d: &Decl<'a>) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        for (iface, _) in &d.derives {
            out.extend(builtin_members(iface).iter().map(|s| s.to_string()));
            if let Some(id) = self.interface_decl(iface) {
                out.extend(id.methods.keys().cloned());
                out.extend(id.constants.keys().cloned());
            }
        }
        for iface in &d.implements {
            out.extend(builtin_members(&iface.interface_key()).iter().map(|s| s.to_string()));
        }
        out
    }

    pub fn encodable(&self, t: &Type) -> bool {
        if let Type::Named(n) = t {
            if n.package.is_empty() && NOT_ENCODABLE.contains(&n.name.as_str()) {
                return false;
            }
            if let Some(d) = self.resolve_named(n, None) {
                if d.kind == "class" && self.implements_core_authority(&d) {
                    return false;
                }
            }
        }
        if matches!(t, Type::Loan { .. } | Type::Fn { .. } | Type::Dynamic(_)) {
            return false;
        }
        self.implements(t, "Encodable")
    }

    fn check_derive_from(&mut self, d: &DeclRef<'a>, source: &Type, tok: &Token) {
        if *source == string() && self.is_core_type(source) {
            self.rec("E-TYP-090", "derive convert from String requires a payload-free enum".into(), "LANG-06-07", Some(tok), Self::typ_fields("an enum", &d.kind, &d.name), "violated");
            return;
        }
        if d.kind != "class" {
            self.rec("E-TYP-090", "derive convert from requires a class target".into(), "LANG-06-07", Some(tok), Self::typ_fields("a class", &d.kind, &d.name), "violated");
            return;
        }
        let sd = self.resolve_type(source, None);
        let sd = match sd {
            Some(sd) if sd.kind == "class" => sd,
            other => {
                let unknown_source = source.is_unknown() || (other.is_none() && matches!(source, Type::Named(_)));
                if unknown_source {
                    return;
                }
                self.rec("E-TYP-090", format!("derive From[{}]: the source is not a class", source.show()), "LANG-06-07", Some(tok), Self::typ_fields("a class", &source.show(), &d.name), "violated");
                return;
            }
        };
        for (fname, (ft, _, _)) in d.fields.iter() {
            let (st, exp, _) = match sd.fields.get(fname) {
                Some(v) => v.clone(),
                None => {
                    self.rec("E-TYP-090", format!("derive From[{}]: {} has no field `{fname}`", source.show(), source.show()), "LANG-06-07", Some(tok), Self::typ_fields(fname, "absent", fname), "violated");
                    continue;
                }
            };
            let st = self.subst(&st, &self.type_substitution(&sd, source));
            if !exp && sd.module != d.module {
                self.rec("E-TYP-090", format!("derive From[{}]: field `{fname}` of the source is not visible", source.show()), "LANG-06-07", Some(tok), Self::typ_fields("an exported field", fname, fname), "violated");
            }
            let same = st == *ft || st.is_unknown() || ft.is_unknown();
            if !(same || self.total_conversion(&st, ft, &d.module)) {
                let displays = diagnostic_types(&[source, &st, ft]);
                self.rec("E-TYP-090", format!("derive From[{}]: field `{fname}` has type {}, needs {}", displays[0], displays[1], displays[2]), "LANG-06-07", Some(tok), Self::typ_fields(&displays[2], &displays[1], fname), "violated");
            }
            let saved_bounds = self.tparam_bounds.clone();
            self.tparam_bounds = d.type_params.iter().map(|p| (p.clone(), d.where_.iter().filter(|(q, _)| q == p).map(|(_, b)| b.interface_key()).collect())).collect();
            let copyable = self.is_copyable(&st);
            self.tparam_bounds = saved_bounds;
            if !copyable {
                self.rec("E-TYP-090", format!("derive From[{}]: field `{fname}: {}` is not Copyable", source.show(), st.show()), "LANG-06-07", Some(tok), Self::typ_fields("Copyable", &st.show(), fname), "violated");
            }
        }
    }

    pub fn check_implements(&mut self, d: &Decl<'a>, iface: &NamedT, methods: &OMap<Sig<'a>>, tok: &Token) {
        let idecl = match self.resolve_named(iface, None) {
            Some(i) => i.clone(),
            None => {
                if ["Authority", "Copyable", "Shareable", "Default", "Equatable", "Hashable", "Comparable", "Displayable", "Encodable", "Decodable", "Codec", "Abandonable", "From"].contains(&iface.name.as_str()) {
                    return;
                }
                self.rec("E-TYP-080", format!("`{}` is not an interface", iface.name), "LANG-09-03", Some(tok), Self::typ_fields("an interface", &iface.name, &d.name), "unknown");
                return;
            }
        };
        if idecl.kind != "interface" {
            self.rec("E-TYP-080", format!("`{}` is not an interface", iface.name), "LANG-09-03", Some(tok), Self::typ_fields("an interface", &idecl.kind, &d.name), "violated");
            return;
        }
        let self_t = named_of_decl(d, d.type_params.iter().map(|p| Type::TypeParam(p.clone())).collect());
        for (mname, isig) in idecl.methods.iter() {
            let have = match methods.get(mname) {
                Some(h) => h.clone(),
                None => {
                    self.rec("E-TYP-080", format!("{} implements {} but lacks `{}`", d.name, iface.name, isig.show()), "LANG-09-03", Some(tok), Self::typ_fields(&isig.show(), "absent", mname), "violated");
                    continue;
                }
            };
            for p in self.compatible(isig, &have, iface, d) {
                let displays = diagnostic_signatures(&[isig, &have]);
                self.rec("E-TYP-080", format!("{}.{mname} does not satisfy {}.{mname}: {p}", d.name, iface.name), "LANG-09-03", Some(tok), Self::typ_fields(&displays[0], &displays[1], mname), "violated");
            }
        }
        for cname in idecl.constants.keys() {
            if !d.constants.contains(cname) {
                self.rec("E-TYP-080", format!("{} implements {} but lacks constant `{cname}`", d.name, iface.name), "LANG-09-03", Some(tok), Self::typ_fields(cname, "absent", cname), "violated");
            }
        }
        let isub: BTreeMap<String, Type> = idecl.type_params.iter().cloned().zip(iface.args.iter().cloned()).collect();
        for req in &idecl.requires {
            let want = match self.subst(&Type::Named(req.clone()), &isub) {
                Type::Named(n) => n,
                _ => req.clone(),
            };
            if !self.requirement_met(d, &self_t, &idecl, &want) {
                self.rec(
                    "E-TYP-080",
                    format!("{} implements {}, which requires {}; {} does not implement it", d.name, iface.show(), want.show(), d.name),
                    "LANG-09-03",
                    Some(tok),
                    Self::typ_fields(&want.show(), &d.name, &d.name),
                    "violated",
                );
            }
        }
        // the interface's own `where` holds for the arguments the implementation gives it (9.5; D-69)
        for (tp, bound) in idecl.where_.iter() {
            let Some(arg) = isub.get(tp) else { continue };
            match arg {
                Type::Unknown(_) => {}
                Type::TypeParam(p) => {
                    if !d.where_.iter().any(|(dp, b)| dp == p && b == bound) {
                        let displays = diagnostic_types(&[&Type::Named(iface.clone()), &Type::Named(bound.clone()), arg]);
                        self.rec("E-TYP-080", format!("{} implements {}, whose `where` needs `{tp}` to implement {}; {}'s own `where` does not grant it for {}", d.name, displays[0], displays[1], d.name, displays[2]), "LANG-09-03", Some(tok), Self::typ_fields(&displays[1], &displays[2], &d.name), "violated");
                    }
                }
                _ => {
                    if !self.implements(arg, &bound.interface_key()) {
                        let displays = diagnostic_types(&[&Type::Named(iface.clone()), &Type::Named(bound.clone()), arg]);
                        self.rec("E-TYP-080", format!("{} implements {}, whose `where` needs `{tp}` to implement {}; {} does not (9.5)", d.name, displays[0], displays[1], displays[2]), "LANG-09-03", Some(tok), Self::typ_fields(&displays[1], &displays[2], &d.name), "violated");
                    }
                }
            }
        }
    }

    /// The declaration `name` reaches in `module`'s own table: this table for this module, the loaded table of a
    /// package module, and this table for a standard module loaded into it.
    pub fn decl_in_module(&self, module: &str, name: &str) -> Option<DeclRef<'a>> {
        let d = if module == self.module_name {
            self.table.get(name).cloned()
        } else {
            let loaded = self.sess.loaded.borrow();
            let found = loaded.iter().find(|((_, p), _)| p == module).and_then(|(_, t)| t.get(name).cloned());
            found.or_else(|| self.table.get(name).cloned())
        };
        d.filter(|d| d.kind != "unknown")
    }

    /// Does `d` implement what `idecl` requires (9.5): the declaration the requirement names as the interface's own
    /// module sees it, with the type arguments the requirement states (D-67; 4.4 makes other modules' and other
    /// instantiations' interfaces distinct). A core, structural or bundle requirement, or one met by a derivation or an
    /// `implement` block, is judged by name.
    fn requirement_met(&self, d: &Decl<'a>, self_t: &Type, _idecl: &Decl<'a>, want: &NamedT) -> bool {
        if !self.implements(self_t, &want.interface_key()) {
            return false;
        }
        let Some(rd) = self.resolve_named(want, None) else { return true };
        if rd.kind != "interface" || is_standard(&rd.module) {
            return true;
        }
        let listed: Vec<&NamedT> = d.implements.iter().filter(|i| i.interface_key() == want.interface_key()).collect();
        if listed.is_empty() {
            return true;
        }
        for i in listed {
            if let Some(ld) = self.resolve_named(i, None) {
                if (ld.module.as_str(), ld.name.as_str()) != (rd.module.as_str(), rd.name.as_str()) {
                    continue;
                }
            }
            if !want.args.is_empty() && i.args != want.args {
                continue;
            }
            return true;
        }
        false
    }

    /// A signature's claims (11.2, 11.5): its modifiers with those its contracts expand to, the effects it rules out (a
    /// `pure` or `noalloc` claim, a contract's `forbids`, and each of `suspend`, `fail`, `recovers` its modifiers do not
    /// permit), and the authorities its contracts exclude.
    /// A contract named in a signature, as its declaring module names it, whatever this module imports (review of
    /// D-61): this table, then the declaring package module's own table.
    pub fn contract_decl(&self, c: &str, module: Option<&str>) -> Option<DeclRef<'a>> {
        let d = self.table.get(c).filter(|d| d.kind == "contract").cloned();
        if let Some(d) = &d {
            if module.map_or(true, |m| d.module == m) {
                return Some(d.clone());
            }
        }
        if let Some(m) = module {
            if let Some(x) = self.table.decls.values().find(|v| v.kind == "contract" && v.name == c && v.module == m) {
                return Some(x.clone());
            }
            for ((_, path), t) in self.sess.loaded.borrow().iter() {
                if path == m {
                    if let Some(y) = t.get(c) {
                        if y.kind == "contract" {
                            return Some(y.clone());
                        }
                    }
                }
            }
        }
        d
    }

    pub fn claims(&self, sig: &Sig<'a>, module: Option<&str>) -> (BTreeSet<String>, BTreeSet<String>, BTreeSet<String>) {
        let mut mods: BTreeSet<String> = sig.modifiers.iter().cloned().collect();
        let (mut forbids, mut excludes) = (BTreeSet::new(), BTreeSet::new());
        for c in &sig.contracts {
            if let Some(cd) = self.contract_decl(c, module) {
                if cd.kind == "contract" {
                    for (k, out) in [("modifiers", &mut mods), ("forbids", &mut forbids), ("excludes", &mut excludes)] {
                        if let Some(v) = cd.items.get(k) {
                            out.extend(v.iter().cloned());
                        }
                    }
                }
            }
        }
        if mods.contains("pure") {
            forbids.extend(crate::effects::PURE_FORBIDS.iter().map(|e| e.to_string()));
        }
        if mods.contains("noalloc") {
            forbids.insert("alloc".into());
        }
        for (effect, m) in [("suspend", "async"), ("fail", "try"), ("recovers", "recovers")] {
            if !mods.contains(m) {
                forbids.insert(effect.into());
            }
        }
        (mods, forbids, excludes)
    }

    /// An implementation may not strengthen a precondition or weaken a guarantee (9.5). Every claim of the interface
    /// method binds it (11.2), written or through a contract: a caller through the interface or `dynamic` relies on
    /// exactly those (D-61).
    fn compatible(&self, isig: &Sig<'a>, have: &Sig<'a>, iface: &NamedT, d: &Decl<'a>) -> Vec<String> {
        let mut problems = Vec::new();
        if isig.receiver.clone().unwrap_or_default() != have.receiver.clone().unwrap_or_default() {
            problems.push(format!("receiver `{}` differs from `{}`", have.receiver.clone().unwrap_or("None".into()), isig.receiver.clone().unwrap_or("None".into())));
        }
        let imodule = self.resolve_named(iface, None).map(|i| i.module.clone());
        let (imods, iforbids, iexcludes) = self.claims(isig, imodule.as_deref());
        let (hmods, hforbids, hexcludes) = self.claims(have, Some(d.module.as_str()));
        let mut explained: BTreeSet<String> = BTreeSet::new();
        for (claim, effects) in [("pure", crate::effects::PURE_FORBIDS), ("noalloc", &["alloc"][..])] {
            if imods.contains(claim) && !hmods.contains(claim) {
                problems.push(format!("drops `{claim}`"));
                explained.extend(effects.iter().map(|e| e.to_string()));
            }
        }
        for (weak, effect) in [("try", "fail"), ("async", "suspend"), ("recovers", "recovers")] {
            if hmods.contains(weak) && !imods.contains(weak) {
                problems.push(format!("adds `{weak}`"));
                explained.insert(effect.into());
            }
        }
        for effect in iforbids.difference(&hforbids).filter(|e| !explained.contains(*e)) {
            problems.push(format!("does not rule out `{effect}`, which the interface's contract forbids"));
        }
        for a in iexcludes.difference(&hexcludes) {
            problems.push(format!("does not exclude capability {a}, which the interface's contract excludes"));
        }
        if isig.params.len() != have.params.len() {
            problems.push("parameter count differs".into());
            return problems;
        }
        let mut subst: BTreeMap<String, Type> = BTreeMap::new();
        subst.insert("Self".into(), named_of_decl(d, d.type_params.iter().map(|p| Type::TypeParam(p.clone())).collect()));
        if let Some(idecl) = self.resolve_named(iface, None) {
            for (p, a) in idecl.type_params.iter().zip(iface.args.iter()) {
                subst.insert(p.clone(), a.clone());
            }
        }
        for ((_, pt, pm), (hn, ht, hm)) in isig.params.iter().zip(have.params.iter()) {
            let pt2 = self.subst(pt, &subst);
            if pm != hm {
                problems.push(format!("parameter `{hn}` mode `{hm}` differs from `{pm}`"));
            }
            if !param_compatible(&pt2, ht) {
                let displays = diagnostic_types(&[&pt2, ht]);
                problems.push(format!("parameter `{hn}` narrows {} to {}", displays[0], displays[1]));
            }
        }
        let rt = self.subst(&isig.ret, &subst);
        if !return_compatible(&rt, &have.ret) {
            let displays = diagnostic_types(&[&have.ret, &rt]);
            problems.push(format!("return {} widens {}", displays[0], displays[1]));
        }
        problems
    }

    pub fn subst(&self, t: &Type, m: &BTreeMap<String, Type>) -> Type {
        match t {
            Type::Named(n) => {
                if n.args.is_empty() {
                    if let Some(r) = m.get(&n.name) {
                        return r.clone();
                    }
                }
                Type::Named(NamedT { name: n.name.clone(), args: n.args.iter().map(|a| self.subst(a, m)).collect(), package: n.package.clone(), origin: n.origin.clone() })
            }
            Type::TypeParam(n) => m.get(n).cloned().unwrap_or_else(|| t.clone()),
            Type::Optional(i) => optional(self.subst(i, m)),
            Type::Loan { inner, mode, origin } => Type::Loan { inner: Box::new(self.subst(inner, m)), mode: mode.clone(), origin: origin.clone() },
            Type::Tuple(items) => Type::Tuple(items.iter().map(|x| self.subst(x, m)).collect()),
            Type::Fn { params, ret, kind, modifiers, origin, dynamic, identity, capture_origins, param_modes } => {
                Type::Fn { params: params.iter().map(|x| self.subst(x, m)).collect(), ret: Box::new(self.subst(ret, m)), kind: kind.clone(), modifiers: modifiers.clone(), origin: origin.clone(), dynamic: *dynamic, identity: identity.clone(), capture_origins: capture_origins.clone(), param_modes: param_modes.clone() }
            }
            Type::Record(fs) => Type::Record(fs.iter().map(|(k, x)| (k.clone(), self.subst(x, m))).collect()),
            _ => t.clone(),
        }
    }

    fn check_interface(&mut self, d: &DeclRef<'a>) {
        let tok = tok_of(d);
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let path = vec![d.name.clone()];
        self.visit_requires(d, &named_of_decl(d, vec![]).named_ref().unwrap().interface_key(), path, &mut seen, &tok);
    }

    fn visit_requires(&mut self, d: &DeclRef<'a>, n: &str, path: Vec<String>, seen: &mut BTreeSet<String>, tok: &Token) -> bool {
        let idecl = match self.interface_decl(n) {
            Some(i) if i.kind == "interface" => i.clone(),
            _ => return false,
        };
        for r in &idecl.requires {
            if r.interface_key() == named_of_decl(d, vec![]).named_ref().unwrap().interface_key() {
                let mut p = path.clone();
                p.push(r.name.clone());
                self.rec(
                    "E-TYP-060",
                    format!("interface requirement cycle: {}", p.join(" requires ")),
                    "LANG-09-03",
                    Some(tok),
                    Self::typ_fields("an acyclic requirement graph", &p.join(" -> "), &d.name),
                    "violated",
                );
                return true;
            }
            if !seen.contains(&r.interface_key()) {
                seen.insert(r.interface_key());
                let mut p = path.clone();
                p.push(r.name.clone());
                if self.visit_requires(d, &r.interface_key(), p, seen, tok) {
                    return true;
                }
            }
        }
        false
    }

    /// Language 1.0 (X-180): an `implement` block's target is a class or enum the program declares. Core types and
    /// newtypes have no run-time identity of their own in 1.0's backends, so a block for one could not be honoured by
    /// `==`, interpolation, the codec or dispatch through a type parameter; a later release lifts this (E-TYP-171).
    /// The name an `implement` block's target is written with (`money.Currency`, `Tag`), or None for a form that is
    /// not a plain named type.
    fn target_spelling(node: N<'a>) -> Option<String> {
        let ts = node.child_nodes("type");
        let t = ts.get(1)?;
        let nt = match t.kids().first() {
            Some(C::N(n)) if n.name() == "namedType" => *n,
            _ => return None,
        };
        let tr = match nt.kids().first() {
            Some(C::N(n)) if n.name() == "typeRef" => *n,
            _ => return None,
        };
        let dts = tr.direct_tokens();
        if dts.first()?.kind == Kind::Lower && dts.len() >= 3 {
            Some(format!("{}.{}", dts[0].text, dts[2].text))
        } else {
            Some(dts[0].text.clone())
        }
    }

    /// The declaration an `implement` block's target names, found by the block's own spelling: `money.Currency` through
    /// a module alias is that module's `Currency`, never a program class that happens to share the name (the canonical
    /// `Named` carries only the name, and a core type's identity is as empty as a single-file program's).
    /// The spelling of a block's target when it is a plain name (`Box`, `shapes.Box`): no type arguments, not optional.
    fn plain_target_spelling(node: N<'a>) -> Option<String> {
        let ts = node.child_nodes("type");
        let t = ts.get(1)?;
        let kids = t.kids();
        let nt = match kids.as_slice() {
            [C::N(n)] if n.name() == "namedType" => *n,
            _ => return None,
        };
        let nk = nt.kids();
        match nk.as_slice() {
            [C::N(tr)] if tr.name() == "typeRef" && tr.child_node("typeArgs").is_none() => Self::target_spelling(node),
            _ => None,
        }
    }

    fn implement_target_decl(&self, node: N<'a>, target: &Type) -> Option<DeclRef<'a>> {
        Self::target_decl_in(&self.table, node, target)
    }

    /// The declaration a block's target names, resolved in a module's own table (its aliases).
    fn target_decl_in(table: &Table<'a>, node: N<'a>, target: &Type) -> Option<DeclRef<'a>> {
        if let Some(sp) = Self::target_spelling(node) {
            if let Some(d) = table.get(&sp) {
                if d.kind != "unknown" {
                    return Some(d.clone());
                }
            }
        }
        match target {
            Type::Named(n) => table.get(&n.name).cloned(),
            _ => None,
        }
    }

    /// The `implement` blocks of this package's other modules (read from their trees: a module loaded as a library
    /// keeps no blocks), each with the declaration its plainly named target resolves to in its own module (aliases are
    /// per module), so a clash between modules that do not import each other is seen.
    fn sibling_block_entries(&self) -> Vec<(String, N<'a>, Option<DeclRef<'a>>)> {
        let mut out = Vec::new();
        if self.package.is_empty() {
            return out;
        }
        let lib = self.library();
        let mut mods: Vec<(&'a String, &'a LibraryModule)> = lib.modules.iter().map(|(k, v)| (k, &**v)).collect();
        mods.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then(a.0.cmp(b.0)));
        let mut seen: BTreeSet<usize> = BTreeSet::new();
        for (key, lm) in mods {
            if !seen.insert(lm as *const LibraryModule as usize) {
                continue;
            }
            if lm.library.is_none() || lm.package != self.package || *key == self.module_name {
                continue;
            }
            let sub = self.loaded(lm, key);
            for dd in declarations(lm.tree()) {
                if dd.kind != "implement" {
                    continue;
                }
                let d = Self::plain_target_spelling(dd.node).and_then(|sp| sub.get(&sp).cloned()).filter(|d| d.kind != "unknown");
                out.push((key.clone(), dd.node, d));
            }
        }
        out
    }

    /// The methods the mixins of a class provide (with bodies), with the providing mixin, as the class's module sees
    /// them.
    fn mixin_provided(&self, tdecl: &Decl<'a>) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let mut home: Option<Option<Rc<Table<'a>>>> = None;
        for mx in &tdecl.mixins {
            let mut md = self.resolve_named(mx, None).filter(|d| d.kind == "mixin");
            if md.is_none() && tdecl.module != self.module_name {
                if home.is_none() {
                    let lib = self.library();
                    home = Some(match lib.modules.get(&tdecl.module) {
                        Some(lm) if lm.library.is_some() => {
                            let lm: &'a LibraryModule = lm;
                            Some(self.loaded(lm, &tdecl.module))
                        }
                        _ => None,
                    });
                }
                md = home.as_ref().unwrap().as_ref().and_then(|t| t.get(&mx.name).cloned()).filter(|d| d.kind == "mixin");
            }
            let Some(md) = md else { continue };
            for (mname, _) in md.methods.iter() {
                if !mname.starts_with("requires:") {
                    out.push((mname.clone(), mx.name.clone()));
                }
            }
        }
        out
    }

    /// Why a declaration cannot be an `implement` block's target in Language 1.0 (X-180), or None when it can: the
    /// target is a non-generic class or enum the program declares. The two extensions of the user's option B, a
    /// newtype and a generic class or enum, stand by the user's decision of 29 Oct 2026; the newtype exclusion may be
    /// relaxed for a block inside the declaring module if a newtype comes to expose its representation there (Q2), so
    /// the whole test lives here, in one place.
    fn implement_target_excluded(tdecl: &Decl<'a>) -> Option<String> {
        if tdecl.module == "fortis" || tdecl.module.starts_with("fortis.") {
            return Some("a core library type".to_string());
        }
        if tdecl.kind == "class" || tdecl.kind == "enum" {
            return if tdecl.type_params.is_empty() { None } else { Some(format!("a generic {}", tdecl.kind)) };
        }
        Some(if tdecl.kind == "type" { "a newtype".to_string() } else { format!("a declaration of kind `{}`", tdecl.kind) })
    }

    fn check_implement_targets(&mut self) {
        let mut blocks: Vec<(Type, String, N<'a>)> = self.table.implement_blocks.iter().map(|(_, t, m, n)| (Type::Named(t.clone()), m.clone(), *n)).collect();
        blocks.extend(self.table.other_implement_targets.iter().cloned());
        blocks.retain(|b| b.1 == self.module_name);
        blocks.sort_by_key(|b| {
            let t = &b.2.tokens()[0];
            (t.line, t.col)
        });
        let mut found: Vec<(usize, usize, &'static str, String, &'static str, Token, Json)> = Vec::new();
        let mut targets: Vec<(String, DeclRef<'a>)> = Vec::new();
        for (target, _, node) in blocks {
            let ts = node.child_nodes("type");
            if ts.len() < 2 {
                continue;
            }
            let tok = ts[1].tokens()[0].clone();
            let iface = ts[0].tokens()[0].text.clone();
            let mut shown = target.show();
            if let (Type::Named(n), Some(sp)) = (&target, Self::target_spelling(node)) {
                if n.args.is_empty() && sp.contains('.') {
                    shown = sp; // a module alias's type, as the block writes it
                }
            }
            let what = match &target {
                Type::Named(_) => {
                    let Some(tdecl) = self.implement_target_decl(node, &target) else { continue };
                    if tdecl.kind == "unknown" {
                        continue;
                    }
                    match Self::implement_target_excluded(&tdecl) {
                        Some(what) => what,
                        None => {
                            let key = format!("{}.{}", tdecl.module, tdecl.name);
                            if !targets.iter().any(|(k, _)| *k == key) {
                                targets.push((key, tdecl.clone()));
                            }
                            continue;
                        }
                    }
                }
                _ => "not a class or enum".to_string(),
            };
            found.push((
                tok.line,
                tok.col,
                "E-TYP-171",
                format!(
                    "`implement {iface} for {shown}`: in Language 1.0 an `implement` block's target must be a non-generic class or enum the program declares, and `{shown}` is {what} (X-180); implementations for such targets come in a later release. Wrap the value in a class the program declares, or list the interface in its `implements`"
                ),
                "LANG-06-13",
                tok.clone(),
                Self::typ_fields("a non-generic class or enum the program declares", &shown, &shown),
            ));
        }
        // member clashes, target by target, over every block of the package for it: the target's own members and its
        // mixins' come first, then blocks in module, line and column order (the later of two clashing blocks is reported)
        if !targets.is_empty() {
            let siblings = self.sibling_block_entries();
            for (key, tdecl) in targets {
                let mut entries: Vec<(String, usize, usize, N<'a>)> = Vec::new();
                for (_, target, module, node) in self.table.implement_blocks.iter() {
                    if *module == self.module_name {
                        if let Some(d) = self.implement_target_decl(*node, &Type::Named(target.clone())) {
                            if format!("{}.{}", d.module, d.name) == key {
                                let t = &node.tokens()[0];
                                entries.push((module.clone(), t.line, t.col, *node));
                            }
                        }
                    }
                }
                for (module, node, d) in &siblings {
                    if let Some(d) = d {
                        if format!("{}.{}", d.module, d.name) == key {
                            let t = &node.tokens()[0];
                            entries.push((module.clone(), t.line, t.col, *node));
                        }
                    }
                }
                entries.sort_by(|a, b| (&a.0, a.1, a.2).cmp(&(&b.0, b.1, b.2)));
                let mut have: BTreeMap<String, (&'static str, String, String)> = BTreeMap::new();
                for m in tdecl.fields.keys().chain(tdecl.methods.keys()) {
                    have.entry(m.clone()).or_insert(("own", String::new(), String::new()));
                }
                for (m, mx) in self.mixin_provided(&tdecl) {
                    have.entry(m).or_insert(("mixin", mx, String::new()));
                }
                for (module, _, _, node) in entries {
                    let ts = node.child_nodes("type");
                    let iface = ts.first().map(|t| t.tokens()[0].text.clone()).unwrap_or_default();
                    let at = node.tokens()[0];
                    let bound = self.table.implement_blocks.iter().find(|(_, _, m, n)| *m == module && {
                        let other = n.tokens()[0]; (other.line, other.col) == (at.line, at.col)
                    }).map(|(i, _, _, _)| i);
                    if bound.map_or(false, |i| tdecl.implements.contains(i) || tdecl.derives.iter().any(|(n, _)| *n == i.interface_key())) {
                        continue; // the whole block overlaps an implementation: E-TYP-100 alone
                    }
                    let shown = match Self::target_spelling(node) {
                        Some(sp) if sp.contains('.') => sp,
                        _ => tdecl.name.clone(),
                    };
                    for mm in node.child_nodes("implementMember") {
                        let Some(head) = mm.find("methodHead") else { continue };
                        let Some(mn) = head.find("memberName") else { continue };
                        let mtok = mn.tokens()[0].clone();
                        let src = have.get(&mtok.text).cloned();
                        if let Some(src) = &src {
                            if module == self.module_name {
                                let how = match src.0 {
                                    "own" => format!("a member {shown} already has"),
                                    "mixin" => format!("a member {shown}'s mixin {} provides", src.1),
                                    _ => format!("a member that `implement {} for {shown}` also adds", src.1) + &(if src.2 != self.module_name { format!(" (module {})", src.2) } else { String::new() }),
                                };
                                found.push((
                                    mtok.line,
                                    mtok.col,
                                    "E-TYP-070",
                                    format!("member `{}` of {shown} is declared twice: `implement {iface} for {shown}` adds {how} (an `implement` block is its target listing the interface, X-180)", mtok.text),
                                    "LANG-09-01",
                                    mtok.clone(),
                                    Self::typ_fields("one member", "two", &mtok.text),
                                ));
                            }
                        }
                        if src.is_none() {
                            have.insert(mtok.text.clone(), ("block", iface.clone(), module.clone()));
                        }
                    }
                }
            }
        }
        found.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
        for (_, _, code, msg, rule, tok, fields) in found {
            self.rec(code, msg, rule, Some(&tok), fields, "violated");
        }
    }

    /// A named type's identity for a pairing rule: the declaration it names (module and name) with its type
    /// arguments as written, so `Box[Int]` and `Box[String]` are two types and two modules' `Box` are two types. The
    /// coherence of `implement` blocks keys its (interface, target) pairs by it (9.5), as a conversion pair may.
    pub fn whole_type_key(tdecl: Option<&DeclRef<'a>>, t: &NamedT) -> String {
        let base = tdecl.map_or_else(|| format!("{}.{}", if t.package.is_empty() { &t.origin } else { &t.package }, t.name), |d| {
            let identity = d.type_identity();
            format!("{}.{}", if !identity.is_empty() { &identity } else if !d.core_home.is_empty() { &d.core_home } else { &d.module }, d.name)
        });
        format!("{base}{:?}", t.args)
    }

    /// Whether a base name is a core built-in type here: no declaration of it is visible but a standard one, so a
    /// user's class of the name (`class Int`) is itself and the built-in conversion rows do not reach it (6.13; the
    /// rows are keyed by the built-in types, not by their spellings, D-516).
    pub fn is_core_type(&self, t: &Type) -> bool {
        match strip(t) {
            Type::Named(n) | Type::Refined { base: n, .. } => n.package.is_empty(),
            _ => false,
        }
    }

    /// The declaration a nominal type names, through its origin stamp where it carries one (X-136).
    pub fn named_decl(&self, t: &Type) -> Option<DeclRef<'a>> {
        if matches!(t, Type::Named(_)) { self.resolve_type(t, None) } else { None }
    }

    /// One side of a conversion pair as its whole-type key (6.13): the declaration's module and name with the type
    /// arguments as written; a non-nominal side (rejected, E-TYP-184) keys by its spelling.
    fn side_key(&self, t: &Type) -> String {
        match t {
            Type::Named(n) => Self::whole_type_key(self.named_decl(t).as_ref(), n),
            _ => t.show(),
        }
    }

    /// The declared conversions of every visible class and enum (6.13, X-551): the forms a member may take, and one
    /// conversion per pair. A pair is keyed by the whole type, as `implement` coherence keys its pairs (9.5); a
    /// member of an imported module counts for the overlap and is reported by its own module (as D-132).
    /// Explicit members and legacy From implementations resolve to the same declared pair (6.13, X-552).
    pub fn all_conversions(&self, d: &DeclRef<'a>) -> Vec<Convert<'a>> {
        let mut out = d.converts.clone();
        for c in &d.converts {
            if c.direction == "from" && self.is_core_type(&c.other) && (c.other == string() || c.other == string_view()) {
                let mut alternate = c.clone();
                alternate.other = if c.other == string() { string_view() } else { string() };
                out.push(alternate);
            }
        }
        for (iface, args) in &d.derives {
            if (iface == "From" || iface == "convert") && args.len() == 1 {
                if iface == "convert" && d.kind == "enum" && args[0] == string() && self.is_core_type(&args[0]) {
                    for source in [string(), string_view()] {
                        let mut sig = Sig::simple(&format!("derive:from:{}", source.show()), vec![("source".into(), source.clone(), String::new())], named_of_decl(d, vec![]), vec!["try".into()]);
                        sig.node = d.node;
                        sig.exported = d.exported;
                        out.push(Convert { direction: "from".into(), other: source, fallible: true, owned: false, exported: d.exported, sig, node: d.node.unwrap(), derived: true, legacy: true });
                    }
                    continue;
                }
                let source = args[0].clone();
                let qual = match &source { Type::Named(n) => if n.package.is_empty() { &n.origin } else { &n.package }, _ => "" };
                let suffix = if qual.is_empty() { String::new() } else { format!("@{qual}") };
                let mut sig = Sig::simple(&format!("derive:from:{}{suffix}", source.show()), vec![("source".into(), source.clone(), String::new())], named_of_decl(d, d.type_params.iter().map(|p| Type::TypeParam(p.clone())).collect()), vec![]);
                sig.node = d.node;
                sig.exported = d.exported;
                out.push(Convert { direction: "from".into(), other: source, fallible: false, owned: false, exported: d.exported, sig, node: d.node.unwrap(), derived: true, legacy: true });
            }
        }
        for iface in &d.implements {
            if iface.name == "From" && iface.args.len() == 1 && self.resolve_named(iface, None).map_or(false, |i| is_standard(&i.module)) {
                if let Some(sig) = d.methods.get("from") {
                    out.push(Convert { direction: "from".into(), other: iface.args[0].clone(), fallible: false, owned: false, exported: sig.exported, sig: sig.clone(), node: sig.node.unwrap(), derived: false, legacy: true });
                }
            }
        }
        for (iface, target, _, node) in &self.table.implement_blocks {
            if iface.name != "From" || iface.args.len() != 1 || !self.resolve_named(iface, None).map_or(false, |i| is_standard(&i.module)) || !self.named_decl(&target.as_type()).map_or(false, |t| Rc::ptr_eq(&t, d)) {
                continue;
            }
            for member in node.child_nodes("implementMember") {
                if let Some(head) = member.find("methodHead") {
                    if head.find("memberName").map_or(false, |n| n.tokens()[0].text == "from") {
                        let pname = head.find("paramList").and_then(|p| p.child_nodes("param").first().copied()).and_then(|p| p.direct_tokens().first().copied()).map_or("source".into(), |t| t.text.clone());
                        let modifiers = head.direct_tokens().iter().take_while(|t| t.text != "func").map(|t| t.text.clone()).collect();
                        let mut sig = Sig::simple("from", vec![(pname, iface.args[0].clone(), String::new())], named_of_decl(d, vec![]), modifiers);
                        sig.node = Some(head);
                        sig.body = member.find("funcBody");
                        out.push(Convert { direction: "from".into(), other: iface.args[0].clone(), fallible: false, owned: false, exported: true, sig, node: head, derived: false, legacy: true });
                    }
                }
            }
        }
        out
    }

    pub fn type_substitution(&self, d: &DeclRef<'a>, t: &Type) -> BTreeMap<String, Type> {
        match t { Type::Named(n) => d.type_params.iter().cloned().zip(n.args.iter().cloned()).collect(), _ => BTreeMap::new() }
    }

    /// Legacy generic implementations retain ordinary written type arguments (X-555).
    pub fn instantiated_conversions(&self, d: &DeclRef<'a>, t: &Type) -> Vec<Convert<'a>> {
        let subst = self.type_substitution(d, t);
        self.all_conversions(d).into_iter().map(|mut c| {
            c.other = self.subst(&c.other, &subst);
            c.sig.params = c.sig.params.into_iter().map(|(n, pt, mode)| (n, self.subst(&pt, &subst), mode)).collect();
            c.sig.ret = self.subst(&c.sig.ret, &subst);
            c
        }).collect()
    }

    /// The one representation step (6.9, 6.13, X-552): declaration, representation, wrapping, proved.
    /// Match the base by identity before considering its range. Never search through another newtype or a
    /// numeric conversion. The proof is the construction judgement of X-511.
    pub fn newtype_conversion(&self, src: &Type, dst: &Type) -> Option<(DeclRef<'a>, Type, bool, bool)> {
        let s = match strip(src) { Type::Loan { inner, .. } => strip(inner), t => t };
        let d = strip(dst);
        let base = |t: &Type| match t { Type::Refined { base, .. } => base.as_type(), t => t.clone() };
        if let Some(sd) = self.resolve_type(s, None) {
            if sd.kind == "type" {
                if let Some(rep) = &sd.alias {
                    let rep = strip(rep).clone();
                    if *d == rep || *d == base(&rep) {
                        return Some((sd, rep, false, true));
                    }
                }
            }
        }
        if let Some(dd) = self.resolve_type(d, None) {
            if dd.kind == "type" {
                if let Some(rep) = &dd.alias {
                    let rep = strip(rep).clone();
                    let rb = base(&rep);
                    let literal = crate::literals::is_strlit(s) && (rb == string() || rb == string_view());
                    if base(s) == rb || literal {
                        let mut proved = crate::literals::assignable(s, &rep).0 == Some(true);
                        if let (Type::Refined { base: sb, lo: sl, hi: sh, kind: sk }, Type::Refined { base: rb, lo: rl, hi: rh, kind: rk }) = (s, &rep) {
                            if sb == rb && sk == rk {
                                proved = rl.map_or(true, |l| sl.map_or(false, |x| x >= l)) && rh.map_or(true, |h| sh.map_or(false, |x| x <= h));
                            }
                        }
                        return Some((dd, rep, true, proved));
                    }
                }
            }
        }
        None
    }

    fn total_conversion(&self, source: &Type, target: &Type, module: &str) -> bool {
        if let Some((decl, rep, wrapping, proved)) = self.newtype_conversion(source, target) {
            return self.is_trivial(&rep) && proved && (wrapping || decl.module == module);
        }
        if let (Some(s), Some(t)) = (crate::conversions::base_of(source), crate::conversions::base_of(target)) {
            if self.is_core_type(source) && self.is_core_type(target) {
                if let Some(row) = crate::conversions::resolve(s, t) {
                    return !row.fallible && !matches!(target, Type::Refined { .. });
                }
            }
        }
        let source = if let Type::Refined { base, .. } = source { base.as_type() } else { source.clone() };
        let mut found = Vec::new();
        for (t, direction, other) in [(&source, "to", target), (target, "from", &source)] {
            if let Some(d) = self.resolve_type(t, None) {
                if d.kind == "class" || d.kind == "enum" {
                    found.extend(self.instantiated_conversions(&d, t).into_iter().filter(|c| c.direction == direction && c.other == *other && (c.exported || d.module == module)));
                }
            }
        }
        found.len() == 1 && !found[0].fallible && !found[0].owned
    }

    fn check_conversions(&mut self) {
        let mut seen: BTreeMap<(String, String), (String, String)> = BTreeMap::new();
        let mut visited: BTreeSet<usize> = BTreeSet::new();
        let decls: Vec<DeclRef<'a>> = self.table.decls.values().cloned().collect();
        for d in &decls {
            if d.kind != "class" && d.kind != "enum" {
                continue;
            }
            if !visited.insert(Rc::as_ptr(d) as usize) {
                continue;
            }
            for cv in self.all_conversions(&d) {
                let toks = cv.node.tokens();
                let tok = toks.first().copied();
                let local = d.module == self.module_name;
                let src = if cv.direction == "from" { cv.other.clone() } else { named_of_decl(d, vec![]) };
                let dst = if cv.direction == "from" { named_of_decl(d, vec![]) } else { cv.other.clone() };
                let key = (self.side_key(&src), self.side_key(&dst));
                if local {
                    if !d.type_params.is_empty() && !cv.legacy {
                        self.rec("E-TYP-184", format!("`convert` member of a generic {}: {} has type parameters, and a conversion is a pair of whole types (6.13, X-551)", d.kind, d.name), "LANG-06-15", tok, Self::typ_fields("a non-generic class or enum", &format!("a generic {}", d.kind), &d.name), "violated");
                    }
                    if !matches!(cv.other, Type::Named(_)) && !cv.legacy {
                        self.rec("E-TYP-184", format!("`convert {} {}`: the other type of a conversion is a nominal type, never {} (a refinement target stays the built-in `validate` row, 6.13)", cv.direction, cv.other.show(), cv.other.show()), "LANG-06-15", tok, Self::typ_fields("a nominal type", &cv.other.show(), &cv.other.show()), "violated");
                    } else if self.named_decl(&cv.other).map_or(false, |o| Rc::ptr_eq(&o, d)) || key.0 == key.1 {
                        self.rec("E-TYP-183", format!("`convert {} {}` of {}: the pair is the identity, which 6.13 already decides: a conversion converts nothing here", cv.direction, cv.other.show(), d.name), "LANG-06-15", tok, Self::typ_fields("a different type", &d.name, &cv.other.show()), "violated");
                    } else if self.newtype_conversion(&src, &dst).is_some() {
                        self.rec("E-TYP-183", format!("`convert {} {}` of {}: the pair ({}, {}) is a built-in newtype representation conversion (6.13)", cv.direction, cv.other.show(), d.name, src.show(), dst.show()), "LANG-06-15", tok, Self::typ_fields("a pair the table does not decide", &format!("{} to {}", src.show(), dst.show()), &cv.other.show()), "violated");
                    } else if let (Type::Named(s), Type::Named(t2)) = (&src, &dst) {
                        if s.args.is_empty() && t2.args.is_empty() && self.is_core_type(&src) && self.is_core_type(&dst)
                            && (crate::conversions::same_builtin(&s.name, &t2.name) || crate::conversions::resolve(&s.name, &t2.name).is_some()) {
                            self.rec("E-TYP-183", format!("`convert {} {}` of {}: the pair ({}, {}) is a row of the built-in table, which needs no declaration (6.13)", cv.direction, cv.other.show(), d.name, src.show(), dst.show()), "LANG-06-15", tok, Self::typ_fields("a pair the table does not decide", &format!("{} to {}", src.show(), dst.show()), &cv.other.show()), "violated");
                        }
                    }
                    if let Some(first) = seen.get(&key) {
                        self.rec("E-TYP-182", format!("two conversions are declared for the pair ({}, {}): this one, and one {} declares (module {}): one conversion per pair (6.13)", src.show(), dst.show(), first.0, first.1), "LANG-06-15", tok, Self::typ_fields("one conversion", "two", &cv.other.show()), "violated");
                    }
                }
                seen.entry(key).or_insert_with(|| (d.name.clone(), d.module.clone()));
            }
        }
    }

    fn check_coherence(&mut self) {
        self.check_implement_targets();
        let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
        let blocks = self.table.implement_blocks.clone();
        for (iface, target, module, node) in blocks {
            let tok = node.tokens()[0].clone();
            let idecl = self.resolve_named(&iface, None);
            let tdecl = self.implement_target_decl(node, &Type::Named(target.clone()));
            // one implementation per (interface, target type): the target's declaration with its type arguments, so
            // `Box[Int]` and `Box[String]` are distinct targets (9.5)
            let key = (Self::whole_type_key(idecl.as_ref(), &iface), Self::whole_type_key(tdecl.as_ref(), &target));
            if module != self.module_name {
                // an imported module's block (D-132): it counts for overlap; that module reports its own records
                seen.insert(key);
                continue;
            }
            let local_iface = idecl.as_ref().map_or(false, |i| i.package == self.package && i.module.split('.').next() != Some("fortis") && !i.module.starts_with("fortis."));
            let local_type = tdecl.as_ref().map_or(false, |t| t.package == self.package && !t.module.starts_with("fortis."));
            if !(local_iface || local_type) {
                self.rec(
                    "E-TYP-100",
                    format!("`implement {} for {}`: neither is declared in this package (coherence)", iface.name, target.name),
                    "LANG-06-10",
                    Some(&tok),
                    Self::typ_fields("a local interface or type", &format!("{}/{}", iface.name, target.name), &target.name),
                    "violated",
                );
            }
            let dup = seen.contains(&key) || tdecl.as_ref().map_or(false, |t| t.implements.iter().any(|i| i == &iface) || t.derives.iter().any(|(n, _)| *n == iface.interface_key()));
            if dup {
                self.rec("E-TYP-100", format!("two implementations of {} for {}", iface.name, target.name), "LANG-06-10", Some(&tok), Self::typ_fields("one implementation", "two", &target.name), "violated");
            }
            seen.insert(key);
            if let (Some(idecl), Some(tdecl)) = (idecl, tdecl) {
                if idecl.kind == "interface" {
                    let mut methods: OMap<Sig<'a>> = OMap::new();
                    for m in node.child_nodes("implementMember") {
                        if let Some(head) = m.find("methodHead") {
                            let sig = self.signature(head, true, m.find("funcBody"));
                            let n = sig.name.clone();
                            methods.insert(&n, sig);
                        }
                    }
                    self.check_implements(&tdecl, &iface, &methods, &tok);
                }
            }
        }
    }
}

pub fn param_compatible(iface_t: &Type, impl_t: &Type) -> bool {
    if !crate::literals::origin_clash(iface_t, impl_t).is_empty() { return false; }
    if iface_t.is_unknown() || impl_t.is_unknown() {
        return true;
    }
    match (iface_t, impl_t) {
        (Type::Refined { base: ib, lo: ilo, hi: ihi, .. }, Type::Refined { base: hb, lo: hlo, hi: hhi, .. }) if ib == hb => {
            let lo_ok = hlo.is_none() || (ilo.is_some() && hlo.unwrap() <= ilo.unwrap());
            let hi_ok = hhi.is_none() || (ihi.is_some() && hhi.unwrap() >= ihi.unwrap());
            lo_ok && hi_ok
        }
        (Type::Refined { base, .. }, Type::Named(n)) => base == n,
        (Type::Named(_), Type::Refined { .. }) => false,
        _ => iface_t == impl_t,
    }
}

pub fn return_compatible(iface_t: &Type, impl_t: &Type) -> bool {
    if !crate::literals::origin_clash(iface_t, impl_t).is_empty() { return false; }
    if iface_t.is_unknown() || impl_t.is_unknown() {
        return true;
    }
    match (iface_t, impl_t) {
        (Type::Refined { base: ib, lo: ilo, hi: ihi, .. }, Type::Refined { base: hb, lo: hlo, hi: hhi, .. }) if ib == hb => {
            let lo_ok = ilo.is_none() || (hlo.is_some() && hlo.unwrap() >= ilo.unwrap());
            let hi_ok = ihi.is_none() || (hhi.is_some() && hhi.unwrap() <= ihi.unwrap());
            lo_ok && hi_ok
        }
        (Type::Named(n), Type::Refined { base, .. }) => base == n,
        (Type::Refined { .. }, Type::Named(_)) => false,
        _ => iface_t == impl_t,
    }
}

/// The name token of an `@ignorable` application among a declaration's aspect uses (5.5, 11.8; X-345).
pub fn ignorable_use<'a>(uses: &[N<'a>]) -> Option<&'a Token> {
    uses.iter().filter_map(|u| u.direct_tokens().get(1).copied()).find(|t| t.text == "ignorable")
}
