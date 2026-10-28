//! The host capabilities with their development adapters (Language Reference 11.6, 15; DEL-47).
//!
//! A capability is a `Copy` handle (Appendix C.1: "zero-sized or handle-carrying struct with private constructor"),
//! passed by value like any trivial value, so a future that holds one owns what it holds and may be detached.
//! State a handle refers to (a file-system root, a random stream) lives for the program. Each adapter follows
//! interpreter/intrinsics.py so that native runs agree with the definitional interpreter, and each declares its
//! boundary in `compiler/crates/fortis-native/adapters/<Capability>.json` (ownership, effects, failures,
//! allocation, cancellation).

use std::cell::Cell;

use crate::failure::{Dyn, Failure, Outcome};
use crate::numeric::{Duration, Timestamp};
use crate::show::Show;

fn fail(code: &'static str) -> Failure {
    Failure::new(code, "")
}

fn leak_str(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

// ---- Console ----------------------------------------------------------------------------------------------------

/// Writes lines to standard output (effect `io`; no failure; no allocation: the text is written as it is).
#[derive(Clone, Copy, Debug, Default)]
pub struct Console(());

impl Console {
    #[inline]
    pub fn print<T: AsRef<str> + ?Sized>(&self, text: &T) {
        use std::io::Write;
        let mut out = std::io::stdout().lock();
        let _ = out.write_all(text.as_ref().as_bytes());
        let _ = out.write_all(b"\n");
    }
}

// ---- Clock --------------------------------------------------------------------------------------------------------

/// The wall clock, or a frozen one (`frozen(at)`): `now` is `nondet`; `sleep` suspends on the executor.
#[derive(Clone, Copy, Debug)]
pub struct Clock {
    frozen: Option<Timestamp>,
}

impl Clock {
    #[inline]
    pub fn now(&self) -> Timestamp {
        self.frozen.unwrap_or_else(|| Timestamp(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as i64).unwrap_or(0)))
    }
    #[inline]
    pub fn frozen(&self, at: Timestamp) -> Clock {
        Clock { frozen: Some(at) }
    }
    #[inline]
    pub fn elapsed_since(&self, t: Timestamp) -> Outcome<Duration> {
        let n = self.now();
        if t.0 > n.0 {
            return Err(fail("clock.future"));
        }
        Duration::of(n.0 - t.0, 1)
    }
}

// ---- Random -------------------------------------------------------------------------------------------------------

/// A splitmix64 stream seeded by `FORTIS_DEV_SEED` (or `seeded(n)`); `below` is uniform without modulo bias.
#[derive(Clone, Copy, Debug)]
pub struct Random {
    seed: &'static Cell<u64>,
}

impl Random {
    fn step(&self) -> u64 {
        let state = self.seed.get().wrapping_add(0x9E37_79B9_7F4A_7C15);
        self.seed.set(state);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// A uniform integer in [0, bound) by rejection sampling.
    #[inline]
    pub fn below(&self, bound: i64) -> i64 {
        if bound < 1 {
            crate::defect("contract_violation");
        }
        let b = bound as u128;
        let limit = ((1u128 << 64) / b) * b;
        loop {
            let z = self.step() as u128;
            if z < limit {
                return (z % b) as i64;
            }
        }
    }
    #[inline]
    pub fn seeded(&self, seed: i64) -> Random {
        Random { seed: Box::leak(Box::new(Cell::new(seed as u64))) }
    }
}

// ---- Network, Process, RawSql (development adapters: a closed world) ----------------------------------------------

#[derive(Clone, Copy, Debug, Default)]
pub struct Network(());

impl Network {
    /// Resolves `localhost` only; anything else fails `network.unreachable`.
    #[inline]
    pub fn resolve(&self, host: &str) -> Outcome<String> {
        if host == "localhost" {
            Ok("127.0.0.1".into())
        } else {
            Err(fail("network.unreachable").with("host", Dyn::Str(host.to_string())))
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Process(());

impl Process {
    /// Runs only `true` (0) and `false` (1); anything else fails `process.denied` (effect `block`).
    #[inline]
    pub fn run(&self, program: &str, _arguments: &[String]) -> Outcome<i64> {
        match program {
            "true" => Ok(0),
            "false" => Ok(1),
            _ => Err(fail("process.denied").with("program", Dyn::Str(program.to_string()))),
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RawSql(());

impl RawSql {
    /// No database is attached in development: every statement fails `sql.unavailable`.
    #[inline]
    pub fn execute(&self, _statement: &str) -> Outcome<i64> {
        Err(fail("sql.unavailable"))
    }
}

// ---- Environment, SecretAccess, Secret ----------------------------------------------------------------------------

/// A secret value: displays as a per-process fingerprint, never the value (15).
#[derive(Clone, Debug, PartialEq)]
pub struct Secret<T>(T);

impl<T: Show> Show for Secret<T> {
    fn show(&self) -> String {
        use std::hash::{BuildHasher, Hasher};
        thread_local! {
            static KEY: std::collections::hash_map::RandomState = std::collections::hash_map::RandomState::new();
        }
        let mut h = KEY.with(|k| k.build_hasher());
        h.write(self.0.show().as_bytes());
        format!("Secret({:08x})", h.finish() as u32)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Environment(());

impl Environment {
    /// A variable's value as a secret, or null for an absent or malformed name.
    #[inline]
    pub fn get(&self, name: &str) -> Option<Secret<String>> {
        if name.is_empty() || name.contains('=') {
            return None;
        }
        std::env::var(name).ok().map(Secret)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SecretAccess(());

/// `reveal(secret, cap)`: the value, with the authority to read it.
#[inline]
pub fn reveal<T: Clone>(secret: &Secret<T>, _cap: SecretAccess) -> T {
    secret.0.clone()
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Supervisor(());

// ---- FileSystem, File, Path -----------------------------------------------------------------------------------------

/// A path as written; `parse` refuses empty text and NUL.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Path(String);

impl Path {
    #[inline]
    pub fn parse(text: &str) -> Outcome<Path> {
        if text.is_empty() || text.contains('\0') {
            return Err(fail("path.invalid").with("text", Dyn::Str(text.to_string())));
        }
        Ok(Path(text.to_string()))
    }
}

impl Show for Path {
    fn show(&self) -> String {
        self.0.clone()
    }
}

/// A confinement to one directory: a path that leaves it fails `fs.outside_root`; a subtree that would leave it is
/// a confinement that refuses everything.
#[derive(Clone, Copy, Debug)]
pub struct FileSystem {
    root: Root,
}

/// Where a file system is rooted. The development root is made on first use, so a program that never touches the
/// file system creates no directory (as the interpreter's adapter does).
#[derive(Clone, Copy, Debug)]
enum Root {
    Dev,
    Dir(&'static str),
    Nothing,
}

/// The development root: `FORTIS_DEV_ROOT`, or a fresh directory under the system's temporary directory, which
/// the run removes when it ends (`release_dev_root`).
static DEV_ROOT: std::sync::OnceLock<(&'static str, bool)> = std::sync::OnceLock::new();

fn dev_root() -> &'static str {
    DEV_ROOT
        .get_or_init(|| {
            let (root, ours) = match std::env::var("FORTIS_DEV_ROOT") {
                Ok(r) => (r, false),
                Err(_) => (std::env::temp_dir().join(format!("fortis-dev-{}", std::process::id())).display().to_string(), true),
            };
            let _ = std::fs::create_dir_all(&root);
            (leak_str(root), ours)
        })
        .0
}

/// At the end of a run: remove the development root if this run created it (never a `FORTIS_DEV_ROOT`).
pub fn release_dev_root() {
    if let Some((root, true)) = DEV_ROOT.get() {
        let _ = std::fs::remove_dir_all(root);
    }
}

/// An open file (a resource, not a capability): an operating-system handle in read or write mode, so the file
/// stays readable when its name is removed; a closed file refuses both; dropping it closes the handle.
#[derive(Debug)]
pub struct File {
    handle: Option<std::fs::File>,
    write: bool,
}

/// An operating-system error on an open handle: outside every declared failure of the contract (the disk, not
/// the program), so it is a defect, never an undeclared failure.
fn os_defect<T>(_e: std::io::Error) -> T {
    crate::defect("runtime_invariant")
}

fn normalise(t: &str) -> Option<String> {
    let mut parts: Vec<&str> = vec![];
    for c in t.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            x => parts.push(x),
        }
    }
    Some(parts.join("/"))
}

impl FileSystem {
    fn resolve(&self, path: &Path) -> Outcome<String> {
        let outside = || Err(fail("fs.outside_root").with("path", Dyn::Str(path.0.clone())));
        let root = match self.root {
            Root::Dev => dev_root(),
            Root::Dir(r) => r,
            Root::Nothing => return outside(),
        };
        if path.0.starts_with('/') {
            return outside();
        }
        match normalise(&path.0) {
            Some(n) if n.is_empty() => Ok(root.to_string()),
            Some(n) => Ok(format!("{root}/{n}")),
            None => outside(),
        }
    }
    fn not_found(path: &Path) -> Failure {
        fail("fs.not_found").with("path", Dyn::Str(path.0.clone()))
    }

    pub fn read_text(&self, path: &Path) -> Outcome<String> {
        let full = self.resolve(path)?;
        let p = std::path::Path::new(&full);
        if !p.exists() {
            return Err(Self::not_found(path));
        }
        if !p.is_file() {
            return Err(fail("fs.not_a_file").with("path", Dyn::Str(path.0.clone())));
        }
        let bytes = std::fs::read(p).map_err(|_| Self::not_found(path))?;
        String::from_utf8(bytes).map_err(|_| fail("fs.invalid_text").with("path", Dyn::Str(path.0.clone())))
    }

    pub fn write_all(&self, path: &Path, text: &str) -> Outcome<()> {
        let full = self.resolve(path)?;
        let p = std::path::Path::new(&full);
        if !p.parent().map_or(true, |d| d.is_dir()) {
            return Err(Self::not_found(path));
        }
        std::fs::write(p, text).map_err(|_| Self::not_found(path))
    }

    pub fn read_all(&self, path: &Path) -> Outcome<Vec<u8>> {
        let full = self.resolve(path)?;
        let p = std::path::Path::new(&full);
        if !p.exists() {
            return Err(Self::not_found(path));
        }
        if !p.is_file() {
            return Err(fail("fs.not_a_file").with("path", Dyn::Str(path.0.clone())));
        }
        std::fs::read(p).map_err(|_| Self::not_found(path))
    }

    pub fn open(&self, path: &Path) -> Outcome<File> {
        let full = self.resolve(path)?;
        if !std::path::Path::new(&full).is_file() {
            return Err(Self::not_found(path));
        }
        let handle = std::fs::File::open(&full).map_err(|_| Self::not_found(path))?;
        Ok(File { handle: Some(handle), write: false })
    }

    pub fn create(&self, path: &Path) -> Outcome<File> {
        let full = self.resolve(path)?;
        let p = std::path::Path::new(&full);
        if !p.parent().map_or(true, |d| d.is_dir()) {
            return Err(Self::not_found(path));
        }
        let handle = std::fs::File::create(p).map_err(|_| Self::not_found(path))?;
        Ok(File { handle: Some(handle), write: true })
    }

    /// The confinement to a subdirectory (created); a path that leaves the root gives one that refuses everything.
    pub fn subtree(&self, path: &Path) -> FileSystem {
        match self.resolve(path) {
            Ok(full) => {
                let _ = std::fs::create_dir_all(&full);
                FileSystem { root: Root::Dir(leak_str(full)) }
            }
            Err(_) => FileSystem { root: Root::Nothing },
        }
    }
}

impl File {
    /// The remaining bytes (the position advances to the end).
    pub fn read_all(&mut self) -> Outcome<Vec<u8>> {
        use std::io::Read;
        let write = self.write;
        let h = self.handle.as_mut().ok_or_else(|| fail("fs.closed"))?;
        if write {
            return Err(fail("fs.wrong_mode"));
        }
        let mut rest = Vec::new();
        h.read_to_end(&mut rest).unwrap_or_else(os_defect);
        Ok(rest)
    }
    /// Appends the bytes.
    pub fn write_all(&mut self, data: &[u8]) -> Outcome<()> {
        use std::io::Write;
        let write = self.write;
        let h = self.handle.as_mut().ok_or_else(|| fail("fs.closed"))?;
        if !write {
            return Err(fail("fs.wrong_mode"));
        }
        h.write_all(data).unwrap_or_else(os_defect);
        Ok(())
    }
    /// Closes the handle; closing twice has no effect.
    pub fn close(&mut self) {
        self.handle = None;
    }
}

// ---- the capability set -------------------------------------------------------------------------------------------

/// `Host.capabilities()`: every development adapter (main only, 11.6). The fields carry the Fortis names.
#[derive(Clone, Copy, Debug)]
#[allow(non_snake_case)]
pub struct Capabilities {
    pub console: Console,
    pub clock: Clock,
    pub random: Random,
    pub network: Network,
    pub process: Process,
    pub environment: Environment,
    pub supervisor: Supervisor,
    pub secretAccess: SecretAccess,
    pub rawSql: RawSql,
    pub fileSystem: FileSystem,
}

/// Whether the process has created the capabilities (11.6; D-110).
static CAPABILITIES_MADE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Claim the one creation `flag` guards: true the first time, false on every later call.
fn first_creation(flag: &std::sync::atomic::AtomicBool) -> bool {
    !flag.swap(true, std::sync::atomic::Ordering::SeqCst)
}

/// The capabilities are created once per process (11.6): the checker admits one direct call, in the entry `main`;
/// a second creation means that guarantee is broken, which is the runtime-invariant defect (10.8; D-110).
pub fn capabilities() -> Capabilities {
    if !first_creation(&CAPABILITIES_MADE) {
        crate::defect("runtime_invariant");
    }
    let seed: u64 = std::env::var("FORTIS_DEV_SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(0);
    Capabilities {
        console: Console(()),
        clock: Clock { frozen: None },
        random: Random { seed: Box::leak(Box::new(Cell::new(seed))) },
        network: Network(()),
        process: Process(()),
        environment: Environment(()),
        supervisor: Supervisor(()),
        secretAccess: SecretAccess(()),
        rawSql: RawSql(()),
        fileSystem: FileSystem { root: Root::Dev },
    }
}

macro_rules! show_cap {
    ($($t:ident),*) => {$(
        impl Show for $t {
            fn show(&self) -> String {
                stringify!($t).into()
            }
        }
    )*};
}
show_cap!(Console, Clock, Random, Network, Process, Environment, Supervisor, SecretAccess, RawSql, FileSystem);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_stay_inside_the_root() {
        let fs = FileSystem { root: Root::Dir("/tmp/x") };
        assert_eq!(fs.resolve(&Path("a/b".into())).ok().as_deref(), Some("/tmp/x/a/b"));
        assert_eq!(fs.resolve(&Path("a/../b".into())).ok().as_deref(), Some("/tmp/x/b"));
        assert_eq!(fs.resolve(&Path("../b".into())).err().map(|f| f.code), Some("fs.outside_root"));
        assert_eq!(fs.resolve(&Path("/etc".into())).err().map(|f| f.code), Some("fs.outside_root"));
        assert_eq!(FileSystem { root: Root::Nothing }.resolve(&Path("a".into())).err().map(|f| f.code), Some("fs.outside_root"));
    }

    #[test]
    fn a_secret_never_displays_its_value() {
        let s = Secret(String::from("hunter2"));
        assert!(!s.show().contains("hunter2"));
        assert_eq!(s.show(), Secret(String::from("hunter2")).show());
        assert_eq!(reveal(&s, SecretAccess(())), "hunter2");
    }

    #[test]
    fn a_file_keeps_its_contract() {
        let dir = std::env::temp_dir().join(format!("fortis-host-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fs = FileSystem { root: Root::Dir(leak_str(dir.display().to_string())) };
        let p = Path::parse("notes.txt").unwrap();
        let mut w = fs.create(&p).unwrap();
        assert_eq!(w.read_all().err().map(|f| f.code), Some("fs.wrong_mode"));
        w.write_all(b"hello ").unwrap();
        w.write_all(b"world").unwrap();
        w.close();
        w.close();
        assert_eq!(w.write_all(b"!").err().map(|f| f.code), Some("fs.closed"));
        let mut r = fs.open(&p).unwrap();
        std::fs::remove_file(dir.join("notes.txt")).unwrap();
        assert_eq!(r.read_all().unwrap(), b"hello world");
        assert_eq!(r.read_all().unwrap(), b"");
        assert_eq!(r.write_all(b"x").err().map(|f| f.code), Some("fs.wrong_mode"));
        assert_eq!(fs.open(&p).err().map(|f| f.code), Some("fs.not_found"));
        let sub = fs.subtree(&Path::parse("../escape").unwrap());
        assert_eq!(sub.read_text(&p).err().map(|f| f.code), Some("fs.outside_root"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_second_creation_is_refused() {
        let flag = std::sync::atomic::AtomicBool::new(false);
        assert!(first_creation(&flag), "the first creation is admitted");
        assert!(!first_creation(&flag), "a second creation is refused");
        assert!(!first_creation(&flag), "and every later one");
    }

    #[test]
    fn capabilities_are_copy_handles() {
        let caps = capabilities();
        let console = caps.console;
        let again = caps.console;
        let _ = (console, again);
        assert_eq!(Path::parse("").err().map(|f| f.code), Some("path.invalid"));
    }
}
