//! The typed IR (Programme 5.2, "Typed CFG IR"; PROG-ARC-01, PROG-CMP-02).
//!
//! A program is a set of functions over resolved, typed, explicit expressions and statements. Control flow stays
//! structured (blocks, `if`, `loop`, `match`) so that a backend can emit idiomatic Rust and the VM can interpret it
//! directly; `crate::cfg` derives the control-flow graph, with its cleanup edges, that the validator's dataflow
//! checks run on. Every name is resolved: a local is a `LocalId`, a callee is a qualified function name, an
//! intrinsic or a dispatch through an interface. Every operand says how it uses its place (read, move, copy, alias
//! or editable loan), every argument carries its call mode, every coercion is explicit (`Present`, `Success`), and
//! outcome propagation, observation, reports and cleanup are statements or expressions of their own.

use fortis_types::types::Type;

pub type LocalId = u32;

/// A whole program: the user's modules and every library module they reach.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Program {
    pub types: Vec<TypeDef>,
    pub functions: Vec<Function>,
    pub consts: Vec<ConstDef>,
    /// the qualified name of `main`, when the program has one
    pub entry: Option<String>,
    pub tests: Vec<TestDef>,
    /// tests the builder could not lower (a construct outside what the IR builds yet), each with its title and
    /// the reason; runners report them as `unbuilt`, never as passed or failed
    pub unbuilt_tests: Vec<(String, String)>,
}

/// A module of the user's program (a single file's `main`, or a module of a package other than `fortis`).
pub fn is_user_module(m: &str) -> bool {
    !m.starts_with("fortis.") && m != "fortis"
}

impl Program {
    pub fn function(&self, name: &str) -> Option<&Function> {
        self.functions.iter().find(|f| f.name == name)
    }
    pub fn type_def(&self, name: &str) -> Option<&TypeDef> {
        self.types.iter().find(|t| t.name == name)
    }
}

/// A nominal type the program declares (class, enum, newtype, error, interface, observation schema).
#[derive(Clone, Debug, PartialEq)]
pub struct TypeDef {
    pub name: String,
    pub module: String,
    pub type_params: Vec<String>,
    pub kind: TypeKind,
    /// a trivial type lowers to a `Copy` value (Appendix C.1)
    pub trivial: bool,
    /// `copy p` is permitted (6.11)
    pub copyable: bool,
    pub implements: Vec<String>,
    /// the mixins whose methods it receives (9.6)
    pub mixins: Vec<String>,
    /// its lifetime parameters (`[lifetime r]`, 7.6): at most one
    pub lifetimes: Vec<String>,
    /// the function implementing `cleanup(editable self)`, run at destruction (9.4)
    pub cleanup: Option<String>,
    /// method name to the qualified function implementing it, for dispatch through an interface or type parameter
    pub methods: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TypeKind {
    Class { fields: Vec<FieldDef> },
    Enum { variants: Vec<VariantDef> },
    Newtype { repr: Type },
    Error { code: String, payload: Vec<FieldDef>, message: String },
    /// each method with the modifiers its signature declares (an implementation may not exceed them)
    Interface { methods: Vec<(String, Vec<String>)> },
    /// methods it provides (with bodies) and methods it requires of the class
    Mixin { provides: Vec<String>, requires: Vec<String> },
    /// the schema's fields and its `version` (1 when the declaration has none), which every record carries (Platform 6.1)
    Observation { fields: Vec<(String, Type, String)>, version: u32 },
}

#[derive(Clone, Debug, PartialEq)]
pub struct FieldDef {
    pub name: String,
    pub ty: Type,
    pub exported: bool,
    /// the value of an omitted field (a constant expression, 9.2)
    pub default: Option<Expr>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct VariantDef {
    pub name: String,
    pub fields: Vec<(String, Type)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConstDef {
    pub name: String,
    pub module: String,
    pub ty: Type,
    pub value: Expr,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TestDef {
    pub name: String,
    /// the title without quotes
    pub title: String,
    /// `test` or `property`
    pub kind: String,
    pub function: String,
    /// a table test's rows: each row's name and record, bound to the function's one parameter
    pub cases: Vec<(String, Expr)>,
    /// a property's parameters and the types its generators draw from (14)
    pub generators: Vec<(String, Type)>,
}

/// How a parameter receives its argument (7.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode {
    /// a read loan (the default)
    Read,
    /// an exclusive loan
    Editable,
    /// ownership transferred to the callee
    Owned,
}

impl Mode {
    pub fn word(self) -> &'static str {
        match self {
            Mode::Read => "read",
            Mode::Editable => "editable",
            Mode::Owned => "owned",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Function {
    /// qualified: `<module>::<label>`, e.g. `main::function total`, `fortis.prelude::method List.map`
    pub name: String,
    /// the checker's label: `function f`, `method T.m`, `lambda@12:5`, `cleanup T`, `test "name"`
    pub label: String,
    pub module: String,
    pub line: u32,
    /// the parameters, in order; each is a local (`self` first for a method with a receiver)
    pub params: Vec<Param>,
    pub ret: Type,
    /// `pure`, `noalloc`, `async`, `try`, `recovers`, `trusted`, and contract names, as declared
    pub modifiers: Vec<String>,
    pub locals: Vec<LocalDecl>,
    /// None for a trusted function: an intrinsic implements it
    pub body: Option<Block>,
    /// the closed effect summary the checker computed (11.1)
    pub effects: Effects,
    /// the observation schema of an `@observe` function: its frame is emitted at every exit (13)
    pub observe: Option<String>,
    /// a lambda's kind and environment: its captures are locals initialised from the closure value
    pub closure: Option<ClosureDef>,
}

impl Function {
    pub fn is_try(&self) -> bool {
        self.modifiers.iter().any(|m| m == "try")
    }
    pub fn has(&self, m: &str) -> bool {
        self.modifiers.iter().any(|x| x == m)
    }
    pub fn local(&self, id: LocalId) -> &LocalDecl {
        &self.locals[id as usize]
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    pub local: LocalId,
    pub name: String,
    pub mode: Mode,
    /// the value of an omitted argument (a constant expression)
    pub default: Option<Expr>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Effects {
    /// (effect, provenance): provenance is `proved` or `trusted`
    pub effects: Vec<(String, String)>,
    pub footprint: Vec<String>,
    pub authorities: Vec<String>,
}

impl Effects {
    pub fn has(&self, e: &str) -> bool {
        self.effects.iter().any(|(x, _)| x == e)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClosureDef {
    /// `fn`, `fn mut` or `fn once` (8.2)
    pub kind: String,
    pub captures: Vec<CaptureDef>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CaptureDef {
    /// the local of the lambda function that holds the capture
    pub local: LocalId,
    pub name: String,
    pub mode: CaptureMode,
}

/// How a lambda captures a place (8.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CaptureMode {
    /// a bare name: a trivial value, copied
    Value,
    Copy,
    Move,
    Alias,
    Editable,
    /// no capture list: the body sees the enclosing locals it names, by reference
    Implicit,
}

impl CaptureMode {
    pub fn word(self) -> &'static str {
        match self {
            CaptureMode::Value => "value",
            CaptureMode::Copy => "copy",
            CaptureMode::Move => "move",
            CaptureMode::Alias => "alias",
            CaptureMode::Editable => "editable",
            CaptureMode::Implicit => "implicit",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct LocalDecl {
    pub name: String,
    pub ty: Type,
    pub mutable: bool,
    pub kind: LocalKind,
    /// a lambda without a capture list refers to it, so it lives in a shared cell
    pub boxed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LocalKind {
    Param,
    SelfParam,
    Let,
    /// a binding introduced by a pattern, a `catch`, a `report ... as` or a `for`
    Binding,
    /// a lambda's capture
    Capture,
    /// a value the builder introduced to fix evaluation order
    Temp,
}

/// A lexical block: its statements run in order; at its exit the locals it declared are destroyed in reverse
/// declaration order unless moved (7.9), after its deferred blocks (12.1).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    /// the locals this block declares, in declaration order
    pub locals: Vec<LocalId>,
    pub line: u32,
    /// the deferred blocks registered in this block, run LIFO at every exit of it
    pub defers: Vec<Block>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Stmt {
    pub line: u32,
    pub kind: StmtKind,
    /// the statement's temporaries (5.7, 7.9), in creation order: destroyed at the end of the statement, in
    /// reverse order, unless a defect terminates the task at once (10.8); a temporary's local belongs to no
    /// block, so no block exit destroys it again
    pub temps: Vec<LocalId>,
}

impl Stmt {
    pub fn new(line: u32, kind: StmtKind) -> Stmt {
        Stmt { line, kind, temps: vec![] }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodecOp {
    ToText,
    ToBytes,
    FromText,
    FromBytes,
}

impl CodecOp {
    pub fn name(self) -> &'static str {
        match self {
            CodecOp::ToText => "toText",
            CodecOp::ToBytes => "toBytes",
            CodecOp::FromText => "fromText",
            CodecOp::FromBytes => "fromBytes",
        }
    }
    pub fn of(name: &str) -> Option<CodecOp> {
        Some(match name {
            "toText" => CodecOp::ToText,
            "toBytes" => CodecOp::ToBytes,
            "fromText" => CodecOp::FromText,
            "fromBytes" => CodecOp::FromBytes,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum StmtKind {
    /// introduce a local of the enclosing block, initialised by `init`
    Let { local: LocalId, init: Expr },
    /// destructure a value with an irrefutable pattern (`let P = move p`)
    LetPattern { pattern: Pattern, init: Expr },
    /// store into a place; `drop_old` destroys the previous value first (7.9)
    Assign { place: Place, value: Expr, drop_old: bool },
    /// `place op= value` on a numeric place
    Compound { place: Place, op: BinOp, value: Expr },
    Expr(Expr),
    /// `discard e`: evaluate and drop (5.5)
    Discard(Expr),
    If { cond: Expr, then: Block, els: Option<Block> },
    /// `loop`, and `while` (whose condition is the first statement: `if !cond { break }`)
    Loop { label: Option<String>, body: Block },
    /// `for binding in source`: the source's items in order (6.6), the binding a fresh local per iteration
    For { label: Option<String>, binding: Option<LocalId>, source: ForSource, body: Block },
    Break(Option<String>),
    Continue(Option<String>),
    /// return from the function (the success value of a `try` function)
    Return(Option<Expr>),
    /// the value of the enclosing block expression (`give e`)
    Give(Expr),
    /// the value of the enclosing `catch` (`recover e`)
    Recover(Expr),
    /// propagate a failure out of the `try` function
    Fail(Expr),
    Block(Block),
    Match { scrutinee: Expr, arms: Vec<Arm> },
    /// `capture field = e` into the activation's observation frame (13)
    Capture { field: String, value: Expr },
    /// `record Schema(field: e, ...)`: an observation event (13)
    Record { schema: String, fields: Vec<(String, Expr)> },
    /// `report e [as name]`: emit the failure's report and consume it (10.4)
    Report { value: Expr, bind: Option<LocalId> },
    /// `expect cond` in a test; the operands of a comparison are its evidence (14)
    Expect { cond: Expr, evidence: Option<(Expr, String, Expr)> },
    /// `defer { ... }` reached: registers the enclosing block's deferred block `defers[i]`, run at the block's exit
    /// in reverse registration order (5.1, 12.4); a defer control never reached is never run
    Defer(usize),
    /// `scope name { body }`: `local` holds the scope; at `body`'s exit its unjoined tasks are cancelled and joined
    /// before the body's deferred blocks run (12.5)
    Scope { local: LocalId, body: Block },
    /// destroy the value at `place` and leave it moved: the fields a `...move` spread's target lacks, dropped at
    /// the spread in the source's declaration order (9.2, D-250)
    Drop(Place),
}

#[derive(Clone, Debug, PartialEq)]
pub enum ForSource {
    /// `lo .. hi` (inclusive) or `lo ..< hi`
    Range { lo: Expr, hi: Expr, inclusive: bool },
    /// a List, Array, Set or Map value (its items, or its (key, value) pairs, in first-insertion order)
    Items(Expr),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Arm {
    pub pattern: Pattern,
    pub guard: Option<Expr>,
    /// the locals the pattern binds
    pub locals: Vec<LocalId>,
    pub body: ArmBody,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ArmBody {
    Expr(Expr),
    Block(Block),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Pattern {
    Wildcard,
    Bind(LocalId),
    Literal(Const),
    Null,
    Present(Box<Pattern>),
    Success(Box<Pattern>),
    Failure(Box<Pattern>),
    /// an enum variant, with its fields by name
    Variant { ty: String, variant: String, fields: Vec<(String, Pattern)> },
    /// a class destructured under move
    Class { ty: String, fields: Vec<(String, Pattern)> },
    /// `is Type(fields) [as name]`: a failure of the error type (or a value of the class)
    Is { ty: String, fields: Vec<(String, Pattern)>, bind: Option<LocalId> },
    Tuple(Vec<Pattern>),
    Record(Vec<(String, Pattern)>),
    /// `otherwise [ (name) ]`
    Otherwise(Option<LocalId>),
}

/// A place: a local (or `self`) and a path of projections.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Place {
    pub local: LocalId,
    pub proj: Vec<Proj>,
}

impl Place {
    pub fn local(local: LocalId) -> Place {
        Place { local, proj: vec![] }
    }
    pub fn field(mut self, f: &str) -> Place {
        self.proj.push(Proj::Field(f.to_string()));
        self
    }
    pub fn tuple(mut self, i: u32) -> Place {
        self.proj.push(Proj::Tuple(i));
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Proj {
    Field(String),
    /// `.0` of a tuple
    Tuple(u32),
    /// `[n]` with a literal index (a place form)
    Index(u32),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub ty: Type,
    pub line: u32,
}

/// How an operand uses its place (7.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Use {
    /// a read: a trivial value is copied, any other value is read through an implicit read loan
    Read,
    Move,
    /// `copy p`: an explicit deep copy (6.11)
    Copy,
    Alias,
    Editable,
}

impl Use {
    pub fn word(self) -> &'static str {
        match self {
            Use::Read => "read",
            Use::Move => "move",
            Use::Copy => "copy",
            Use::Alias => "alias",
            Use::Editable => "editable",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Const {
    Void,
    Bool(bool),
    /// an integer of the expression's type
    Int(i128),
    /// an IEEE 754 binary64, by its bits
    Float(u64),
    /// a binary32, by its bits
    Float32(u32),
    /// canonical decimal text, e.g. `12.50`
    Decimal(String),
    /// nanoseconds
    Duration(i64),
    Str(String),
    Bytes(Vec<u8>),
    Null,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Shl,
    Shr,
    BitAnd,
    BitOr,
    BitXor,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl BinOp {
    pub fn symbol(self) -> &'static str {
        use BinOp::*;
        match self {
            Add => "+",
            Sub => "-",
            Mul => "*",
            Div => "/",
            Rem => "%",
            Shl => "<<",
            Shr => ">>",
            BitAnd => "&",
            BitOr => "|",
            BitXor => "^",
            Eq => "==",
            Ne => "!=",
            Lt => "<",
            Le => "<=",
            Gt => ">",
            Ge => ">=",
        }
    }
    pub fn from_symbol(s: &str) -> Option<BinOp> {
        use BinOp::*;
        Some(match s {
            "+" => Add,
            "-" => Sub,
            "*" => Mul,
            "/" => Div,
            "%" => Rem,
            "<<" => Shl,
            ">>" => Shr,
            "&" => BitAnd,
            "|" => BitOr,
            "^" => BitXor,
            "==" => Eq,
            "!=" => Ne,
            "<" => Lt,
            "<=" => Le,
            ">" => Gt,
            ">=" => Ge,
            _ => return None,
        })
    }
    pub fn is_comparison(self) -> bool {
        matches!(self, BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UnOp {
    Neg,
    Not,
}

/// The function a call reaches.
#[derive(Clone, Debug, PartialEq)]
pub enum Callee {
    /// a function, method or associated function with a body, by qualified name
    Func(String),
    /// an intrinsic of the runtime (a trusted library member): `List.push`, `Int.add`, `Console.print`
    Intrinsic(String),
    /// a method reached through an interface or a type parameter: dispatched on the receiver's type
    Virtual { method: String },
    /// a closure or function value
    Value(Box<Expr>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Arg {
    /// the parameter it binds
    pub param: String,
    pub mode: Mode,
    pub value: Expr,
}

/// One part of an interpolated string.
#[derive(Clone, Debug, PartialEq)]
pub enum Part {
    Lit(String),
    /// the value's display form (6.10)
    Show(Expr),
}

#[derive(Clone, Debug, PartialEq)]
pub enum ExprKind {
    Const(Const),
    /// a place used as `Use` says
    Place(Place, Use),
    /// a module constant, by qualified name
    Global(String),
    /// a function as a value
    FuncRef(String),
    /// the canonical codec (6.10): `v.toText()`/`v.toBytes()` encode by the value; `T.fromText(s)`/`T.fromBytes(b)`
    /// decode by `target` (refinements and nominal types validated), yielding the value or a codec failure
    Codec { op: CodecOp, target: Type, arg: Box<Expr> },
    Binary(BinOp, Box<Expr>, Box<Expr>),
    Unary(UnOp, Box<Expr>),
    /// `&&`, `||`: the right operand only when needed
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Call { callee: Callee, args: Vec<Arg> },
    /// a class value: every field given, in declaration order
    Construct { ty: String, fields: Vec<(String, Expr)> },
    Variant { ty: String, variant: String, fields: Vec<(String, Expr)> },
    /// a failure of an error type (its payload by field)
    MakeFailure { ty: String, fields: Vec<(String, Expr)> },
    Tuple(Vec<Expr>),
    Record(Vec<(String, Expr)>),
    /// a lambda: its function and the places captured, in the function's capture order
    Closure { func: String, captures: Vec<Expr> },
    Interpolate(Vec<Part>),
    /// a field of a value that is not a place (a call result, a payload)
    Field(Box<Expr>, String),
    /// `.0` of a tuple value
    TupleField(Box<Expr>, u32),
    /// `xs[i]` with a computed index: a defect when out of range
    Index(Box<Expr>, Box<Expr>),
    /// `a?.f`: null when `a` is null
    OptField(Box<Expr>, String),
    /// a value of `T` where `T?` is expected
    Present(Box<Expr>),
    /// the success value of an outcome where `Outcome[T]` is expected (a `try` function's result)
    Success(Box<Expr>),
    /// `a ?? b`
    Coalesce(Box<Expr>, Box<Expr>),
    /// `e or fail`: a failure propagates out of the `try` function, else the success value
    OrFail(Box<Expr>),
    /// `e or recover r`
    OrRecover(Box<Expr>, Box<Expr>),
    /// `e catch name { ... }`: the block ends in `recover`, `return` or `fail`
    Catch { value: Box<Expr>, bind: LocalId, handler: Block },
    If { cond: Box<Expr>, then: Block, els: Block },
    Match { scrutinee: Box<Expr>, arms: Vec<Arm> },
    /// a block that ends in `give`
    Block(Block),
    /// `fail e` in expression position: type Never
    Fail(Box<Expr>),
    /// `T.validate(x)` against a refinement: the value, or a `refinement.out_of_range` failure
    Validate { value: Box<Expr>, lo: Option<i128>, hi: Option<i128>, length: bool },
    /// a newtype from its representation
    Wrap { ty: String, value: Box<Expr> },
    /// `await e`
    Await(Box<Expr>),
    /// a value of a class as `dynamic I` (Appendix C.1): owned (boxed, allocating) or borrowed (`alias dynamic I`)
    ToDynamic { iface: String, value: Box<Expr>, borrowed: bool },
    /// a statement temporary (7.9): `value` evaluated and held in `local`, which the statement's `temps` destroys
    /// at its end; the value itself flows on, read through
    Temp { local: LocalId, value: Box<Expr> },
}

impl Expr {
    pub fn new(kind: ExprKind, ty: Type, line: u32) -> Expr {
        Expr { kind, ty, line }
    }
    pub fn void(line: u32) -> Expr {
        Expr::new(ExprKind::Const(Const::Void), fortis_types::types::void(), line)
    }
}

impl Block {
    /// Whether every path through the block leaves it (return, fail, break, continue, give, recover, a Never call),
    /// so control never reaches its end.
    pub fn terminates(&self) -> bool {
        self.stmts.last().map_or(false, |s| s.terminates())
    }
}

impl Stmt {
    pub fn terminates(&self) -> bool {
        match &self.kind {
            StmtKind::Return(_) | StmtKind::Fail(_) | StmtKind::Recover(_) | StmtKind::Break(_) | StmtKind::Continue(_) | StmtKind::Give(_) => true,
            StmtKind::Expr(e) | StmtKind::Discard(e) => matches!(e.kind, ExprKind::Fail(_)) || matches!(&e.ty, fortis_types::types::Type::Named(n) if n.name == "Never"),
            StmtKind::If { then, els: Some(e), .. } => then.terminates() && e.terminates(),
            StmtKind::Block(b) | StmtKind::Scope { body: b, .. } => b.terminates(),
            StmtKind::Match { arms, .. } => {
                !arms.is_empty()
                    && arms.iter().all(|a| match &a.body {
                        ArmBody::Block(b) => b.terminates(),
                        ArmBody::Expr(e) => matches!(e.kind, ExprKind::Fail(_)),
                    })
            }
            StmtKind::Loop { body, .. } => !body.breaks_out(),
            _ => false,
        }
    }
}

impl Block {
    /// Whether a `break` occurs anywhere in the block (a loop with one may end).
    pub fn breaks_out(&self) -> bool {
        fn st(s: &Stmt) -> bool {
            match &s.kind {
                StmtKind::Break(_) => true,
                StmtKind::If { then, els, .. } => then.breaks_out() || els.as_ref().map_or(false, |e| e.breaks_out()),
                StmtKind::Loop { body, .. } | StmtKind::For { body, .. } => body.breaks_out(),
                StmtKind::Block(b) | StmtKind::Scope { body: b, .. } => b.breaks_out(),
                StmtKind::Match { arms, .. } => arms.iter().any(|a| matches!(&a.body, ArmBody::Block(b) if b.breaks_out())),
                _ => false,
            }
        }
        self.stmts.iter().any(st)
    }
}
