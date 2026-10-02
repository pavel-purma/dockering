//! `cargo xtask wslc-abi-check <tag|latest> [--against <vendored-tag>]` (spec 20 §5.7).
//!
//! Fetches `wslc.idl` + `WSLCShared.idl` for a WSL release tag from GitHub (curl argv; no
//! shell), normalises them (comments and whitespace stripped), and compares every interface
//! (IID + ordered method signatures) and every typedef'd struct/enum against the newest
//! vendored copy in `crates/dk-engine-wslc/idl/<tag>/`. Prints a report; exits with status 1
//! when the ABI changed, so the weekly CI job can open an issue.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

const IDL_FILES: [&str; 2] = ["wslc.idl", "WSLCShared.idl"];
const REPO: &str = "microsoft/WSL";
const IDL_DIR: &str = "src/windows/service/inc";

pub fn run(args: &[String]) -> Result<()> {
    let mut tag = None;
    let mut against = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--against" => against = it.next().cloned(),
            "-h" | "--help" => {
                println!(
                    "usage: cargo xtask wslc-abi-check <tag|latest> [--against <vendored-tag>]"
                );
                return Ok(());
            }
            s if tag.is_none() && !s.starts_with('-') => tag = Some(s.to_owned()),
            other => bail!("unexpected argument `{other}`"),
        }
    }
    let tag = tag.context("missing <tag|latest>")?;
    let tag = if tag == "latest" { latest_tag()? } else { tag };
    validate_tag(&tag)?;

    let idl_root = workspace_root().join("crates/dk-engine-wslc/idl");
    let base_tag = match against {
        Some(t) => t,
        None => newest_vendored(&idl_root)?,
    };
    println!("WSLC ABI check: WSL {tag} vs vendored {base_tag}");

    let mut changed = false;
    for f in IDL_FILES {
        let base = std::fs::read_to_string(idl_root.join(&base_tag).join(f))
            .with_context(|| format!("reading vendored {base_tag}/{f}"))?;
        let new = fetch(&tag, f)?;
        let report = compare(&parse_idl(&base), &parse_idl(&new));
        if report.is_empty() {
            println!("  {f}: no ABI change");
        } else {
            changed = true;
            println!("  {f}: ABI CHANGED");
            for line in report {
                println!("    {line}");
            }
        }
    }
    if changed {
        println!(
            "\nThe internal WSLC ABI differs. Vendor idl/{tag}/, add a com/abi module, run the live \
             suite on WSL {tag}; until then WSL {tag} uses the CLI fallback."
        );
        std::process::exit(1);
    }
    println!("\nNo ABI change: extend the verified range of the existing module after a live run.");
    Ok(())
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Tags are passed into URLs/argv: digits, dots, letters, dashes only.
fn validate_tag(tag: &str) -> Result<()> {
    if tag.is_empty()
        || tag.len() > 64
        || tag.starts_with('-')
        || !tag
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
    {
        bail!("invalid tag `{tag}`");
    }
    Ok(())
}

/// `idl/<tag>` directories sorted numerically; the newest wins.
fn newest_vendored(idl_root: &Path) -> Result<String> {
    let mut tags: Vec<(Vec<u64>, String)> = std::fs::read_dir(idl_root)
        .with_context(|| format!("listing {}", idl_root.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .map(|t| (version_key(&t), t))
        .collect();
    tags.sort();
    tags.pop()
        .map(|(_, t)| t)
        .context("no vendored IDL directory")
}

fn version_key(t: &str) -> Vec<u64> {
    t.split('.').map(|p| p.parse().unwrap_or(0)).collect()
}

fn curl(args: &[&str]) -> Result<String> {
    let out = Command::new("curl")
        .args(["-sSfL", "--max-time", "60"])
        .args(args)
        .output()
        .context("running curl")?;
    if !out.status.success() {
        bail!(
            "curl {:?} failed: {}",
            args.last().copied().unwrap_or_default(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Latest release tag: `gh api` when available (authenticated, higher rate limit), else the
/// public GitHub API via curl.
fn latest_tag() -> Result<String> {
    let gh = Command::new("gh")
        .args([
            "api",
            &format!("repos/{REPO}/releases/latest"),
            "--jq",
            ".tag_name",
        ])
        .output();
    if let Ok(o) = gh
        && o.status.success()
    {
        let t = String::from_utf8_lossy(&o.stdout).trim().to_owned();
        if !t.is_empty() {
            return Ok(t);
        }
    }
    let body = curl(&[
        "-H",
        "Accept: application/vnd.github+json",
        &format!("https://api.github.com/repos/{REPO}/releases/latest"),
    ])?;
    let v: serde_json::Value = serde_json::from_str(&body).context("GitHub API JSON")?;
    v.get("tag_name")
        .and_then(|t| t.as_str())
        .map(str::to_owned)
        .context("no tag_name in latest release")
}

fn fetch(tag: &str, file: &str) -> Result<String> {
    curl(&[&format!(
        "https://raw.githubusercontent.com/{REPO}/{tag}/{IDL_DIR}/{file}"
    )])
}

// ───────────────────────────── normalisation & parsing ─────────────────────────────

/// Strips `/* */` and `//` comments and collapses whitespace (so formatting and comment
/// edits never count as ABI changes).
pub fn normalise(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let b = src.as_bytes();
    let mut i = 0;
    let mut in_str = false;
    while i < b.len() {
        let c = b[i];
        if in_str {
            out.push(c as char);
            if c == b'\\' && i + 1 < b.len() {
                out.push(b[i + 1] as char);
                i += 2;
                continue;
            }
            if c == b'"' {
                in_str = false;
            }
            i += 1;
            continue;
        }
        match c {
            b'"' => {
                in_str = true;
                out.push('"');
                i += 1;
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                    i += 1;
                }
                i += 2;
                out.push(' ');
            }
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
                out.push('\n');
            }
            _ => {
                out.push(c as char);
                i += 1;
            }
        }
    }
    // Collapse whitespace runs to a single space, but keep `;`/`{`/`}` boundaries tight.
    let mut collapsed = String::with_capacity(out.len());
    let mut ws = false;
    for ch in out.chars() {
        if ch.is_whitespace() {
            ws = true;
        } else {
            if ws && !collapsed.is_empty() {
                let last = collapsed.chars().last().unwrap_or(' ');
                if !matches!(last, '(' | '[' | '{' | ';' | ',')
                    && !matches!(ch, ')' | ']' | '}' | ';' | ',')
                {
                    collapsed.push(' ');
                }
            }
            collapsed.push(ch);
            ws = false;
        }
    }
    collapsed
}

/// ABI-relevant declarations of one IDL file.
#[derive(Debug, Default, PartialEq)]
pub struct IdlAbi {
    /// interface name → (uuid, base, ordered method signatures)
    pub interfaces: BTreeMap<String, (String, String, Vec<String>)>,
    /// typedef name → ordered member list (struct fields / enum items)
    pub types: BTreeMap<String, Vec<String>>,
    /// `#define` / `cpp_quote("#define …")` constants
    pub defines: BTreeMap<String, String>,
}

pub fn parse_idl(src: &str) -> IdlAbi {
    let n = normalise(src);
    let mut abi = IdlAbi::default();

    // cpp_quote("#define NAME VALUE ...") and #define NAME VALUE
    for line in src.lines() {
        let l = line.trim();
        let l = l
            .strip_prefix("cpp_quote(\"")
            .and_then(|r| r.strip_suffix("\")"))
            .unwrap_or(l);
        if let Some(rest) = l.strip_prefix("#define ") {
            let rest = rest.split("/*").next().unwrap_or(rest);
            let rest = rest.split("//").next().unwrap_or(rest);
            let mut parts = rest.trim().splitn(2, char::is_whitespace);
            if let (Some(k), Some(v)) = (parts.next(), parts.next()) {
                abi.defines.insert(
                    k.to_owned(),
                    v.split_whitespace().collect::<Vec<_>>().join(" "),
                );
            }
        }
    }

    // interfaces: [ uuid(X) ... ] interface NAME : BASE { body };
    let mut rest = n.as_str();
    while let Some(pos) = rest.find("interface ") {
        let before = &rest[..pos];
        let uuid = before
            .rfind("uuid(")
            .and_then(|u| {
                before[u + 5..]
                    .find(')')
                    .map(|e| before[u + 5..u + 5 + e].to_ascii_uppercase())
            })
            .unwrap_or_default();
        let after = &rest[pos + "interface ".len()..];
        let Some(brace) = after.find('{') else { break };
        let header = after[..brace].trim();
        if header.ends_with(';') || !header.contains(':') {
            rest = &after[brace..];
            continue;
        }
        let (name, base) = header
            .split_once(':')
            .map(|(a, b)| (a.trim().to_owned(), b.trim().to_owned()))
            .unwrap_or_default();
        let Some(end) = matching_brace(&after[brace..]) else {
            break;
        };
        let body = &after[brace + 1..brace + end];
        let methods = body
            .split(';')
            .map(str::trim)
            .filter(|m| !m.is_empty())
            .map(str::to_owned)
            .collect();
        abi.interfaces.insert(name, (uuid, base, methods));
        rest = &after[brace + end..];
    }

    // typedef struct|enum [_Tag] { … } Name;  (unions inside structs stay part of the body)
    let mut rest = n.as_str();
    while let Some(pos) = rest.find("typedef ") {
        let after = &rest[pos + "typedef ".len()..];
        let Some(brace) = after.find('{') else { break };
        let semi = after.find(';').unwrap_or(usize::MAX);
        if semi < brace {
            // `typedef A B;` alias
            let decl = after[..semi].trim();
            if let Some((ty, name)) = decl.rsplit_once(' ') {
                abi.types.insert(name.to_owned(), vec![format!("= {ty}")]);
            }
            rest = &after[semi + 1..];
            continue;
        }
        let Some(end) = matching_brace(&after[brace..]) else {
            break;
        };
        let body = &after[brace + 1..brace + end];
        let tail = &after[brace + end + 1..];
        let name = tail.split(';').next().unwrap_or("").trim().to_owned();
        let kind = after[..brace]
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_owned();
        let sep = if kind == "enum" { ',' } else { ';' };
        let members = body
            .split(sep)
            .map(str::trim)
            .filter(|m| !m.is_empty())
            .map(|m| format!("{kind} {m}"))
            .collect();
        abi.types.insert(name, members);
        rest = tail;
    }
    abi
}

fn matching_brace(s: &str) -> Option<usize> {
    let mut depth = 0i32;
    for (i, c) in s.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// Human-readable differences; empty = ABI identical.
pub fn compare(base: &IdlAbi, new: &IdlAbi) -> Vec<String> {
    let mut out = Vec::new();
    for (name, (uuid, parent, methods)) in &base.interfaces {
        match new.interfaces.get(name) {
            None => out.push(format!("interface {name}: REMOVED")),
            Some((nu, np, nm)) => {
                if nu != uuid {
                    out.push(format!("interface {name}: IID {uuid} -> {nu}"));
                }
                if np != parent {
                    out.push(format!("interface {name}: base {parent} -> {np}"));
                }
                let n = methods.len().max(nm.len());
                for i in 0..n {
                    match (methods.get(i), nm.get(i)) {
                        (Some(a), Some(b)) if a == b => {}
                        (Some(a), Some(b)) => {
                            out.push(format!("interface {name} slot {i}: `{a}` -> `{b}`"))
                        }
                        (Some(a), None) => {
                            out.push(format!("interface {name} slot {i}: removed `{a}`"))
                        }
                        (None, Some(b)) => {
                            out.push(format!("interface {name} slot {i}: added `{b}`"))
                        }
                        (None, None) => {}
                    }
                }
            }
        }
    }
    for name in new.interfaces.keys() {
        if !base.interfaces.contains_key(name) {
            out.push(format!("interface {name}: ADDED"));
        }
    }
    for (name, members) in &base.types {
        match new.types.get(name) {
            None => out.push(format!("type {name}: REMOVED")),
            Some(nm) if nm != members => {
                out.push(format!("type {name}: members changed"));
                let n = members.len().max(nm.len());
                for i in 0..n {
                    let (a, b) = (members.get(i), nm.get(i));
                    if a != b {
                        out.push(format!(
                            "  [{i}] `{}` -> `{}`",
                            a.map_or("-", String::as_str),
                            b.map_or("-", String::as_str)
                        ));
                    }
                }
            }
            Some(_) => {}
        }
    }
    for name in new.types.keys() {
        if !base.types.contains_key(name) {
            out.push(format!("type {name}: ADDED"));
        }
    }
    for (k, v) in &base.defines {
        match new.defines.get(k) {
            None => out.push(format!("#define {k}: REMOVED")),
            Some(nv) if nv != v => out.push(format!("#define {k}: {v} -> {nv}")),
            Some(_) => {}
        }
    }
    for k in new.defines.keys() {
        if !base.defines.contains_key(k) {
            out.push(format!("#define {k}: ADDED"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vendored(f: &str) -> String {
        std::fs::read_to_string(
            workspace_root()
                .join("crates/dk-engine-wslc/idl/3.0.1")
                .join(f),
        )
        .expect("vendored idl")
    }

    #[test]
    fn parses_vendored_idl() {
        let abi = parse_idl(&vendored("wslc.idl"));
        let (uuid, base, methods) = &abi.interfaces["IWSLCSessionManager"];
        assert_eq!(uuid, "82A7ABC8-6B50-43FC-AB96-15FBBE7E8760");
        assert_eq!(base, "IUnknown");
        assert_eq!(methods.len(), 6);
        assert!(methods[5].contains("OpenSessionByName"), "{}", methods[5]);
        assert_eq!(abi.interfaces["IWSLCSession"].2.len(), 46);
        assert_eq!(abi.interfaces["IWSLCContainer"].2.len(), 20);
        assert_eq!(abi.interfaces["IWSLCProcess"].2.len(), 7);
        assert!(abi.types.contains_key("WSLCContainerEntry"));
        assert_eq!(abi.types["WSLCContainerEntry"].len(), 14);
        assert!(abi.defines.contains_key("WSLC_E_CONTAINER_DISABLED"));
        let shared = parse_idl(&vendored("WSLCShared.idl"));
        assert!(shared.types["WSLCContainerState"].len() == 5);
    }

    #[test]
    fn identical_and_reformatted_idl_has_no_changes() {
        let src = vendored("wslc.idl");
        assert!(compare(&parse_idl(&src), &parse_idl(&src)).is_empty());
        // Comment and whitespace edits are not ABI changes.
        let reformatted = src
            .replace(
                "    HRESULT Signal([in] int Signal);",
                "    // a comment\n    HRESULT   Signal( [in] int   Signal );",
            )
            .replace("/*++", "/* extra */ /*++");
        assert!(compare(&parse_idl(&src), &parse_idl(&reformatted)).is_empty());
    }

    #[test]
    fn detects_slot_insertion_struct_and_iid_change() {
        let src = vendored("wslc.idl");
        let moved = src.replacen(
            "    HRESULT GetState([out] WSLCProcessState* State, [out] int* Code);",
            "    HRESULT NewSlot();\n    HRESULT GetState([out] WSLCProcessState* State, [out] int* Code);",
            1,
        );
        let r = compare(&parse_idl(&src), &parse_idl(&moved));
        assert!(
            r.iter().any(|l| l.contains("IWSLCProcess slot 5")),
            "{r:#?}"
        );
        let field = src.replacen(
            "    LONGLONG SizeRw;",
            "    LONGLONG SizeRw;\n    ULONG Extra;",
            1,
        );
        let r = compare(&parse_idl(&src), &parse_idl(&field));
        assert!(r.iter().any(|l| l.contains("WSLCContainerEntry")), "{r:#?}");
        let iid = src.replacen(
            "82A7ABC8-6B50-43FC-AB96-15FBBE7E8760",
            "82A7ABC8-6B50-43FC-AB96-15FBBE7E8761",
            1,
        );
        let r = compare(&parse_idl(&src), &parse_idl(&iid));
        assert!(r.iter().any(|l| l.contains("IID")), "{r:#?}");
    }

    #[test]
    fn tag_validation() {
        assert!(validate_tag("3.0.1").is_ok());
        assert!(validate_tag("-x").is_err());
        assert!(validate_tag("3.0.1;rm").is_err());
        assert!(validate_tag("").is_err());
        assert!(version_key("2.10.0") > version_key("2.9.13"));
    }
}
