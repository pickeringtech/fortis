//! The Telemetry kit's in-process adapter (Platform 6.3; Language Reference 15; SP-09 design 3.1): the privacy
//! policy applied to captured values before any of them leaves the process, the record's identity and its schema
//! version, written as the canonical permitted record to the producer's channel. Implemented once for the VM and
//! native; the interpreter's `telemetry.py` is the same rule, byte for byte.
//!
//! `FORTIS_TELEMETRY` selects the mode: unset, the diagnostic plane withholds every field and nothing is written;
//! `permit-all`, every field is permitted unchanged on the record stream (the conformance corpus's plan); otherwise
//! the path of a compiled policy.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    Permit,
    Redact,
    Hash,
    Drop,
}

pub struct Policy {
    channel: Mutex<std::fs::File>,
    key_id: String,
    key: Vec<u8>,
    producer: String,
    /// (schema, version, field) -> (action, declared type)
    fields: HashMap<(String, u32, String), (Action, String)>,
    seq: AtomicU64,
}

pub enum Mode {
    Withhold,
    PermitAll,
    Policy(Policy),
}

static MODE: OnceLock<Mode> = OnceLock::new();

/// The adapter's mode, read once; called at program start so that opening the channel happens before any frame.
pub fn mode() -> &'static Mode {
    MODE.get_or_init(|| match std::env::var("FORTIS_TELEMETRY") {
        Err(_) => Mode::Withhold,
        Ok(v) if v.is_empty() => Mode::Withhold,
        Ok(v) if v == "permit-all" => Mode::PermitAll,
        Ok(path) => match load(&path) {
            Ok(p) => Mode::Policy(p),
            Err(e) => {
                // the policy is unusable: withhold everything, and say why without quoting the file
                let _ = writeln!(std::io::stderr(), "telemetry: policy {path}: {e}; every captured field is withheld");
                Mode::Withhold
            }
        },
    })
}

/// Whether the diagnostic plane writes captured values: only under `permit-all`.
pub fn diagnostic_values() -> bool {
    matches!(mode(), Mode::PermitAll)
}

fn load(path: &str) -> Result<Policy, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("unreadable ({})", e.kind()))?;
    parse(&text)
}

/// The policy's line format (design 2): a header, `channel`, `key`, an optional `producer`, and `field` lines.
pub fn parse(text: &str) -> Result<Policy, String> {
    let mut lines = text.lines().enumerate().filter(|(_, l)| !l.trim().is_empty() && !l.trim_start().starts_with('#'));
    match lines.next() {
        Some((_, l)) if l.trim() == "fortis-telemetry-policy 1" => {}
        _ => return Err("line 1: not a fortis-telemetry-policy 1 file".into()),
    }
    let (mut channel, mut key, mut producer, mut fields) = (None, None, None, HashMap::new());
    for (i, l) in lines {
        let ws: Vec<&str> = l.split_whitespace().collect();
        let bad = || format!("line {}: malformed `{}` line", i + 1, ws[0]);
        match ws[0] {
            "channel" if ws.len() == 2 => channel = Some(ws[1].to_string()),
            "key" if ws.len() == 3 => key = Some((ws[1].to_string(), unhex(ws[2]).filter(|k| k.len() == 32).ok_or_else(bad)?)),
            "producer" if ws.len() == 2 => producer = Some(unhex(ws[1]).filter(|p| p.len() == 16).map(|_| ws[1].to_ascii_lowercase()).ok_or_else(bad)?),
            "field" if ws.len() == 6 => {
                let version: u32 = ws[2].parse().map_err(|_| bad())?;
                let action = match ws[4] {
                    "permit" => Action::Permit,
                    "redact" => Action::Redact,
                    "hash" => Action::Hash,
                    "drop" => Action::Drop,
                    _ => return Err(bad()),
                };
                fields.insert((ws[1].to_string(), version, ws[3].to_string()), (action, ws[5].to_string()));
            }
            _ => return Err(bad()),
        }
    }
    let channel = channel.ok_or("no `channel` line")?;
    let (key_id, key) = key.ok_or("no `key` line")?;
    let producer = match producer {
        Some(p) => p,
        None => incarnation()?,
    };
    let file = std::fs::OpenOptions::new().create(true).append(true).open(&channel).map_err(|e| format!("channel unwritable ({})", e.kind()))?;
    Ok(Policy { channel: Mutex::new(file), key_id, key, producer, fields, seq: AtomicU64::new(0) })
}

/// A new producer incarnation: 128 bits from the operating system's random source (design 3.1.2).
fn incarnation() -> Result<String, String> {
    use std::io::Read;
    let mut b = [0u8; 16];
    std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b)).map_err(|_| "no random source for the producer incarnation".to_string())?;
    Ok(hex(&b))
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

pub fn hex(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for x in b {
        let _ = write!(s, "{x:02x}");
    }
    s
}

/// One observation leaving the process: a frame (`function` and `outcome` given) or an event.
pub struct Observation<'a> {
    pub schema: &'a str,
    pub version: u32,
    pub function: Option<&'a str>,
    pub outcome: Option<&'a str>,
    /// (field, canonical text), in any order
    pub fields: &'a [(&'a str, &'a str)],
    pub truncated: &'a [&'a str],
}

/// Write `o` to the channel as a canonical permitted record when a policy is in force (design 3.1.4).
pub fn observe(o: &Observation) {
    if let Mode::Policy(p) = mode() {
        let line = p.canonical(o);
        if let Ok(mut f) = p.channel.lock() {
            let _ = f.write_all(line.as_bytes());
        }
    }
}

impl Policy {
    pub fn canonical(&self, o: &Observation) -> String {
        let mut kept: Vec<(&str, String)> = Vec::new();
        let mut withheld = 0u32;
        for (name, text) in o.fields {
            match self.fields.get(&(o.schema.to_string(), o.version, name.to_string())) {
                None => withheld += 1,
                Some((Action::Drop, _)) => {}
                Some((Action::Permit, _)) => kept.push((name, text.to_string())),
                Some((Action::Redact, _)) => kept.push((name, "[redacted]".into())),
                Some((Action::Hash, ty)) => kept.push((name, keyed_hash(&self.key_id, &self.key, ty, text))),
            }
        }
        kept.sort_by(|a, b| a.0.cmp(b.0));
        let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        let mut s = String::from("{\"fields\": {");
        for (i, (k, v)) in kept.iter().enumerate() {
            if i > 0 {
                s.push_str(", ");
            }
            json_str(&mut s, k);
            s.push_str(": ");
            json_str(&mut s, v);
        }
        s.push('}');
        if let Some(f) = o.function {
            s.push_str(", \"function\": ");
            json_str(&mut s, f);
        }
        let _ = write!(s, ", \"id\": {{\"producer\": \"{}\", \"seq\": {seq}}}", self.producer);
        s.push_str(", \"kind\": ");
        json_str(&mut s, if o.function.is_some() { "observation-frame" } else { "observation-event" });
        if let Some(out) = o.outcome {
            s.push_str(", \"outcome\": ");
            json_str(&mut s, out);
        }
        s.push_str(", \"schema\": ");
        json_str(&mut s, o.schema);
        s.push_str(", \"truncated\": [");
        // that a value was cut to its bound says something about it: only a permitted field's cut is told
        let told: Vec<&&str> = o.truncated.iter().filter(|t| matches!(self.fields.get(&(o.schema.to_string(), o.version, t.to_string())), Some((Action::Permit, _)))).collect();
        for (i, t) in told.iter().enumerate() {
            if i > 0 {
                s.push_str(", ");
            }
            json_str(&mut s, t);
        }
        let _ = writeln!(s, "], \"version\": {}, \"withheld\": {withheld}}}", o.version);
        s
    }
}

/// A frame's bounded `String` fields (`String<scalars: 0..n>`) cut to their bound, in place; the names of the fields
/// cut, in capture order, which the frame records (Language Reference 13, LANG-13-03).
pub fn bound_fields(fields: &mut [(impl AsRef<str>, String)], bounds: &[(&str, usize)]) -> Vec<String> {
    let mut cut = Vec::new();
    for (name, text) in fields.iter_mut() {
        if let Some((_, n)) = bounds.iter().find(|(b, _)| *b == name.as_ref()) {
            if let Some((i, _)) = text.char_indices().nth(*n) {
                text.truncate(i);
                cut.push(name.as_ref().to_string());
            }
        }
    }
    cut
}

/// `h1:<key-id>:<first 16 bytes of HMAC-SHA256(key, "fortis.telemetry.hash.v1" 0 type 0 text), hex>` (design 3.1.3).
pub fn keyed_hash(key_id: &str, key: &[u8], ty: &str, text: &str) -> String {
    let mut msg = Vec::with_capacity(32 + ty.len() + text.len());
    msg.extend_from_slice(b"fortis.telemetry.hash.v1\0");
    msg.extend_from_slice(ty.as_bytes());
    msg.push(0);
    msg.extend_from_slice(text.as_bytes());
    format!("h1:{key_id}:{}", hex(&hmac_sha256(key, &msg)[..16]))
}

/// A JSON string as Python's `json.dumps` writes it (ASCII only; `\uXXXX` in lowercase, surrogate pairs above the
/// basic plane), so the three backends' canonical records are byte-identical.
pub fn json_str(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 || (c as u32) > 0x7e => {
                let mut buf = [0u16; 2];
                for unit in c.encode_utf16(&mut buf) {
                    let _ = write!(out, "\\u{unit:04x}");
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

// SHA-256 (FIPS 180-4) and HMAC (RFC 2104), without dependencies.

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
    0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
    0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
    0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut h: [u32; 8] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    for block in msg.chunks(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([block[4 * i], block[4 * i + 1], block[4 * i + 2], block[4 * i + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *x = x.wrapping_add(y);
        }
    }
    let mut out = [0u8; 32];
    for (i, x) in h.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&x.to_be_bytes());
    }
    out
}

pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..32].copy_from_slice(&sha256(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut inner: Vec<u8> = k.iter().map(|b| b ^ 0x36).collect();
    inner.extend_from_slice(msg);
    let mut outer: Vec<u8> = k.iter().map(|b| b ^ 0x5c).collect();
    outer.extend_from_slice(&sha256(&inner));
    sha256(&outer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_known_answers() {
        assert_eq!(hex(&sha256(b"")), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(hex(&sha256(b"abc")), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(hex(&sha256(&[b'a'; 1_000])), "41edece42d63e8d9bf515a9ba6932e1c20cbc9f5a5d134645adb5db1b9737ea3");
    }

    /// RFC 4231 test cases 1, 2, 3, 4, 6 and 7 (5 is a truncation of the same function)
    #[test]
    fn hmac_rfc4231() {
        let cases: [(Vec<u8>, Vec<u8>, &str); 6] = [
            (vec![0x0b; 20], b"Hi There".to_vec(), "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"),
            (b"Jefe".to_vec(), b"what do ya want for nothing?".to_vec(), "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"),
            (vec![0xaa; 20], vec![0xdd; 50], "773ea91e36800e46854db8ebd09181a72959098b3ef8c122d9635514ced565fe"),
            ((1..=25).collect(), vec![0xcd; 50], "82558a389a443c0ea4cc819899f2083a85f0faa3e578f8077a2e3ff46729665b"),
            (vec![0xaa; 131], b"Test Using Larger Than Block-Size Key - Hash Key First".to_vec(), "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"),
            (
                vec![0xaa; 131],
                b"This is a test using a larger than block-size key and a larger than block-size data. The key needs to be hashed before being used by the HMAC algorithm.".to_vec(),
                "9b09ffa71b942fcb27635fbcd5b0e944bfdc63644f0713938a7f51535c3a35e2",
            ),
        ];
        for (k, m, want) in cases {
            assert_eq!(hex(&hmac_sha256(&k, &m)), want);
        }
    }

    #[test]
    fn json_like_python() {
        let mut s = String::new();
        json_str(&mut s, "a\"b\\c\n\r\t\u{8}\u{c}\u{1}\u{7f}é😀");
        assert_eq!(s, "\"a\\\"b\\\\c\\n\\r\\t\\b\\f\\u0001\\u007f\\u00e9\\ud83d\\ude00\"");
        assert_eq!(keyed_hash("k", &[0; 32], "String", "y"), "h1:k:964e177c54a0e4a5397e6cf9a7e124ba");
    }

    #[test]
    fn canonical_record() {
        let dir = std::env::temp_dir().join(format!("fortis-tel-{}", std::process::id()));
        let text = format!(
            "fortis-telemetry-policy 1\nchannel {}\nkey k1 {}\nproducer {}\nfield S 2 a permit Int64\nfield S 2 b redact String\nfield S 2 c hash String\nfield S 2 d drop String\n",
            dir.display(),
            "00".repeat(32),
            "ab".repeat(16)
        );
        let p = parse(&text).unwrap();
        let o = Observation { schema: "S", version: 2, function: Some("f"), outcome: Some("success"), fields: &[("d", "x"), ("c", "y"), ("b", "z"), ("a", "1"), ("e", "w")], truncated: &[] };
        let line = p.canonical(&o);
        let h = keyed_hash("k1", &[0; 32], "String", "y");
        assert_eq!(
            line,
            format!("{{\"fields\": {{\"a\": \"1\", \"b\": \"[redacted]\", \"c\": \"{h}\"}}, \"function\": \"f\", \"id\": {{\"producer\": \"{}\", \"seq\": 1}}, \"kind\": \"observation-frame\", \"outcome\": \"success\", \"schema\": \"S\", \"truncated\": [], \"version\": 2, \"withheld\": 1}}\n", "ab".repeat(16))
        );
        let _ = std::fs::remove_file(dir);
    }
}
