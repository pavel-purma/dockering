//! REL-086: the app version has exactly one reader.
//!
//! Every production read of the package version goes through `dk_hub::app_version()`, which is
//! where the `update-e2e` test seam can override it. This scan fails on any other compile-time
//! read under `crates/*/src`, so a forgotten call site can't report a different version than
//! the updater compares (and sends in its `User-Agent`).
//!
//! It lives in xtask, not next to the code: `scripts/check-blocking.sh` scans whole files under
//! `crates/dockering/src`, tests included, and a scan needs file-system access.

use std::fs;
use std::path::{Path, PathBuf};

use crate::util::workspace_root;

/// The only file that may read the package version (and the build-time override).
const DEFINITION: &str = "crates/dk-hub/src/version.rs";

/// Compile-time reads that must stay inside [`DEFINITION`], written without whitespace
/// (`option_env!(…)` contains the first one, and the prefix also covers `…_MAJOR` and friends).
const NEEDLES: &[&str] = &["env!(\"CARGO_PKG_VERSION", "env!(\"DOCKERING_E2E_VERSION\""];

/// 1-based line numbers of direct reads in `source`. Comment-only lines are skipped. A read
/// split across lines (`env!(` and the name on different lines) is reported as line 0.
fn direct_reads(source: &str) -> Vec<usize> {
    let squeeze = |s: &str| s.split_whitespace().collect::<String>();
    let code: Vec<(usize, String)> = source
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim_start().starts_with("//"))
        .map(|(i, line)| (i + 1, squeeze(line)))
        .collect();
    let mut hits: Vec<usize> = code
        .iter()
        .filter(|(_, line)| NEEDLES.iter().any(|n| line.contains(n)))
        .map(|(n, _)| *n)
        .collect();
    let joined: String = code.iter().map(|(_, line)| line.as_str()).collect();
    let total: usize = NEEDLES.iter().map(|n| joined.matches(n).count()).sum();
    let found: usize = code
        .iter()
        .map(|(_, line)| {
            NEEDLES
                .iter()
                .map(|n| line.matches(n).count())
                .sum::<usize>()
        })
        .sum();
    if total > found {
        hits.push(0);
    }
    hits
}

/// Every `.rs` file under `dir`, sorted.
fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// `crates/*/src/**/*.rs` below `root`, as `(path relative to root with `/`, path)`.
fn crate_sources(root: &Path) -> Vec<(String, PathBuf)> {
    let mut crates: Vec<PathBuf> = fs::read_dir(root.join("crates"))
        .map(|d| d.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    crates.sort();
    let mut files = Vec::new();
    for krate in crates {
        rust_files(&krate.join("src"), &mut files);
    }
    files
        .into_iter()
        .map(|path| {
            let rel = path.strip_prefix(root).unwrap_or(&path);
            (rel.to_string_lossy().replace('\\', "/"), path)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_version_has_no_direct_uses() {
        let root = workspace_root();
        assert!(
            fs::read_to_string(root.join("Cargo.toml")).is_ok_and(|t| t.contains("[workspace]")),
            "{} is not the workspace root",
            root.display()
        );
        let sources = crate_sources(&root);
        // Guard against a scan that silently finds nothing.
        for expected in [
            DEFINITION,
            "crates/dockering/src/main.rs",
            "crates/dk-update/src/lib.rs",
        ] {
            assert!(
                sources.iter().any(|(rel, _)| rel == expected),
                "the scan did not visit {expected}"
            );
        }

        let mut offenders = Vec::new();
        let mut definition_reads = 0;
        for (rel, path) in &sources {
            let text = fs::read_to_string(path)
                .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
            let hits = direct_reads(&text);
            if rel == DEFINITION {
                definition_reads = hits.len();
            } else {
                offenders.extend(hits.into_iter().map(|line| format!("{rel}:{line}")));
            }
        }
        assert!(
            offenders.is_empty(),
            "read the version through dk_hub::app_version() (REL-086) instead of at:\n  {}",
            offenders.join("\n  ")
        );
        assert!(
            definition_reads > 0,
            "{DEFINITION} no longer reads the package version: where is app_version() defined?"
        );
    }

    #[test]
    fn app_version_scan_detects_direct_uses() {
        for bad in [
            "let v = env!(\"CARGO_PKG_VERSION\");",
            "let v = option_env!( \"CARGO_PKG_VERSION\" );",
            "let v = std::env!(\"CARGO_PKG_VERSION_MAJOR\");",
            "const V: &str = concat!(\"v\", env!(\"CARGO_PKG_VERSION\"));",
            "let v = option_env!(\"DOCKERING_E2E_VERSION\");",
        ] {
            assert_eq!(direct_reads(bad), vec![1], "{bad}");
        }
        // The line number is the right one, and a read split across lines is still caught.
        assert_eq!(
            direct_reads("fn f() {}\n\nlet v = env!(\"CARGO_PKG_VERSION\");\n"),
            vec![3]
        );
        assert_eq!(
            direct_reads("let v = env!(\n    \"CARGO_PKG_VERSION\"\n);"),
            vec![0]
        );
    }

    #[test]
    fn app_version_scan_ignores_comments_and_the_accessor() {
        for ok in [
            "let v = dk_hub::app_version();",
            "// env!(\"CARGO_PKG_VERSION\") is read in one place",
            "    /// see env!(\"CARGO_PKG_VERSION\")",
            "let v = std::env::var(\"CARGO_PKG_VERSION\");",
            "let n = env!(\"CARGO_PKG_NAME\");",
            "let v = env!(\"CARGO_MANIFEST_DIR\");",
        ] {
            assert!(direct_reads(ok).is_empty(), "{ok}");
        }
    }
}
