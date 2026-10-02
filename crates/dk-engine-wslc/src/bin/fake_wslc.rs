//! Fake `wslc.exe` for the CLI transport contract tests (spec 21 §7).
//!
//! Replays recorded fixtures by argv. The map file `fake_map.json` in the fixture directory
//! (`$FAKE_WSLC_FIXTURES`, default `<crate>/tests/fixtures/cli`) is:
//!
//! ```json
//! { "sessions": ["wslc-cli-user"],
//!   "commands": { "container list --quiet": { "stdout_file": "…", "exit_code": 0 },
//!                 "container run --detach *": { "stdout": "…\n" } } }
//! ```
//!
//! Keys are the argv (without the program and without a leading `--session <s>`) joined by
//! single spaces. A key ending in ` *` matches that prefix followed by anything; the longest
//! match wins. Entry fields: `stdout` / `stdout_file`, `stderr` / `stderr_file`, `exit_code`
//! (default 0), `line_delay_ms` (sleep between stdout lines), `hang_ms` (sleep after output,
//! simulating a long-running child). `--session <s>` with `s` not in `sessions` fails like
//! wslc 3.0.1 (`WSLC_E_SESSION_NOT_FOUND`). Unknown argv → exit 99.

use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::Value;

fn fixtures_dir() -> PathBuf {
    std::env::var_os("FAKE_WSLC_FIXTURES")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests")
                .join("fixtures")
                .join("cli")
        })
}

fn fail(msg: &str, code: i32) -> ! {
    let _ = writeln!(std::io::stderr(), "{msg}");
    std::process::exit(code);
}

fn read(dir: &std::path::Path, file: &str) -> Vec<u8> {
    std::fs::read(dir.join(file))
        .unwrap_or_else(|e| fail(&format!("fake-wslc: can't read fixture {file}: {e}"), 98))
}

fn lookup<'a>(commands: &'a serde_json::Map<String, Value>, key: &str) -> Option<&'a Value> {
    if let Some(v) = commands.get(key) {
        return Some(v);
    }
    commands
        .iter()
        .filter_map(|(k, v)| {
            let prefix = k.strip_suffix(" *")?;
            (key == prefix || key.starts_with(&format!("{prefix} "))).then_some((k.len(), v))
        })
        .max_by_key(|(len, _)| *len)
        .map(|(_, v)| v)
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let dir = fixtures_dir();
    let map_text = std::fs::read_to_string(dir.join("fake_map.json"))
        .unwrap_or_else(|e| fail(&format!("fake-wslc: no fake_map.json: {e}"), 98));
    let map: Value = serde_json::from_str(&map_text)
        .unwrap_or_else(|e| fail(&format!("fake-wslc: bad fake_map.json: {e}"), 98));

    if args.first().map(String::as_str) == Some("--session") {
        let session = args.get(1).cloned().unwrap_or_default();
        args.drain(..args.len().min(2));
        let known = map
            .get("sessions")
            .and_then(Value::as_array)
            .is_some_and(|a| a.iter().any(|s| s.as_str() == Some(session.as_str())));
        if !known {
            fail(
                &format!(
                    "Session not found: '{session}'\r\nError code: WSLC_E_SESSION_NOT_FOUND\r\n\
                     If this error was unexpected, please consider searching for existing issues \
                     or filing a new issue at https://github.com/microsoft/WSL/issues."
                ),
                1,
            );
        }
    }

    let key = args.join(" ");
    let Some(commands) = map.get("commands").and_then(Value::as_object) else {
        fail("fake-wslc: fake_map.json has no \"commands\"", 98);
    };
    let Some(entry) = lookup(commands, &key) else {
        fail(&format!("fake-wslc: no fixture for: {key}"), 99);
    };

    let text = |k: &str| entry.get(k).and_then(Value::as_str);
    let ms = |k: &str| entry.get(k).and_then(Value::as_u64).unwrap_or(0);

    let stderr: Vec<u8> = match (text("stderr"), text("stderr_file")) {
        (Some(s), _) => s.as_bytes().to_vec(),
        (None, Some(f)) => read(&dir, f),
        (None, None) => Vec::new(),
    };
    let stdout: Vec<u8> = match (text("stdout"), text("stdout_file")) {
        (Some(s), _) => s.as_bytes().to_vec(),
        (None, Some(f)) => read(&dir, f),
        (None, None) => Vec::new(),
    };

    let mut out = std::io::stdout().lock();
    let delay = ms("line_delay_ms");
    if delay > 0 {
        for line in stdout.split_inclusive(|b| *b == b'\n') {
            let _ = out.write_all(line);
            let _ = out.flush();
            std::thread::sleep(Duration::from_millis(delay));
        }
    } else {
        let _ = out.write_all(&stdout);
    }
    let _ = out.flush();
    drop(out);
    let _ = std::io::stderr().write_all(&stderr);
    let _ = std::io::stderr().flush();

    let hang = ms("hang_ms");
    if hang > 0 {
        std::thread::sleep(Duration::from_millis(hang));
    }
    let code = entry.get("exit_code").and_then(Value::as_i64).unwrap_or(0);
    std::process::exit(i32::try_from(code).unwrap_or(1));
}
