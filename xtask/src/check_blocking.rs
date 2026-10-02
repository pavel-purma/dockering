//! NFR-001: rejects blocking APIs in code that can run on the GPUI thread.
//! Cross-platform twin of `scripts/check-blocking.sh`; keep the two in sync.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};

use crate::util::workspace_root;

/// Source trees that run on (or may be called from) the UI thread.
pub const SCOPES: &[&str] = &["crates/dockering/src", "crates/dk-terminal/src/view"];

/// Substrings that indicate blocking I/O, process spawning, or sleeping.
pub const PATTERNS: &[&str] = &[
    "std::fs",
    "std::process",
    "std::net",
    "block_on",
    "thread::sleep",
];

/// A reviewed exception must carry a reason after the marker, on the same line.
pub const ALLOW_MARKER: &str = "nfr-001-allow:";

pub fn run(args: &[String]) -> anyhow::Result<()> {
    if let Some(arg) = args.first() {
        bail!("check-blocking takes no arguments (got `{arg}`)");
    }
    let root = workspace_root();
    let mut hits = Vec::new();
    for scope in SCOPES {
        let dir = root.join(scope);
        if dir.is_dir() {
            scan_dir(&dir, &root, &mut hits)?;
        }
    }

    if hits.is_empty() {
        println!("NFR-001 blocking-call check passed.");
        return Ok(());
    }
    for hit in &hits {
        eprintln!("{hit}");
    }
    eprintln!(
        "NFR-001 check failed: blocking API found in a UI-thread source scope.\n\
         Move the work behind HubHandle/background_spawn. Pre-window startup code may use\n\
         \"// {ALLOW_MARKER} <reason>\" on the same line after review."
    );
    std::process::exit(1);
}

fn scan_dir(dir: &Path, root: &Path, hits: &mut Vec<String>) -> anyhow::Result<()> {
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .map(|e| e.map(|e| e.path()))
        .collect::<Result<_, _>>()?;
    entries.sort();
    for path in entries {
        if path.is_dir() {
            scan_dir(&path, root, hits)?;
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            let text =
                fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
            let rel = path.strip_prefix(root).unwrap_or(&path);
            for (lineno, line) in scan_source(&text) {
                hits.push(format!(
                    "{}:{lineno}: {}",
                    rel.display().to_string().replace('\\', "/"),
                    line.trim()
                ));
            }
        }
    }
    Ok(())
}

/// Returns `(1-based line number, line)` for every offending line.
pub fn scan_source(text: &str) -> Vec<(usize, &str)> {
    text.lines()
        .enumerate()
        .filter(|(_, line)| PATTERNS.iter().any(|p| line.contains(p)) && !is_allowed(line))
        .map(|(i, line)| (i + 1, line))
        .collect()
}

fn is_allowed(line: &str) -> bool {
    line.find(ALLOW_MARKER)
        .is_some_and(|i| !line[i + ALLOW_MARKER.len()..].trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nfr_001_flags_blocking_calls() {
        let src = "use std::fs::read;\nlet x = 1;\nfutures::executor::block_on(f);\n\
                   std::thread::sleep(d);\nuse std::process::Command;\nstd::net::TcpStream::connect(a);";
        let lines: Vec<usize> = scan_source(src).into_iter().map(|(n, _)| n).collect();
        assert_eq!(lines, vec![1, 3, 4, 5, 6]);
    }

    #[test]
    fn nfr_001_allow_marker_needs_a_reason() {
        assert!(
            scan_source("let p = std::fs::read(x); // nfr-001-allow: pre-window config load")
                .is_empty()
        );
        assert_eq!(
            scan_source("let p = std::fs::read(x); // nfr-001-allow:").len(),
            1
        );
        assert_eq!(
            scan_source("let p = std::fs::read(x); // nfr-001-allow:   ").len(),
            1
        );
    }
}
