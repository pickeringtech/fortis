//! `fortisc build` (DEL-43): a checked program to a native executable, offline, with a build manifest.
//!
//! - **Feature sets first** (PROG-ARC-03, PLAT-01-03): the check runs with the target's feature set, so a module
//!   the target lacks fails before any native step.
//! - **No build scripts, prebuilt runtime** (PROG-CMP-06): a package that ships one is refused. The runtime crates
//!   (`fortis-native`, `fortis-runtime`) are vendored path crates of the toolchain, built once into
//!   `build/native-runtime` (offline; their graph must hold no build script and nothing from a registry), and the
//!   generated program is compiled by rustc directly against that prebuilt rlib.
//! - **Compile and link timed apart** (PROG-NFR-07): rustc links through this executable acting as the linker
//!   (`FORTIS_LINK_TIMES`), which runs `cc` and records how long it took. Generated files that did not change are
//!   not rewritten, so an unchanged program builds with nothing to compile or link.
//! - **Reproducible** (PROG-NFR-22): paths are remapped (`--remap-path-prefix`), the crate has one codegen unit, and
//!   the manifest lists its own nondeterministic fields; two clean clones give identical executables.
//! - **Exposed changes** (PROG-NFR-15): the manifest records code size (by ELF section) and allocation (per
//!   function: whether it allocates, its clones and reference counts), and the deltas from the previous build.
//! - **Trust** (PROG-ARC-05): for every function of the program, the trusted operations it reaches transitively:
//!   runtime intrinsics and the program's own foreign `trusted` functions.
//! - **Three notions apart** (PROG-ARC-06): the manifest names the target's feature set, each function's effects
//!   and each function's required authorities in separate fields.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use fortis_diag::{Json, Record};
use fortis_mir::model::{Callee, ExprKind, Program};

pub const SCHEMA: &str = "fortis.build/v1";

/// The fields of the manifest that differ between identical builds (disclosed, PROG-NFR-22).
pub const NONDETERMINISTIC: &[&str] = &["timings", "rewritten", "changes"];

/// Act as the linker when rustc invokes this executable with `FORTIS_LINK_TIMES` set: run `cc` with the same
/// arguments and append the milliseconds it took to that file.
pub fn link_shim() -> Option<i32> {
    let times = std::env::var_os("FORTIS_LINK_TIMES")?;
    let args: Vec<String> = std::env::args().skip(1).collect();
    let t0 = Instant::now();
    let status = std::process::Command::new("cc").args(&args).status();
    let ms = t0.elapsed().as_secs_f64() * 1e3;
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&times) {
        let _ = writeln!(f, "{ms:.3}");
    }
    Some(status.map(|s| s.code().unwrap_or(1)).unwrap_or(1))
}

/// Section name -> size in bytes, for an ELF64 little-endian executable (empty for anything else).
pub fn elf_sections(bytes: &[u8]) -> BTreeMap<String, u64> {
    let mut out = BTreeMap::new();
    if bytes.len() < 64 || &bytes[..4] != b"\x7fELF" || bytes[4] != 2 || bytes[5] != 1 {
        return out;
    }
    let u16_ = |o: usize| bytes.get(o..o + 2).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]) as usize);
    let u32_ = |o: usize| bytes.get(o..o + 4).map_or(0, |b| u32::from_le_bytes(b.try_into().unwrap()) as usize);
    let u64_ = |o: usize| bytes.get(o..o + 8).map_or(0, |b| u64::from_le_bytes(b.try_into().unwrap()));
    let (shoff, shentsize, shnum, shstrndx) = (u64_(0x28) as usize, u16_(0x3A), u16_(0x3C), u16_(0x3E));
    if shoff == 0 || shentsize < 64 || shstrndx >= shnum {
        return out;
    }
    let strtab = u64_(shoff + shstrndx * shentsize + 0x18) as usize;
    for i in 0..shnum {
        let h = shoff + i * shentsize;
        let name_off = strtab + u32_(h);
        let name: String = bytes.get(name_off..).map(|b| b.iter().take_while(|c| **c != 0).map(|c| *c as char).collect()).unwrap_or_default();
        if !name.is_empty() {
            out.insert(name, u64_(h + 0x20));
        }
    }
    out
}

/// Section name -> SHA-256 of its bytes (what two builds compare, section by section).
pub fn elf_section_digests(bytes: &[u8]) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    if bytes.len() < 64 || &bytes[..4] != b"\x7fELF" {
        return out;
    }
    let u16_ = |o: usize| bytes.get(o..o + 2).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]) as usize);
    let u32_ = |o: usize| bytes.get(o..o + 4).map_or(0, |b| u32::from_le_bytes(b.try_into().unwrap()) as usize);
    let u64_ = |o: usize| bytes.get(o..o + 8).map_or(0, |b| u64::from_le_bytes(b.try_into().unwrap()));
    let (shoff, shentsize, shnum, shstrndx) = (u64_(0x28) as usize, u16_(0x3A), u16_(0x3C), u16_(0x3E));
    if shoff == 0 || shentsize < 64 || shstrndx >= shnum {
        return out;
    }
    let strtab = u64_(shoff + shstrndx * shentsize + 0x18) as usize;
    for i in 0..shnum {
        let h = shoff + i * shentsize;
        let name: String = bytes.get(strtab + u32_(h)..).map(|b| b.iter().take_while(|c| **c != 0).map(|c| *c as char).collect()).unwrap_or_default();
        let (off, size, kind) = (u64_(h + 0x18) as usize, u64_(h + 0x20) as usize, u32_(h + 4));
        // SHT_NOBITS (.bss) has no bytes in the file
        let data = if kind == 8 { &[][..] } else { bytes.get(off..off + size).unwrap_or(&[]) };
        if !name.is_empty() {
            out.insert(name, fortis_names::sha256::sha256(data));
        }
    }
    out
}

/// Every function's direct callees: functions by name, and intrinsics (`intrinsic:` prefixed).
fn call_graph(prog: &Program) -> BTreeMap<String, BTreeSet<String>> {
    let mut out = BTreeMap::new();
    for f in &prog.functions {
        let mut callees = BTreeSet::new();
        if let Some(b) = &f.body {
            fortis_validate::validate::walk_block(b, &mut |n| {
                if let fortis_validate::validate::Node::Expr(e) = n {
                    match &e.kind {
                        ExprKind::Call { callee: Callee::Func(name), .. } => {
                            callees.insert(name.clone());
                        }
                        ExprKind::Call { callee: Callee::Intrinsic(name), .. } => {
                            callees.insert(format!("intrinsic:{name}"));
                        }
                        ExprKind::FuncRef(name) => {
                            callees.insert(name.clone());
                        }
                        _ => {}
                    }
                }
            });
        }
        out.insert(f.name.clone(), callees);
    }
    out
}

/// For each function of the user's program, the trusted operations it reaches transitively (PROG-ARC-05).
pub fn trust(prog: &Program) -> Json {
    let graph = call_graph(prog);
    let foreign: BTreeSet<&str> = prog.functions.iter().filter(|f| f.body.is_none() && fortis_mir::model::is_user_module(&f.module)).map(|f| f.name.as_str()).collect();
    let mut rows = Vec::new();
    for f in prog.functions.iter().filter(|f| fortis_mir::model::is_user_module(&f.module) && f.body.is_some()) {
        let mut seen = BTreeSet::new();
        let mut todo = vec![f.name.clone()];
        let mut assumptions: BTreeMap<String, &str> = BTreeMap::new();
        while let Some(n) = todo.pop() {
            if !seen.insert(n.clone()) {
                continue;
            }
            for c in graph.get(&n).into_iter().flatten() {
                if let Some(i) = c.strip_prefix("intrinsic:") {
                    let kind = if i.starts_with("kit:") { "kit adapter" } else { "runtime intrinsic (fortis-native)" };
                    assumptions.insert(i.to_string(), kind);
                } else if foreign.contains(c.as_str()) {
                    assumptions.insert(c.clone(), "foreign trusted function");
                } else {
                    todo.push(c.clone());
                }
            }
        }
        let trusted_effects: Vec<String> = f.effects.effects.iter().filter(|(_, l)| l == "trusted").map(|(e, _)| e.clone()).collect();
        rows.push(Json::obj([
            ("operation", Json::str(&f.name)),
            ("line", Json::Int(f.line as i64)),
            ("trusted_effects", Json::strs(trusted_effects)),
            ("assumptions", Json::Arr(assumptions.into_iter().map(|(op, kind)| Json::obj([("operation", Json::str(&op)), ("kind", Json::str(kind))])).collect())),
        ]));
    }
    Json::Arr(rows)
}

/// Per-function allocation facts (PROG-NFR-15): whether it allocates, and its clones and reference counts.
pub fn allocation(prog: &Program, audit: &fortis_lower::Audit) -> BTreeMap<String, (bool, usize, usize)> {
    let mut out = BTreeMap::new();
    for f in prog.functions.iter().filter(|f| fortis_mir::model::is_user_module(&f.module) && f.body.is_some()) {
        let short = f.name.clone();
        let clones = audit.clones.iter().filter(|c| c.0 == short || short.ends_with(&format!("::{}", c.0)) || c.0.ends_with(&f.label)).count();
        let rcs = audit.refcounts.iter().filter(|c| c.0 == short || short.ends_with(&format!("::{}", c.0)) || c.0.ends_with(&f.label)).count();
        out.insert(f.name.clone(), (f.effects.has("alloc"), clones, rcs));
    }
    out
}

/// A package (or one of its dependencies) that ships a Cargo build script, which native builds refuse (PROG-CMP-06).
pub fn build_scripts(package_dirs: &[PathBuf]) -> Vec<(PathBuf, PathBuf)> {
    let mut out = Vec::new();
    for d in package_dirs {
        let mut stack = vec![d.clone()];
        while let Some(p) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&p) else { continue };
            for e in rd.flatten() {
                let path = e.path();
                let name = e.file_name().to_string_lossy().to_string();
                if path.is_dir() {
                    if !name.starts_with('.') && name != "target" && name != "build" {
                        stack.push(path);
                    }
                } else if name == "build.rs" {
                    out.push((d.clone(), path));
                } else if name == "Cargo.toml" && std::fs::read_to_string(&path).map_or(false, |t| t.lines().any(|l| l.trim_start().starts_with("build ") || l.trim_start().starts_with("build="))) {
                    out.push((d.clone(), path));
                }
            }
        }
    }
    out.sort();
    out
}

/// The record refusing a build script.
pub fn build_script_record(package: &Path, script: &Path) -> Record {
    Record::new(
        "E-NAM-032",
        format!("package {} ships a build script ({}): native builds execute no package build scripts; vendor a prebuilt crate instead", package.display(), script.display()),
        "PROG-CMP-06",
        &fortis_names::resolve::rel(&package.join("project.yaml")),
        1,
        1,
        Json::obj([("name", Json::str(&script.display().to_string())), ("candidates", Json::Arr(vec![]))]),
    )
}

/// The record for a tool the native build needs that is missing or failed (`E-INT-003`, PROG-CMP-05): an
/// environment problem, reported before any generated code is compiled, never a crash and never a source error.
pub fn toolchain_record(file: &str, what: &str) -> Record {
    Record::new(
        "E-INT-003",
        format!("the native toolchain could not run ({what}): `fortisc build` needs cargo and rustc of the supported Rust release, and the C linker driver, on PATH"),
        "PROG-CMP-05",
        file,
        1,
        1,
        Json::obj([("reproduction", Json::str(what))]),
    )
}

/// The path flags every native compilation gets: the toolchain root and Cargo's home remapped (PROG-NFR-22).
pub fn remap_flags(root: &Path) -> Vec<String> {
    let home = std::env::var("CARGO_HOME").unwrap_or_else(|_| format!("{}/.cargo", std::env::var("HOME").unwrap_or_default()));
    vec![format!("--remap-path-prefix={}=/fortis", root.display()), format!("--remap-path-prefix={home}=/cargo")]
}

/// Why the runtime crates are not available: their graph breaks PROG-CMP-06, or a tool the build needs (cargo,
/// rustc, the linker) is missing or failed before any generated code was compiled.
pub enum RuntimeError {
    Graph(String),
    Tool(String),
}

/// The runtime crates built once (PROG-CMP-06): the rlib of `fortis-native` and the directory of its
/// dependencies, after checking that their crate graph holds only the toolchain's path crates with no build script.
pub fn prebuilt_runtime(root: &Path) -> Result<(PathBuf, PathBuf), RuntimeError> {
    let manifest = root.join("compiler").join("crates").join("fortis-native").join("Cargo.toml");
    let problems = runtime_graph_problems(&manifest).map_err(|e| RuntimeError::Tool(format!("cargo metadata: {e}")))?;
    if !problems.is_empty() {
        return Err(RuntimeError::Graph(problems.join("; ")));
    }
    let target = root.join("build").join("native-runtime");
    let o = std::process::Command::new("cargo")
        .args(["build", "--release", "--offline", "--quiet", "-p", "fortis-native", "--manifest-path"])
        .arg(&manifest)
        .env("CARGO_TARGET_DIR", &target)
        .env("RUSTFLAGS", remap_flags(root).join(" "))
        .output()
        .map_err(|e| RuntimeError::Tool(format!("cargo: {e}")))?;
    if !o.status.success() {
        let why = String::from_utf8_lossy(&o.stderr).lines().find(|l| l.starts_with("error")).unwrap_or("the runtime did not build").to_string();
        return Err(RuntimeError::Tool(format!("cargo build of the runtime: {why}")));
    }
    Ok((target.join("release").join("libfortis_native.rlib"), target.join("release").join("deps")))
}

/// The packages `fortis-native` depends on (from `cargo metadata --offline`) that are not the toolchain's own path
/// crates, or have a build script; empty when the runtime's graph is clean.
pub fn runtime_graph_problems(manifest: &Path) -> Result<Vec<String>, String> {
    let o = std::process::Command::new("cargo").args(["metadata", "--offline", "--format-version", "1", "--manifest-path"]).arg(manifest).output().map_err(|e| e.to_string())?;
    if !o.status.success() {
        return Err(String::from_utf8_lossy(&o.stderr).lines().find(|l| l.contains("error")).unwrap_or("cargo metadata failed").to_string());
    }
    let j = Json::parse(&String::from_utf8_lossy(&o.stdout)).map_err(|e| format!("cargo metadata: {e}"))?;
    let s = |v: Option<&Json>| match v {
        Some(Json::Str(x)) => x.clone(),
        _ => String::new(),
    };
    let packages: BTreeMap<String, &Json> = match j.get("packages") {
        Some(Json::Arr(ps)) => ps.iter().map(|p| (s(p.get("id")), p)).collect(),
        _ => BTreeMap::new(),
    };
    let deps: BTreeMap<String, Vec<String>> = match j.get("resolve").and_then(|r| r.get("nodes")) {
        Some(Json::Arr(ns)) => ns.iter().map(|n| (s(n.get("id")), match n.get("dependencies") {
            Some(Json::Arr(d)) => d.iter().map(|x| s(Some(x))).collect(),
            _ => vec![],
        })).collect(),
        _ => BTreeMap::new(),
    };
    let start = packages.iter().find(|(_, p)| s(p.get("name")) == "fortis-native").map(|(id, _)| id.clone()).ok_or("fortis-native is not in the workspace")?;
    let crates_dir = manifest.parent().and_then(|p| p.parent()).map(|p| p.to_path_buf()).unwrap_or_default();
    let (mut seen, mut todo, mut out) = (BTreeSet::new(), vec![start], Vec::new());
    while let Some(id) = todo.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        let Some(p) = packages.get(&id) else { continue };
        let name = s(p.get("name"));
        if !matches!(p.get("source"), Some(Json::Null) | None) {
            out.push(format!("{name}: not a vendored path crate"));
        }
        if !Path::new(&s(p.get("manifest_path"))).starts_with(&crates_dir) {
            out.push(format!("{name}: outside the toolchain's crates"));
        }
        if let Some(Json::Arr(ts)) = p.get("targets") {
            if ts.iter().any(|t| matches!(t.get("kind"), Some(Json::Arr(k)) if k.contains(&Json::str("custom-build")))) {
                out.push(format!("{name}: has a build script"));
            }
        }
        todo.extend(deps.get(&id).cloned().unwrap_or_default());
    }
    Ok(out)
}

/// Put `bytes` at `out` without truncating a previous executable. An unchanged file is left in place and, on
/// Unix, made runnable if its mode was cleared. A new file is written to a sibling temporary, marked runnable,
/// then renamed over `out`, so a short write or a failed mode change leaves the previous executable (D-55).
pub fn publish_executable(out: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Ok(old) = std::fs::read(out) {
        if old == bytes {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = std::fs::metadata(out).map(|m| m.permissions().mode()).unwrap_or(0);
                if mode & 0o111 != 0o111 {
                    std::fs::set_permissions(out, std::fs::Permissions::from_mode(0o755)).map_err(|e| format!("the executable at {} could not be made runnable ({e})", out.display()))?;
                }
            }
            return Ok(());
        }
    }
    let name = out.file_name().and_then(|n| n.to_str()).unwrap_or("fortis-out");
    let parent = out.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    let tmp = parent.join(format!(".{name}.tmp"));
    if let Err(e) = std::fs::write(&tmp, bytes) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("the executable could not be written to {} ({e})", out.display()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)) {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("the executable at {} could not be made runnable ({e})", out.display()));
        }
    }
    if let Err(e) = std::fs::rename(&tmp, out) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("the executable could not be written to {} ({e})", out.display()));
    }
    Ok(())
}

/// rustc's arguments for the generated program: the release settings the lowering's Cargo.toml names, against the
/// prebuilt runtime, linked through `linker`.
pub fn rustc_args(root: &Path, crate_name: &str, main_rs: &Path, out: &Path, rlib: &Path, deps: &Path, linker: &Path) -> Vec<String> {
    let mut a: Vec<String> = vec![
        "--crate-name".into(),
        crate_name.into(),
        "--edition=2021".into(),
        "--crate-type".into(),
        "bin".into(),
        main_rs.display().to_string(),
        "-C".into(),
        "opt-level=3".into(),
        "-C".into(),
        "codegen-units=1".into(),
        "-C".into(),
        "panic=abort".into(),
        "-C".into(),
        "overflow-checks=on".into(),
        "-C".into(),
        "strip=debuginfo".into(),
        "-C".into(),
        format!("linker={}", linker.display()),
        "-L".into(),
        format!("dependency={}", deps.display()),
        "--extern".into(),
        format!("fortis_native={}", rlib.display()),
        "-o".into(),
        out.display().to_string(),
    ];
    a.extend(remap_flags(root));
    a
}

/// What a build measured and produced, as the manifest's JSON.
pub struct Manifest {
    pub program: String,
    pub target: String,
    pub features: BTreeSet<String>,
    pub inputs: Vec<(String, String)>,
    pub generated: Vec<(String, String)>,
    pub rewritten: Vec<String>,
    pub binary: Option<(String, u64, BTreeMap<String, u64>)>,
    pub allocation: BTreeMap<String, (bool, usize, usize)>,
    pub effects: BTreeMap<String, (Vec<String>, Vec<String>)>,
    pub trust: Json,
    pub timings: Vec<(&'static str, f64)>,
    pub compiler: String,
    pub stdlib: String,
}

impl Manifest {
    pub fn to_json(&self, previous: Option<&Json>) -> Json {
        let strmap = |v: &[(String, String)]| Json::Obj(v.iter().map(|(k, x)| (k.clone(), Json::str(x))).collect());
        let alloc = Json::Obj(
            self.allocation
                .iter()
                .map(|(f, (a, c, r))| (f.clone(), Json::obj([("allocates", Json::Bool(*a)), ("clones", Json::Int(*c as i64)), ("refcounts", Json::Int(*r as i64))])))
                .collect(),
        );
        let functions = Json::Obj(self.effects.iter().map(|(f, (e, a))| (f.clone(), Json::obj([("effects", Json::strs(e.clone())), ("authorities", Json::strs(a.clone()))]))).collect());
        let binary = match &self.binary {
            Some((sha, size, sections)) => Json::obj([
                ("sha256", Json::str(sha)),
                ("bytes", Json::Int(*size as i64)),
                ("sections", Json::Obj(sections.iter().filter(|(k, _)| [".text", ".rodata", ".data", ".bss", ".eh_frame"].contains(&k.as_str())).map(|(k, v)| (k.clone(), Json::Int(*v as i64))).collect())),
            ]),
            None => Json::Null,
        };
        let mut m = BTreeMap::new();
        m.insert("schema".to_string(), Json::str(SCHEMA));
        m.insert("program".into(), Json::str(&self.program));
        m.insert("target".into(), Json::obj([("triple", Json::str(&self.target)), ("feature_set", Json::strs(self.features.iter().cloned().collect::<Vec<_>>()))]));
        m.insert("toolchain".into(), Json::obj([("compiler", Json::str(&self.compiler)), ("stdlib", Json::str(&self.stdlib))]));
        m.insert("inputs".into(), strmap(&self.inputs));
        m.insert("generated".into(), strmap(&self.generated));
        m.insert("binary".into(), binary.clone());
        m.insert("allocation".into(), alloc.clone());
        m.insert("functions".into(), functions);
        m.insert("trust".into(), self.trust.clone());
        m.insert("rewritten".into(), Json::strs(self.rewritten.clone()));
        m.insert("timings".into(), Json::Obj(self.timings.iter().map(|(k, v)| (format!("{k}_ms"), Json::str(&format!("{v:.1}")))).collect()));
        m.insert("nondeterministic".into(), Json::strs(NONDETERMINISTIC.iter().map(|s| s.to_string()).collect::<Vec<_>>()));
        if let Some(prev) = previous {
            m.insert("changes".into(), changes(prev, &binary, &alloc));
        }
        Json::Obj(m)
    }
}

/// Code size and allocation deltas from the previous manifest (PROG-NFR-15).
fn changes(prev: &Json, binary: &Json, alloc: &Json) -> Json {
    let int = |j: Option<&Json>| match j {
        Some(Json::Int(i)) => *i,
        _ => 0,
    };
    let mut size = BTreeMap::new();
    size.insert("bytes".to_string(), Json::Int(int(binary.get("bytes")) - int(prev.get("binary").and_then(|b| b.get("bytes")))));
    if let Some(Json::Obj(secs)) = binary.get("sections") {
        for (k, v) in secs {
            let before = int(prev.get("binary").and_then(|b| b.get("sections")).and_then(|s| s.get(k)));
            size.insert(k.clone(), Json::Int(int(Some(v)) - before));
        }
    }
    let (Json::Obj(now), Some(Json::Obj(before))) = (alloc, prev.get("allocation")) else {
        return Json::obj([("code_size", Json::Obj(size)), ("allocation", Json::Arr(vec![]))]);
    };
    let mut alloc_changes = Vec::new();
    for f in now.keys().chain(before.keys()).collect::<BTreeSet<_>>() {
        let (a, b) = (now.get(f), before.get(f));
        if a != b {
            alloc_changes.push(Json::obj([("function", Json::str(f)), ("before", b.cloned().unwrap_or(Json::Null)), ("after", a.cloned().unwrap_or(Json::Null))]));
        }
    }
    Json::obj([("code_size", Json::Obj(size)), ("allocation", Json::Arr(alloc_changes))])
}

/// Compiler diagnostics from a cargo build's stderr, in the order rustc wrote them.
///
/// Cargo redraws its progress with a bare carriage return (`CARGO_TERM_PROGRESS_WHEN`, and the width).
/// `str::lines` does not split on a bare `\r`, so the first error is glued to the progress line and the lines
/// that do start with `error` are a later pair: the same failure then reads as a different diagnostic. Color
/// (`CARGO_TERM_COLOR`) prefixes the line with an ANSI sequence, and a wrapped line can be indented. Splitting
/// on `\r` as well, stripping that decoration, and skipping cargo's own summary (`could not compile`,
/// `aborting due to`) leaves the compiler's lines.
pub fn compiler_error_lines(stderr: &str) -> Vec<String> {
    stderr
        .split(['\n', '\r'])
        .map(|l| strip_ansi(l).trim().to_string())
        .filter(|l| l.starts_with("error") && !l.starts_with("error: could not compile") && !l.starts_with("error: aborting due to"))
        .collect()
}

/// Drop CSI color sequences (`ESC [ … m`). The diagnostic text, including non-ASCII, stays.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for d in chars.by_ref() {
                if d.is_ascii_alphabetic() {
                    break;
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_executable_has_a_text_section() {
        let exe = std::fs::read(std::env::current_exe().unwrap()).unwrap();
        let s = elf_sections(&exe);
        assert!(s.get(".text").copied().unwrap_or(0) > 0, "{s:?}");
        assert_eq!(elf_section_digests(&exe).len(), s.len());
    }

    #[test]
    fn a_build_script_is_found() {
        let d = std::env::temp_dir().join(format!("fortis-bs-{}", std::process::id()));
        std::fs::create_dir_all(d.join("native")).unwrap();
        std::fs::write(d.join("native").join("build.rs"), "fn main() {}").unwrap();
        assert_eq!(build_scripts(&[d.clone()]).len(), 1);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn cargo_progress_does_not_change_the_compiler_errors() {
        let body = "\
error[E0599]: no method named `area` found for reference `&T` in the current scope
  --> src/main.rs:31:16
   |
error[E0308]: mismatched types
  --> src/main.rs:31:25
error: could not compile `fx_ex11` (bin \"fx_ex11\") due to 2 previous errors
";
        let progress = "\
[                             ] 0/3: fortis-runtime              \r\
[========>                    ] 1/3: fortis-native               \r";
        let compiling_a = "   Compiling fortis-native v0.1.0 (/tmp/one/compiler/crates/fortis-native)\n   Compiling fx v0.1.0 (/tmp/one/build/native-prod/fx)\n";
        let compiling_b = "   Compiling fx v0.1.0 (/home/other/build/native-prod/fx)\n   Compiling fortis-runtime v0.1.0 (/var/tmp/rustc-pid/native)\n";
        let clean = compiler_error_lines(body);
        assert_eq!(compiler_error_lines(&format!("{progress}{body}")), clean);
        assert_eq!(compiler_error_lines(&format!("{compiling_a}{body}")), clean);
        assert_eq!(compiler_error_lines(&format!("{compiling_b}{progress}{body}")), clean);
        // indented, and colored: the same two diagnostics, and cargo's summary is not one of them
        let colored = "\u{1b}[1m\u{1b}[31m  error[E0599]: no method named `area` found for reference `&T` in the current scope\u{1b}[0m\n  error[E0308]: mismatched types\nerror: could not compile `fx_ex11` (bin \"fx_ex11\") due to 2 previous errors\n";
        assert_eq!(compiler_error_lines(colored), clean.iter().take(2).cloned().collect::<Vec<_>>());
        // the two lines a build-error summary keeps are the compiler's first two, not a progress-shifted pair
        assert_eq!(
            clean.iter().take(2).cloned().collect::<Vec<_>>().join(" / "),
            "error[E0599]: no method named `area` found for reference `&T` in the current scope / error[E0308]: mismatched types"
        );
    }

    #[test]
    fn publish_replaces_atomically_and_restores_the_executable_bit() {
        let d = std::env::temp_dir().join(format!("fortis-pub-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let out = d.join("prog");
        std::fs::write(&out, b"old").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&out, std::fs::Permissions::from_mode(0o644)).unwrap();
        }
        publish_executable(&out, b"old").unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b"old");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&out).unwrap().permissions().mode();
            assert_eq!(mode & 0o111, 0o111, "unchanged bytes still become runnable: {mode:#o}");
        }
        publish_executable(&out, b"new").unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b"new");
        assert!(!d.join(".prog.tmp").exists());
        std::fs::remove_dir_all(&d).unwrap();
    }
}
