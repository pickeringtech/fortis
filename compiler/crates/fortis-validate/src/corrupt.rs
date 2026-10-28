//! Targeted corruptions (DEL-45 criterion 1): for each G1 row and pattern, one edit of that row's valid fixture
//! that breaks exactly the frontend-checked property the row's check guards.

use fortis_mir::model::*;
use fortis_types::types::{named, Type};

pub fn each_stmt_mut(b: &mut Block, f: &mut dyn FnMut(&mut Stmt)) {
    for d in &mut b.defers {
        each_stmt_mut(d, f);
    }
    for s in &mut b.stmts {
        f(s);
        match &mut s.kind {
            StmtKind::If { then, els, .. } => {
                each_stmt_mut(then, f);
                if let Some(e) = els {
                    each_stmt_mut(e, f);
                }
            }
            StmtKind::Loop { body, .. } | StmtKind::For { body, .. } | StmtKind::Block(body) | StmtKind::Scope { body, .. } => each_stmt_mut(body, f),
            StmtKind::Match { arms, .. } => {
                for a in arms {
                    if let ArmBody::Block(b) = &mut a.body {
                        each_stmt_mut(b, f);
                    }
                }
            }
            _ => {}
        }
        // statements nested in expressions: catch handlers, block, `if` and `match` expressions
        visit_stmt_exprs(s, &mut |e| expr_blocks(e, f));
    }
}

/// The statements of the blocks directly inside an expression (not inside its sub-expressions, which
/// `visit_stmt_exprs` reaches through `each_expr_mut`).
fn expr_blocks(e: &mut Expr, f: &mut dyn FnMut(&mut Stmt)) {
    match &mut e.kind {
        ExprKind::Catch { handler, .. } => each_stmt_mut(handler, f),
        ExprKind::Block(b) => each_stmt_mut(b, f),
        ExprKind::If { then, els, .. } => {
            each_stmt_mut(then, f);
            each_stmt_mut(els, f);
        }
        ExprKind::Match { arms, .. } => {
            for a in arms {
                if let ArmBody::Block(b) = &mut a.body {
                    each_stmt_mut(b, f);
                }
            }
        }
        _ => {}
    }
}

fn visit_stmt_exprs(s: &mut Stmt, f: &mut dyn FnMut(&mut Expr)) {
    match &mut s.kind {
        StmtKind::Let { init, .. } | StmtKind::LetPattern { init, .. } => each_expr_mut(init, f),
        StmtKind::Assign { value, .. } | StmtKind::Compound { value, .. } | StmtKind::Expr(value) | StmtKind::Discard(value) | StmtKind::Give(value) | StmtKind::Recover(value) | StmtKind::Fail(value) | StmtKind::Capture { value, .. } | StmtKind::Report { value, .. } => {
            each_expr_mut(value, f)
        }
        StmtKind::Return(Some(e)) => each_expr_mut(e, f),
        StmtKind::If { cond, .. } => each_expr_mut(cond, f),
        StmtKind::Match { scrutinee, arms } => {
            each_expr_mut(scrutinee, f);
            for a in arms {
                if let ArmBody::Expr(e) = &mut a.body {
                    each_expr_mut(e, f);
                }
            }
        }
        _ => {}
    }
}

/// Visit an expression and its sub-expressions (not the statements of nested blocks).
pub fn each_expr_mut(e: &mut Expr, f: &mut dyn FnMut(&mut Expr)) {
    f(e);
    match &mut e.kind {
        ExprKind::Binary(_, a, b) | ExprKind::And(a, b) | ExprKind::Or(a, b) | ExprKind::Coalesce(a, b) | ExprKind::OrRecover(a, b) | ExprKind::Index(a, b) => {
            each_expr_mut(a, f);
            each_expr_mut(b, f);
        }
        ExprKind::Unary(_, a)
        | ExprKind::Field(a, _)
        | ExprKind::TupleField(a, _)
        | ExprKind::OptField(a, _)
        | ExprKind::Present(a)
        | ExprKind::Success(a)
        | ExprKind::OrFail(a)
        | ExprKind::Fail(a)
        | ExprKind::Await(a)
        | ExprKind::Temp { value: a, .. }
        | ExprKind::Validate { value: a, .. }
        | ExprKind::Wrap { value: a, .. }
        | ExprKind::ToDynamic { value: a, .. }
        | ExprKind::Catch { value: a, .. } => each_expr_mut(a, f),
        ExprKind::Call { callee, args } => {
            if let Callee::Value(v) = callee {
                each_expr_mut(v, f);
            }
            for a in args {
                each_expr_mut(&mut a.value, f);
            }
        }
        ExprKind::Construct { fields, .. } | ExprKind::Variant { fields, .. } | ExprKind::MakeFailure { fields, .. } | ExprKind::Record(fields) => fields.iter_mut().for_each(|(_, x)| each_expr_mut(x, f)),
        ExprKind::Tuple(es) | ExprKind::Closure { captures: es, .. } => es.iter_mut().for_each(|x| each_expr_mut(x, f)),
        ExprKind::Interpolate(parts) => parts.iter_mut().for_each(|p| {
            if let Part::Show(x) = p {
                each_expr_mut(x, f)
            }
        }),
        ExprKind::If { cond, .. } => each_expr_mut(cond, f),
        ExprKind::Match { scrutinee, .. } => each_expr_mut(scrutinee, f),
        _ => {}
    }
}

fn user_fn<'p>(p: &'p mut Program, label: &str) -> Option<&'p mut Function> {
    p.functions.iter_mut().find(|f| f.module == "main" && f.label == label)
}

fn first_expr(f: &mut Function, pred: &dyn Fn(&Expr) -> bool, edit: &mut dyn FnMut(&mut Expr)) -> bool {
    let mut done = false;
    if let Some(b) = &mut f.body {
        each_stmt_mut(b, &mut |s| {
            if done {
                return;
            }
            visit_stmt_exprs(s, &mut |e| {
                if !done && pred(e) {
                    edit(e);
                    done = true;
                }
            });
        });
    }
    done
}

/// Every block of a body (pre-order), for edits that insert or reorder statements.
fn each_block_mut(b: &mut Block, f: &mut dyn FnMut(&mut Block)) {
    f(b);
    for d in &mut b.defers {
        each_block_mut(d, f);
    }
    for s in &mut b.stmts {
        match &mut s.kind {
            StmtKind::If { then, els, .. } => {
                each_block_mut(then, f);
                if let Some(e) = els {
                    each_block_mut(e, f);
                }
            }
            StmtKind::Loop { body, .. } | StmtKind::For { body, .. } | StmtKind::Block(body) | StmtKind::Scope { body, .. } => each_block_mut(body, f),
            _ => {}
        }
    }
}

fn typed(f: &Function, l: LocalId, name: &str) -> bool {
    crate::validate::type_name(&f.local(l).ty) == Some(name)
}

fn add_effect(f: &mut Function, e: &str) {
    if !f.effects.has(e) {
        f.effects.effects.push((e.to_string(), "proved".to_string()));
    }
}

fn line_expr(kind: ExprKind, ty: Type, line: u32) -> Expr {
    Expr::new(kind, ty, line)
}

/// Apply the corruption for a check id to the fixture's program. Returns false when the fixture lacks the target.
pub fn corrupt(id: &str, p: &mut Program) -> bool {
    match id {
        "C1.newtype" => user_fn(p, "function main").map_or(false, |f| {
            first_expr(f, &|e| matches!(e.kind, ExprKind::Wrap { .. }), &mut |e| {
                if let ExprKind::Wrap { value, .. } = &mut e.kind {
                    value.kind = ExprKind::Const(Const::Int(200));
                }
            })
        }),
        "C1.class" => match p.types.iter_mut().find(|t| t.module == "main" && matches!(t.kind, TypeKind::Class { .. })) {
            Some(t) => {
                t.module = "acme.other".into();
                if let TypeKind::Class { fields } = &mut t.kind {
                    if let Some(fd) = fields.first_mut() {
                        fd.exported = false;
                    }
                }
                true
            }
            None => false,
        },
        "C1.trivial" => match p.types.iter_mut().find(|t| t.module == "main" && t.trivial) {
            Some(t) => {
                if let TypeKind::Class { fields } = &mut t.kind {
                    if let Some(fd) = fields.last_mut() {
                        fd.ty = named("String");
                        return true;
                    }
                }
                false
            }
            None => false,
        },
        "C1.copy" => match p.types.iter_mut().find(|t| t.module == "main" && t.copyable && !t.trivial) {
            Some(t) => {
                t.copyable = false;
                true
            }
            None => false,
        },
        "C1.read-loan" => user_fn(p, "function main").map_or(false, |f| {
            // the loan holder's declaration moves to the outer block: it outlives the inner block that owns the place
            let body = match &mut f.body {
                Some(b) => b,
                None => return false,
            };
            let mut holder = None;
            for s in &mut body.stmts {
                if let StmtKind::Block(inner) = &mut s.kind {
                    for st in &inner.stmts {
                        if let StmtKind::Let { local, init: Expr { kind: ExprKind::Place(_, Use::Alias), .. } } = &st.kind {
                            holder = Some(*local);
                        }
                    }
                    if let Some(h) = holder {
                        inner.locals.retain(|l| *l != h);
                    }
                }
            }
            match holder {
                Some(h) => {
                    body.locals.push(h);
                    true
                }
                None => false,
            }
        }),
        "C1.exclusive-loan" => user_fn(p, "function main").map_or(false, |f| {
            let body = match &mut f.body {
                Some(b) => b,
                None => return false,
            };
            let at = body.stmts.iter().position(|s| matches!(&s.kind, StmtKind::Let { init: Expr { kind: ExprKind::Place(_, Use::Editable), .. }, .. }));
            match at {
                Some(i) => {
                    let (owner, ty, line) = match &body.stmts[i].kind {
                        StmtKind::Let { init, .. } => match &init.kind {
                            ExprKind::Place(p, _) => (p.clone(), init.ty.clone(), body.stmts[i].line),
                            _ => return false,
                        },
                        _ => return false,
                    };
                    body.stmts.insert(i + 1, Stmt { line, temps: vec![], kind: StmtKind::Discard(line_expr(ExprKind::Place(owner, Use::Read), ty, line)) });
                    true
                }
                None => false,
            }
        }),
        "C1.move" => user_fn(p, "function main").map_or(false, |f| {
            let body = match &mut f.body {
                Some(b) => b,
                None => return false,
            };
            let moved = body.stmts.iter().find_map(|s| match &s.kind {
                StmtKind::Let { init: Expr { kind: ExprKind::Place(p, Use::Move), ty, .. }, .. } => Some((p.clone(), ty.clone(), s.line)),
                _ => None,
            });
            match moved {
                Some((pl, ty, line)) => {
                    body.stmts.push(Stmt { line, temps: vec![], kind: StmtKind::Discard(line_expr(ExprKind::Place(pl, Use::Read), ty, line)) });
                    true
                }
                None => false,
            }
        }),
        "C1.from-return" => p.functions.iter_mut().find(|f| f.module == "main" && matches!(f.ret, Type::Loan { .. })).map_or(false, |f| {
            let other = f.params.iter().find(|p| match &f.ret {
                Type::Loan { origin, .. } => &p.name != origin,
                _ => false,
            });
            let other = match other {
                Some(o) => o.local,
                None => return false,
            };
            let mut done = false;
            if let Some(b) = &mut f.body {
                each_stmt_mut(b, &mut |s| {
                    if let StmtKind::Return(Some(e)) = &mut s.kind {
                        each_expr_mut(e, &mut |x| {
                            if let ExprKind::Place(pl, _) = &mut x.kind {
                                if !done {
                                    pl.local = other;
                                    done = true;
                                }
                            }
                        });
                    }
                });
            }
            done
        }),
        "C1.standard-loans" => match p.types.iter_mut().find(|t| t.module == "main") {
            Some(t) => {
                t.lifetimes.push("r".into());
                true
            }
            None => false,
        },
        "C1.shared" => user_fn(p, "function main").map_or(false, |f| {
            first_expr(f, &|e| matches!(&e.kind, ExprKind::Call { callee: Callee::Intrinsic(n), .. } if n == "Shared.retain"), &mut |e| {
                if let ExprKind::Call { args, .. } = &e.kind {
                    if let ExprKind::Place(pl, _) = &args[0].value.kind {
                        e.kind = ExprKind::Place(pl.clone(), Use::Copy);
                    }
                }
            })
        }),
        "C1.interface" => {
            let impls: Vec<String> = p.types.iter().filter(|t| t.module == "main" && !t.implements.is_empty()).flat_map(|t| t.methods.iter().map(|(_, f)| f.clone())).collect();
            match impls.first().and_then(|n| p.functions.iter_mut().find(|f| &f.name == n)) {
                Some(f) => {
                    add_effect(f, "io");
                    true
                }
                None => false,
            }
        }
        "C1.dynamic" => p.functions.iter_mut().filter(|f| f.module == "main").any(|f| {
            first_expr(f, &|e| matches!(e.kind, ExprKind::ToDynamic { .. }), &mut |e| {
                if let ExprKind::ToDynamic { iface, .. } = &mut e.kind {
                    *iface = "Unrelated".into();
                }
            })
        }),
        "C1.mixin" => match p.types.iter_mut().find(|t| t.module == "main" && !t.mixins.is_empty()) {
            Some(t) => match t.methods.first().cloned() {
                Some(m) => {
                    t.methods.push(m);
                    true
                }
                None => false,
            },
            None => false,
        },
        "C1.lambda" => match p.functions.iter_mut().find(|f| f.closure.as_ref().map_or(false, |c| c.kind == "fn mut")) {
            Some(f) => {
                f.closure.as_mut().unwrap().kind = "fn".into();
                true
            }
            None => false,
        },
        "C1.try" => p.functions.iter_mut().filter(|f| f.module == "main").any(|f| {
            let mut done = false;
            if let Some(b) = &mut f.body {
                each_stmt_mut(b, &mut |s| {
                    if done {
                        return;
                    }
                    if let StmtKind::Let { init, .. } = &s.kind {
                        if let ExprKind::OrFail(inner) = &init.kind {
                            let call = (**inner).clone();
                            s.kind = StmtKind::Expr(call);
                            done = true;
                        }
                    }
                });
            }
            done
        }),
        "C1.catch" => p.functions.iter_mut().filter(|f| f.module == "main").any(|f| {
            first_expr(f, &|e| matches!(e.kind, ExprKind::Catch { .. }), &mut |e| {
                if let ExprKind::Catch { handler, .. } = &mut e.kind {
                    handler.stmts.retain(|s| !matches!(s.kind, StmtKind::Recover(_)));
                }
            })
        }),
        "C1.optional" => p.functions.iter_mut().filter(|f| f.module == "main").any(|f| {
            first_expr(f, &|e| matches!(e.kind, ExprKind::Coalesce(..)), &mut |e| {
                if let ExprKind::Coalesce(a, b) = &e.kind {
                    e.kind = ExprKind::OrRecover(a.clone(), b.clone());
                }
            })
        }),
        "C1.match" => p.functions.iter_mut().filter(|f| f.module == "main").any(|f| {
            let mut done = false;
            let locals = f.locals.clone();
            if let Some(b) = &mut f.body {
                each_stmt_mut(b, &mut |s| {
                    if let StmtKind::Match { arms, .. } = &mut s.kind {
                        for a in arms {
                            let bound: Vec<LocalId> = a.locals.clone();
                            let mut edit = |e: &mut Expr| {
                                each_expr_mut(e, &mut |x| {
                                    if let ExprKind::Place(pl, u) = &mut x.kind {
                                        if !done && bound.contains(&pl.local) && type_heap(&locals[pl.local as usize].ty) {
                                            *u = Use::Move;
                                            done = true;
                                        }
                                    }
                                })
                            };
                            match &mut a.body {
                                ArmBody::Expr(e) => edit(e),
                                ArmBody::Block(b) => each_stmt_mut(b, &mut |st| visit_stmt_exprs(st, &mut edit)),
                            }
                        }
                    }
                });
            }
            done
        }),
        "C1.cleanup" => {
            let destroyer = p.functions.iter_mut().find(|f| f.module == "main" && f.label == "function main");
            match destroyer {
                Some(f) => {
                    let before = f.effects.effects.len();
                    f.effects.effects.retain(|(e, _)| e != "alloc");
                    f.effects.effects.len() != before
                }
                None => false,
            }
        }
        "C1.noalloc" => p.functions.iter_mut().find(|f| f.has("noalloc")).map_or(false, |f| {
            add_effect(f, "alloc");
            true
        }),
        "C1.effects" => p.functions.iter_mut().find(|f| f.module == "main" && f.has("pure")).map_or(false, |f| {
            add_effect(f, "io");
            true
        }),
        "C1.observation" => p.functions.iter_mut().find(|f| f.observe.is_some()).map_or(false, |f| {
            let mut done = false;
            if let Some(b) = &mut f.body {
                let before = b.stmts.len();
                b.stmts.retain(|s| !matches!(s.kind, StmtKind::Capture { .. }));
                done = b.stmts.len() != before;
            }
            done
        }),
        "C1.capability" => p.functions.iter_mut().find(|f| f.module == "main" && !f.effects.authorities.is_empty() && f.label != "function main").map_or(false, |f| {
            // the capability is conjured locally instead of arriving through a parameter
            let caps: Vec<LocalId> = f.params.iter().map(|p| p.local).filter(|l| crate::validate::type_name(&f.locals[*l as usize].ty).map_or(false, |n| n == "Console")).collect();
            for l in &caps {
                f.locals[*l as usize].kind = LocalKind::Let;
            }
            !caps.is_empty()
        }),
        // ---- C.3: whitelisted operations made to allocate, allocating ones moved into noalloc
        "C3.arithmetic" | "C3.match" | "C3.capture" => p.functions.iter_mut().find(|f| f.has("noalloc")).map_or(false, |f| {
            first_expr(f, &|e| matches!(e.kind, ExprKind::Binary(..) | ExprKind::Place(..)) && e.ty.show() == "Int", &mut |e| {
                let line = e.line;
                let inner = e.clone();
                *e = line_expr(ExprKind::Interpolate(vec![Part::Show(inner)]), named("String"), line);
            })
        }),
        "C3.access" | "C3.loans" => p.functions.iter_mut().find(|f| f.has("noalloc")).map_or(false, |f| {
            first_expr(f, &|e| matches!(&e.kind, ExprKind::Place(_, u) if *u == Use::Read || *u == Use::Alias) && type_heap(&e.ty), &mut |e| {
                if let ExprKind::Place(_, u) = &mut e.kind {
                    *u = Use::Copy;
                }
            })
        }),
        "C3.inline-construct" => match p.types.iter_mut().find(|t| t.module == "main" && t.trivial) {
            Some(t) => {
                t.trivial = false;
                if let TypeKind::Class { fields } = &mut t.kind {
                    if let Some(fd) = fields.last_mut() {
                        fd.ty = named("String");
                    }
                }
                // the construction's field value becomes a heap value
                let tn = t.name.clone();
                p.functions.iter_mut().filter(|f| f.has("noalloc")).any(|f| {
                    first_expr(f, &|e| matches!(&e.kind, ExprKind::Construct { ty, .. } if *ty == tn), &mut |e| {
                        if let ExprKind::Construct { fields, .. } = &mut e.kind {
                            if let Some((_, v)) = fields.last_mut() {
                                v.ty = named("String");
                            }
                        }
                    })
                })
            }
            None => false,
        },
        "C3.call" => {
            let callee = p.functions.iter().find(|f| f.has("noalloc")).and_then(|f| {
                let mut c = None;
                if let Some(b) = &f.body {
                    crate::validate::walk_block(b, &mut |n| {
                        if let crate::validate::Node::Expr(e) = n {
                            if let ExprKind::Call { callee: Callee::Func(name), .. } = &e.kind {
                                c.get_or_insert(name.clone());
                            }
                        }
                    });
                }
                c
            });
            match callee.and_then(|n| p.functions.iter_mut().find(|f| f.name == n)) {
                Some(g) => {
                    add_effect(g, "alloc");
                    true
                }
                None => false,
            }
        }
        // ---- the asynchronous rows (DEL-39)
        "C1.mutex" => user_fn(p, "function update").map_or(false, |f| {
            let guards: Vec<LocalId> = (0..f.locals.len() as LocalId).filter(|l| typed(f, *l, "MutexGuard")).collect();
            let mut done = false;
            if let Some(b) = &mut f.body {
                each_block_mut(b, &mut |blk| {
                    if done {
                        return;
                    }
                    if let Some(i) = blk.stmts.iter().position(|s| matches!(&s.kind, StmtKind::Let { local, .. } if guards.contains(local))) {
                        let line = blk.stmts[i].line;
                        blk.stmts.insert(i + 1, Stmt { line, temps: vec![], kind: StmtKind::Expr(Expr::new(ExprKind::Await(Box::new(Expr::void(line))), fortis_types::types::void(), line)) });
                        done = true;
                    }
                });
            }
            done
        }),
        "C1.async" => user_fn(p, "function main").map_or(false, |f| {
            let futures: Vec<LocalId> = (0..f.locals.len() as LocalId).filter(|l| typed(f, *l, "Future")).collect();
            first_expr(f, &|e| matches!(&e.kind, ExprKind::Await(x) if matches!(&x.kind, ExprKind::Place(p, _) if futures.contains(&p.local))), &mut |e| {
                if let ExprKind::Await(x) = &mut e.kind {
                    **x = Expr::void(x.line);
                }
            })
        }),
        "C1.scope" => user_fn(p, "function main").map_or(false, |f| {
            let tasks: Vec<LocalId> = (0..f.locals.len() as LocalId).filter(|l| typed(f, *l, "Task")).collect();
            let mut done = false;
            if let Some(b) = &mut f.body {
                each_stmt_mut(b, &mut |s| {
                    if done {
                        return;
                    }
                    if let StmtKind::Scope { body, .. } = &mut s.kind {
                        if let Some(t) = body.stmts.iter().find_map(|st| match &st.kind {
                            StmtKind::Let { local, .. } if tasks.contains(local) => Some(*local),
                            _ => None,
                        }) {
                            let line = body.line;
                            body.stmts.push(Stmt { line, temps: vec![], kind: StmtKind::Return(Some(Expr::new(ExprKind::Place(Place { local: t, proj: vec![] }, Use::Move), named("Task"), line))) });
                            done = true;
                        }
                    }
                });
            }
            done
        }),
        "C1.launch-parallel" => user_fn(p, "function main").map_or(false, |f| {
            first_expr(f, &|e| matches!(&e.kind, ExprKind::Call { callee: Callee::Intrinsic(n), .. } if n == "Scope.launchParallel"), &mut |e| {
                if let ExprKind::Call { args, .. } = &mut e.kind {
                    if let Some(fut) = args.get_mut(1) {
                        if let ExprKind::Call { args: fargs, .. } = &mut fut.value.kind {
                            for a in fargs {
                                if let ExprKind::Place(_, u) = &mut a.value.kind {
                                    *u = Use::Alias;
                                }
                            }
                        }
                    }
                }
            })
        }),
        "C1.defer" => user_fn(p, "function main").map_or(false, |f| {
            let mut done = false;
            if let Some(b) = &mut f.body {
                each_block_mut(b, &mut |blk| {
                    if done {
                        return;
                    }
                    if let Some(i) = blk.stmts.iter().position(|s| matches!(s.kind, StmtKind::Defer(_))) {
                        if i > 0 {
                            let d = blk.stmts.remove(i);
                            blk.stmts.insert(0, d);
                            done = true;
                        }
                    }
                });
            }
            done
        }),
        "C3.defer" => user_fn(p, "function total").map_or(false, |f| {
            let mut done = false;
            if let Some(b) = &mut f.body {
                each_block_mut(b, &mut |blk| {
                    if done {
                        return;
                    }
                    if let Some(d) = blk.defers.first_mut() {
                        let line = d.line;
                        d.stmts.push(Stmt { line, temps: vec![], kind: StmtKind::Discard(Expr::new(ExprKind::Interpolate(vec![Part::Lit("grows".into())]), named("String"), line)) });
                        done = true;
                    }
                });
            }
            done
        }),
        "C3.launch" => user_fn(p, "function both").map_or(false, |f| {
            let had = f.effects.has("alloc");
            f.effects.effects.retain(|(e, _)| e != "alloc");
            had
        }),
        "C3.boxed-future" => user_fn(p, "function pull").map_or(false, |f| {
            let had = f.effects.has("alloc");
            f.effects.effects.retain(|(e, _)| e != "alloc");
            had
        }),
        "C3.parallel" => user_fn(p, "function run").map_or(false, |f| {
            let had = f.effects.has("alloc");
            f.effects.effects.retain(|(e, _)| e != "alloc");
            had
        }),
        "C3.growth" | "C3.shared-create" | "C3.dynamic-create" | "C3.copy-heap" | "C3.failure-payload" => {
            p.functions.iter_mut().find(|f| f.module == "main" && f.label == "function build").map_or(false, |f| {
                f.modifiers.push("noalloc".into());
                f.effects.effects.retain(|(e, _)| e != "alloc");
                true
            })
        }
        _ => false,
    }
}

fn type_heap(t: &Type) -> bool {
    let n = crate::validate::type_name(t).unwrap_or("");
    matches!(n, "String" | "List" | "Map" | "Set" | "Bytes") || (!fortis_types::types::is_trivial_builtin(n) && !matches!(n, "" | "Bool" | "Void" | "Float" | "Decimal" | "Duration" | "Int"))
}
