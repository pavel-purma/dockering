//! Fake `docker.exe` and `Docker Desktop.exe` for the Docker Desktop start tests (ENG-144).
//!
//! A test copies this binary to `<install>\resources\bin\docker.exe` and
//! `<install>\Docker Desktop.exe`. Behaviour comes from a sibling file `<exe>.mode` with
//! `key=value` lines; every run appends its argv to `<exe>.log` (one line, args joined by `|`).
//!
//! `docker.exe desktop start ...` honours `start=`:
//! `ok` (default, exit 0), `already` (prints "Docker Desktop is already running", exit 0),
//! `unknown` (stderr "docker: unknown command: docker desktop", exit 1), `slow` (sleeps
//! `sleep_ms`, default 60000), `fail` (stderr `message=`, exit 1).
//! `docker.exe desktop status ...` prints `status_out=` (JSON text, `\n` escapes allowed),
//! exits `status_code=` (default 0) after `status_sleep_ms=`.
//! Anything else exits 99. `Docker Desktop.exe` only logs and exits 0.

use std::io::Write;
use std::time::Duration;

fn main() {
    let exe = std::env::current_exe().unwrap_or_default();
    let sibling = |ext: &str| {
        let mut name = exe.file_name().unwrap_or_default().to_os_string();
        name.push(ext);
        exe.with_file_name(name)
    };
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Ok(mut log) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(sibling(".log"))
    {
        let _ = writeln!(log, "{}", args.join("|"));
    }
    let mode = std::fs::read_to_string(sibling(".mode")).unwrap_or_default();
    let get = |key: &str| {
        mode.lines()
            .find_map(|l| l.strip_prefix(key)?.strip_prefix('='))
            .map(|v| v.replace("\\n", "\n"))
    };
    let num = |key: &str, default: u64| get(key).and_then(|v| v.parse().ok()).unwrap_or(default);
    let name = exe
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if name.starts_with("docker desktop") {
        return;
    }
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    match argv.as_slice() {
        ["desktop", "start", ..] => match get("start").as_deref().unwrap_or("ok") {
            "ok" => println!("Docker Desktop started"),
            "already" => println!("Docker Desktop is already running"),
            "unknown" => {
                eprintln!("docker: unknown command: docker desktop");
                std::process::exit(1);
            }
            "slow" => {
                std::thread::sleep(Duration::from_millis(num("sleep_ms", 60_000)));
            }
            _ => {
                eprintln!("{}", get("message").unwrap_or_else(|| "failed".into()));
                std::process::exit(1);
            }
        },
        ["desktop", "status", ..] => {
            std::thread::sleep(Duration::from_millis(num("status_sleep_ms", 0)));
            println!("{}", get("status_out").unwrap_or_default());
            std::process::exit(num("status_code", 0) as i32);
        }
        _ => std::process::exit(99),
    }
}
