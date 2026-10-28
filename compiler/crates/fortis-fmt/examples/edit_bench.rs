//! PROG-NFR-01 (DEL-43; benchmarks/PREREGISTRATION-DEL-43.md): warm parse and format of a 1k-line module, timed
//! per edit over a 1,000-edit session.
//!
//! `cargo run --release -p fortis-fmt --example edit_bench -- FILE [--passes 5] [--edits 1000]` prints one JSON line.

use std::time::Instant;

struct SplitMix(u64);

impl SplitMix {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// One edit of the preregistered script, applied to `lines` in place.
fn edit(k: usize, lines: &mut [String], rng: &mut SplitMix) {
    if k % 2 == 1 {
        // change one decimal integer literal to another value
        let text = lines.join("\n");
        let toks = fortis_syntax::lex(&text).expect("the edited module lexes");
        let ints: Vec<&fortis_syntax::Token> = toks.iter().filter(|t| t.kind == fortis_syntax::Kind::Int && t.text.chars().all(|c| c.is_ascii_digit())).collect();
        let t = ints[rng.below(ints.len())];
        let value: u64 = t.text.parse().unwrap_or(0);
        let new = (value + 1 + rng.below(97) as u64).to_string();
        let line = &mut lines[t.line - 1];
        let chars: Vec<char> = line.chars().collect();
        let (a, b) = (t.col - 1, t.col - 1 + t.text.chars().count());
        *line = chars[..a].iter().collect::<String>() + &new + &chars[b..].iter().collect::<String>();
    } else {
        // re-indent one non-blank line by two spaces
        let candidates: Vec<usize> = (0..lines.len()).filter(|i| !lines[*i].trim().is_empty()).collect();
        let i = candidates[rng.below(candidates.len())];
        if lines[i].starts_with("  ") && rng.next() & 1 == 1 {
            lines[i] = lines[i][2..].to_string();
        } else {
            lines[i] = format!("  {}", lines[i]);
        }
    }
}

fn quantile(sorted: &[f64], q: f64) -> f64 {
    let i = ((sorted.len() as f64 - 1.0) * q).round() as usize;
    sorted[i.min(sorted.len() - 1)]
}

fn summary(ms: &[f64]) -> String {
    let mut s = ms.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mean = s.iter().sum::<f64>() / s.len() as f64;
    format!(
        "{{\"edits\": {}, \"p50_ms\": {:.4}, \"p95_ms\": {:.4}, \"p99_ms\": {:.4}, \"mean_ms\": {:.4}, \"max_ms\": {:.4}}}",
        s.len(),
        quantile(&s, 0.50),
        quantile(&s, 0.95),
        quantile(&s, 0.99),
        mean,
        s[s.len() - 1]
    )
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let file = &args[0];
    let flag = |n: &str, d: usize| args.iter().position(|a| a == n).and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok()).unwrap_or(d);
    let (passes, edits) = (flag("--passes", 5), flag("--edits", 1000));
    let original = std::fs::read_to_string(file).expect("the workload");
    // `--components`: where the time goes (lex, parse, format), each the median of 200 runs; not the figure
    if args.iter().any(|a| a == "--components") {
        let med = |f: &dyn Fn()| {
            let mut v: Vec<f64> = (0..200).map(|_| {
                let t = Instant::now();
                f();
                t.elapsed().as_secs_f64() * 1e3
            }).collect();
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            v[100]
        };
        let lex = med(&|| { fortis_syntax::lex(&original).unwrap(); });
        let parse = med(&|| { fortis_syntax::parse_module(&original).unwrap(); });
        let format = med(&|| { fortis_fmt::format(&original).unwrap(); });
        println!("{{\"lex_ms\": {lex:.3}, \"parse_ms\": {parse:.3}, \"format_ms\": {format:.3}}}");
        return;
    }
    // warm: the process has parsed and formatted the workload 50 times
    for _ in 0..50 {
        fortis_syntax::parse_module(&original).expect("the workload parses");
        fortis_fmt::format(&original).expect("the workload formats");
    }
    let mut all = Vec::new();
    let mut per_pass = Vec::new();
    for _ in 0..passes {
        let mut rng = SplitMix(4301);
        let mut lines: Vec<String> = original.split('\n').map(|l| l.to_string()).collect();
        let mut ms = Vec::with_capacity(edits);
        for k in 1..=edits {
            edit(k, &mut lines, &mut rng);
            let text = lines.join("\n");
            let t0 = Instant::now();
            let parsed = fortis_syntax::parse_module(&text);
            let formatted = fortis_fmt::format(&text);
            let elapsed = t0.elapsed().as_secs_f64() * 1e3;
            if parsed.is_err() || formatted.is_err() {
                eprintln!("edit {k} does not parse or format: {:?}", formatted.err());
                std::process::exit(1);
            }
            ms.push(elapsed);
        }
        per_pass.push(summary(&ms));
        all.extend(ms);
    }
    let lines = original.lines().filter(|l| !l.trim().is_empty()).count();
    println!("{{\"lines\": {lines}, \"passes\": [{}], \"pooled\": {}}}", per_pass.join(", "), summary(&all));
}
