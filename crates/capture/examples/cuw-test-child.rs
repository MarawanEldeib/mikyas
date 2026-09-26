//! Test helper for the `cuw-capture` integration tests; not part of the product (an example, so
//! it is never built into or installed with the shipped package).
//!
//! Echoes stdin to stdout byte for byte, then exits. Flags (any order):
//! - `--exit N`: exit code (default 0)
//! - `--no-read`: never touch stdin
//! - `--print-args`: after the echo, print `[args]` and then every argument on its own line
//! - `--pid-file PATH`: write this process id to PATH before anything else
//! - `--sleep-ms N`: sleep before exiting

use std::io::{self, Read, Write};
use std::time::Duration;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let value_of = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned();
    let has = |flag: &str| args.iter().any(|a| a == flag);

    if let Some(path) = value_of("--pid-file") {
        let _ = std::fs::write(path, std::process::id().to_string());
    }

    let mut out = io::stdout().lock();
    if !has("--no-read") {
        let mut input = Vec::new();
        let _ = io::stdin().read_to_end(&mut input);
        let _ = out.write_all(&input);
    }
    if has("--print-args") {
        let _ = out.write_all(b"[args]\n");
        for arg in &args {
            let _ = writeln!(out, "{arg}");
        }
    }
    let _ = out.flush();

    if let Some(ms) = value_of("--sleep-ms").and_then(|v| v.parse().ok()) {
        std::thread::sleep(Duration::from_millis(ms));
    }
    let code = value_of("--exit").and_then(|v| v.parse().ok()).unwrap_or(0);
    std::process::exit(code);
}
