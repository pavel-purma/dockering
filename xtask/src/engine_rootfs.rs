//! `cargo xtask engine-rootfs-pin-check` (REL-089): the engine root-filesystem pins agree with what was
//! published.
//!
//! `tools/engine-rootfs/pins.toml` is the single source of truth for the Alpine and Ubuntu images
//! (ENG-152). This command
//!
//! 1. checks its shape: every (image, arch) exists, SHA-256 is 64 lowercase hex digits, size > 0, the
//!    URL is `https://github.com/<repo>/releases/download/<rootfs-n>/<asset name>`, and a cell that is
//!    still `unpublished` is accepted only with `--allow-unpublished`;
//! 2. with `--online` (or `--release-json <file>`, a recorded API answer) compares every published
//!    cell with the assets of the GitHub release `rootfs-<n>`: URL, size and SHA-256 digest;
//! 3. compares the Rust pin table `crates/dk-wsl/src/setup/pins.rs` with the file, when that table
//!    exists. Each url, size and sha256 must sit together inside one `{ }` / `( )` entry of the table.
//!
//! Offline mode is what `ci.yml` runs; `release.yml` runs the `--online` form before it builds a draft.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::util::{Args, workspace_root};

const IMAGES: [&str; 2] = ["alpine", "ubuntu"];
const ARCHES: [&str; 2] = ["x86_64", "aarch64"];
const DEFAULT_PINS: &str = "tools/engine-rootfs/pins.toml";
const DEFAULT_RUST_TABLE: &str = "crates/dk-wsl/src/setup/pins.rs";
const USAGE: &str = "usage: cargo xtask engine-rootfs-pin-check [--online] [--allow-unpublished] \
[--pins <file>] [--rust-table <file>] [--release-json <file>]";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Published,
    Unpublished,
}

#[derive(Debug, Clone)]
struct Asset {
    status: Status,
    name: String,
    url: String,
    size: u64,
    sha256: String,
}

#[derive(Debug, Clone)]
struct Cell {
    image: String,
    arch: String,
    base_url: String,
    base_size: u64,
    base_sha256: String,
    asset: Asset,
}

#[derive(Debug, Clone)]
struct Pins {
    repo: String,
    tag: String,
    cells: Vec<Cell>,
}

pub fn run(args: &[String]) -> Result<()> {
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{USAGE}");
        return Ok(());
    }
    let parsed = Args::new(args);
    parsed.reject_unknown(
        &["--pins", "--rust-table", "--release-json"],
        &["--online", "--allow-unpublished"],
    )?;
    let root = workspace_root();
    let pins_path = parsed
        .value("--pins")?
        .map_or_else(|| root.join(DEFAULT_PINS), PathBuf::from);
    let allow_unpublished = parsed.flag("--allow-unpublished");
    let release_json = parsed.value("--release-json")?;
    let online = parsed.flag("--online") || release_json.is_some();

    let text = fs::read_to_string(&pins_path)
        .with_context(|| format!("reading {}", pins_path.display()))?;
    let pins = parse_pins(&text).with_context(|| format!("parsing {}", pins_path.display()))?;

    let mut problems = validate(&pins, allow_unpublished);
    let published = pins
        .cells
        .iter()
        .filter(|c| c.asset.status == Status::Published)
        .count();
    println!(
        "pins: {} cells, {} published, {} unpublished ({})",
        pins.cells.len(),
        published,
        pins.cells.len() - published,
        pins.tag
    );
    for p in &problems {
        println!("FAIL shape: {p}");
    }
    if problems.is_empty() {
        println!("ok   shape");
    }

    match parsed.value("--rust-table")? {
        Some(path) => {
            let source = fs::read_to_string(path).with_context(|| format!("reading {path}"))?;
            report(
                "rust table",
                check_rust_table(&pins, &source),
                &mut problems,
            );
        }
        None => {
            let path = root.join(DEFAULT_RUST_TABLE);
            if path.is_file() {
                let source = fs::read_to_string(&path)
                    .with_context(|| format!("reading {}", path.display()))?;
                report(
                    "rust table",
                    check_rust_table(&pins, &source),
                    &mut problems,
                );
            } else {
                println!(
                    "note {DEFAULT_RUST_TABLE} does not exist yet: skipping the Rust table comparison"
                );
            }
        }
    }

    if online {
        let release = match release_json {
            Some(path) => {
                let raw = fs::read_to_string(path).with_context(|| format!("reading {path}"))?;
                Some(serde_json::from_str::<Value>(&raw).context("parsing the release JSON")?)
            }
            None => fetch_release(&pins.repo, &pins.tag)?,
        };
        match release {
            Some(release) => report(
                "published release",
                compare_published(&pins, &release),
                &mut problems,
            ),
            None => {
                let found = absent_release(&pins);
                if found.is_empty() {
                    println!(
                        "note release {} does not exist and nothing is published in pins.toml: nothing to compare",
                        pins.tag
                    );
                }
                report("published release", found, &mut problems);
            }
        }
    }

    if !problems.is_empty() {
        bail!("{} engine rootfs pin problem(s)", problems.len());
    }
    println!("engine rootfs pins are consistent");
    Ok(())
}

fn report(what: &str, found: Vec<String>, all: &mut Vec<String>) {
    if found.is_empty() {
        println!("ok   {what}");
    }
    for p in found {
        println!("FAIL {what}: {p}");
        all.push(p);
    }
}

// ── parsing ──────────────────────────────────────────────────────────────────────────────────

fn str_of(table: &toml::Table, key: &str, path: &str) -> Result<String> {
    table
        .get(key)
        .and_then(toml::Value::as_str)
        .map(str::to_owned)
        .with_context(|| format!("{path}.{key} must be a string"))
}

fn size_of(table: &toml::Table, key: &str, path: &str) -> Result<u64> {
    let n = table
        .get(key)
        .and_then(toml::Value::as_integer)
        .with_context(|| format!("{path}.{key} must be an integer"))?;
    u64::try_from(n).with_context(|| format!("{path}.{key} must not be negative"))
}

fn table_of<'a>(table: &'a toml::Table, key: &str, path: &str) -> Result<&'a toml::Table> {
    table
        .get(key)
        .and_then(toml::Value::as_table)
        .with_context(|| format!("[{path}.{key}] is missing"))
}

fn parse_pins(text: &str) -> Result<Pins> {
    let root: toml::Table = text.parse().context("not valid TOML")?;
    let schema = root
        .get("schema")
        .and_then(toml::Value::as_integer)
        .unwrap_or(0);
    if schema != 1 {
        bail!("schema must be 1 (found {schema})");
    }
    let repo = str_of(&root, "repo", "")?;
    let tag = str_of(table_of(&root, "release", "")?, "tag", "release")?;
    let images = table_of(&root, "images", "")?;
    let mut cells = Vec::new();
    for (image, value) in images {
        let image_table = value
            .as_table()
            .with_context(|| format!("[images.{image}] must be a table"))?;
        let arches = table_of(image_table, "arch", &format!("images.{image}"))?;
        for (arch, cell) in arches {
            let path = format!("images.{image}.arch.{arch}");
            let cell = cell
                .as_table()
                .with_context(|| format!("[{path}] must be a table"))?;
            let asset = table_of(cell, "asset", &path)?;
            let apath = format!("{path}.asset");
            let status = match str_of(asset, "status", &apath)?.as_str() {
                "published" => Status::Published,
                "unpublished" => Status::Unpublished,
                other => bail!("{apath}.status must be published or unpublished (found `{other}`)"),
            };
            cells.push(Cell {
                image: image.clone(),
                arch: arch.clone(),
                base_url: str_of(cell, "base_url", &path)?,
                base_size: size_of(cell, "base_size", &path)?,
                base_sha256: str_of(cell, "base_sha256", &path)?,
                asset: Asset {
                    status,
                    name: str_of(asset, "name", &apath)?,
                    url: str_of(asset, "url", &apath)?,
                    size: size_of(asset, "size", &apath)?,
                    sha256: str_of(asset, "sha256", &apath)?,
                },
            });
        }
    }
    Ok(Pins { repo, tag, cells })
}

// ── shape ────────────────────────────────────────────────────────────────────────────────────

fn is_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn valid_repo(repo: &str) -> bool {
    let mut parts = repo.split('/');
    let (Some(owner), Some(name), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    [owner, name].iter().all(|p| {
        !p.is_empty()
            && p.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    })
}

fn valid_tag(tag: &str) -> bool {
    tag.strip_prefix("rootfs-").is_some_and(|n| {
        !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) && (n == "0" || !n.starts_with('0'))
    })
}

fn expected_name(image: &str, arch: &str) -> String {
    format!("dockering-engine-{image}-{arch}.tar.xz")
}

fn expected_url(pins: &Pins, name: &str) -> String {
    format!(
        "https://github.com/{}/releases/download/{}/{name}",
        pins.repo, pins.tag
    )
}

fn validate(pins: &Pins, allow_unpublished: bool) -> Vec<String> {
    let mut problems = Vec::new();
    if !valid_repo(&pins.repo) {
        problems.push(format!("repo `{}` is not <owner>/<name>", pins.repo));
    }
    if !valid_tag(&pins.tag) {
        problems.push(format!(
            "release tag `{}` is not rootfs-<n> (digits, no leading zero)",
            pins.tag
        ));
    }
    for image in IMAGES {
        for arch in ARCHES {
            if !pins
                .cells
                .iter()
                .any(|c| c.image == image && c.arch == arch)
            {
                problems.push(format!("missing [images.{image}.arch.{arch}]"));
            }
        }
    }
    let mut seen_sha: BTreeMap<&str, String> = BTreeMap::new();
    let mut seen_name = BTreeSet::new();
    for cell in &pins.cells {
        let id = format!("{}/{}", cell.image, cell.arch);
        if !IMAGES.contains(&cell.image.as_str()) || !ARCHES.contains(&cell.arch.as_str()) {
            problems.push(format!("{id}: unknown image or architecture"));
            continue;
        }
        if !cell.base_url.starts_with("https://") || cell.base_url.contains(char::is_whitespace) {
            problems.push(format!("{id}: base_url must be an https URL"));
        }
        if cell.base_size == 0 {
            problems.push(format!("{id}: base_size must be greater than 0"));
        }
        if !is_sha256(&cell.base_sha256) {
            problems.push(format!("{id}: base_sha256 must be 64 lowercase hex digits"));
        }
        let asset = &cell.asset;
        let name = expected_name(&cell.image, &cell.arch);
        if asset.name != name {
            problems.push(format!(
                "{id}: asset name is `{}`, expected `{name}`",
                asset.name
            ));
        }
        if !seen_name.insert(asset.name.as_str()) {
            problems.push(format!("{id}: asset name `{}` is used twice", asset.name));
        }
        let url = expected_url(pins, &name);
        if asset.url != url {
            problems.push(format!(
                "{id}: asset url is `{}`, expected `{url}`",
                asset.url
            ));
        }
        match asset.status {
            Status::Published => {
                if asset.size == 0 {
                    problems.push(format!("{id}: published asset size must be greater than 0"));
                }
                if !is_sha256(&asset.sha256) {
                    problems.push(format!(
                        "{id}: published asset sha256 must be 64 lowercase hex digits"
                    ));
                } else if let Some(other) = seen_sha.insert(&asset.sha256, id.clone()) {
                    problems.push(format!("{id}: same sha256 as {other}"));
                }
            }
            Status::Unpublished if !allow_unpublished => problems.push(format!(
                "{id}: asset is still `unpublished` (pass --allow-unpublished until the first rootfs release is published)"
            )),
            Status::Unpublished => {}
        }
    }
    problems
}

// ── published release ────────────────────────────────────────────────────────────────────────

/// Compares every published cell with the release's asset list (GitHub's `releases/tags/<tag>` JSON).
fn compare_published(pins: &Pins, release: &Value) -> Vec<String> {
    let mut problems = Vec::new();
    if release["tag_name"].as_str() != Some(pins.tag.as_str()) {
        problems.push(format!(
            "the release is `{}`, pins.toml says `{}`",
            release["tag_name"].as_str().unwrap_or("?"),
            pins.tag
        ));
    }
    if release["draft"].as_bool() != Some(false) {
        problems.push(format!(
            "release {} is a draft, so the app could not download from it",
            pins.tag
        ));
    }
    let assets: BTreeMap<&str, &Value> = release["assets"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|asset| Some((asset["name"].as_str()?, asset)))
                .collect()
        })
        .unwrap_or_default();
    for cell in pins
        .cells
        .iter()
        .filter(|c| c.asset.status == Status::Published)
    {
        let id = format!("{}/{}", cell.image, cell.arch);
        let want = &cell.asset;
        let Some(asset) = assets.get(want.name.as_str()) else {
            problems.push(format!(
                "{id}: release {} has no asset `{}`",
                pins.tag, want.name
            ));
            continue;
        };
        let url = asset["browser_download_url"].as_str().unwrap_or("");
        if url != want.url {
            problems.push(format!(
                "{id}: published url is `{url}`, pinned `{}`",
                want.url
            ));
        }
        match asset["size"].as_u64() {
            Some(size) if size == want.size => {}
            other => problems.push(format!(
                "{id}: published size is {}, pinned {}",
                other.map_or_else(|| "unknown".to_owned(), |s| s.to_string()),
                want.size
            )),
        }
        match asset["digest"].as_str() {
            Some(digest) if digest.eq_ignore_ascii_case(&format!("sha256:{}", want.sha256)) => {}
            Some(digest) => problems.push(format!(
                "{id}: published digest is `{digest}`, pinned `sha256:{}`",
                want.sha256
            )),
            None => problems.push(format!(
                "{id}: GitHub reports no digest for `{}`, so the pin cannot be verified",
                want.name
            )),
        }
    }
    problems
}

/// What an absent release (HTTP 404) means: a problem exactly when `pins.toml` publishes something.
fn absent_release(pins: &Pins) -> Vec<String> {
    let published = pins
        .cells
        .iter()
        .filter(|c| c.asset.status == Status::Published)
        .count();
    if published == 0 {
        return Vec::new();
    }
    vec![format!(
        "release {} does not exist (or is not visible), but pins.toml publishes {published} asset(s)",
        pins.tag
    )]
}

/// The release object for `tag`, or `None` when GitHub answers 404. Uses `gh` (authenticated), else curl.
fn fetch_release(repo: &str, tag: &str) -> Result<Option<Value>> {
    if !valid_repo(repo) || !valid_tag(tag) {
        bail!("refusing to query GitHub for repo `{repo}` tag `{tag}`");
    }
    let endpoint = format!("repos/{repo}/releases/tags/{tag}");
    match Command::new("gh").args(["api", &endpoint]).output() {
        Ok(out) if out.status.success() => {
            return Ok(Some(
                serde_json::from_slice(&out.stdout).context("parsing the release JSON from gh")?,
            ));
        }
        Ok(out) => {
            let err = String::from_utf8_lossy(&out.stderr);
            if err.contains("HTTP 404") || err.contains("Not Found") {
                return Ok(None);
            }
            bail!("gh api {endpoint} failed: {}", err.trim());
        }
        Err(_) => {} // no gh on this machine: curl below
    }
    let url = format!("https://api.github.com/{endpoint}");
    let mut cmd = Command::new("curl");
    cmd.args([
        "-sS",
        "--max-time",
        "60",
        "-H",
        "Accept: application/vnd.github+json",
    ]);
    if let Some(token) = std::env::var("GH_TOKEN")
        .ok()
        .or_else(|| std::env::var("GITHUB_TOKEN").ok())
        .filter(|t| !t.is_empty())
    {
        cmd.args(["-H", &format!("Authorization: Bearer {token}")]);
    }
    cmd.args(["--write-out", "\n%{http_code}", "--", &url]);
    let out = cmd
        .output()
        .context("running curl (and `gh` is not installed)")?;
    let body = String::from_utf8_lossy(&out.stdout);
    let (json, code) = body.rsplit_once('\n').unwrap_or(("", ""));
    match code.trim() {
        "200" => Ok(Some(
            serde_json::from_str(json).context("parsing the release JSON")?,
        )),
        "404" => Ok(None),
        other => bail!("GET {url} answered HTTP {other}"),
    }
}

// ── Rust pin table ───────────────────────────────────────────────────────────────────────────

/// `(start, end, content)` of every plain string literal (byte offsets of the quotes; escapes skipped).
fn string_literals(source: &str) -> Vec<(usize, usize, &str)> {
    let bytes = source.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            let start = i;
            i += 1;
            while i < bytes.len() && bytes[i] != b'"' {
                i += if bytes[i] == b'\\' { 2 } else { 1 };
            }
            let end = i.min(bytes.len().saturating_sub(1));
            out.push((start, end, &source[start + 1..end]));
        }
        i += 1;
    }
    out
}

/// The innermost `{ }`, `( )` or `[ ]` group around `pos`, as a byte range.
fn enclosing_group(source: &str, pos: usize) -> Option<(usize, usize)> {
    let bytes = source.as_bytes();
    let mut depth = 0usize;
    let mut start = None;
    for i in (0..pos).rev() {
        match bytes[i] {
            b')' | b'}' | b']' => depth += 1,
            b'(' | b'{' | b'[' if depth == 0 => {
                start = Some(i);
                break;
            }
            b'(' | b'{' | b'[' => depth -= 1,
            _ => {}
        }
    }
    let start = start?;
    let mut depth = 0usize;
    for (i, b) in bytes.iter().enumerate().skip(start + 1) {
        match b {
            b'(' | b'{' | b'[' => depth += 1,
            b')' | b'}' | b']' if depth == 0 => return Some((start, i + 1)),
            b')' | b'}' | b']' => depth -= 1,
            _ => {}
        }
    }
    None
}

/// Integer literals of `text` (underscores and a `u64`-style suffix allowed), string literals removed.
fn integers_outside_strings(text: &str) -> BTreeSet<u64> {
    let mut stripped = String::new();
    let mut last = 0;
    for (start, end, _) in string_literals(text) {
        stripped.push_str(&text[last..start]);
        stripped.push(' ');
        last = (end + 1).min(text.len());
    }
    stripped.push_str(&text[last..]);
    let bytes = stripped.as_bytes();
    let mut out = BTreeSet::new();
    let mut i = 0;
    while i < bytes.len() {
        let prev_ident = i > 0 && (bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_');
        if bytes[i].is_ascii_digit() && !prev_ident {
            let mut j = i;
            while j < bytes.len() && (bytes[j].is_ascii_digit() || bytes[j] == b'_') {
                j += 1;
            }
            if let Ok(n) = stripped[i..j].replace('_', "").parse::<u64>() {
                out.insert(n);
            }
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

/// Compares the Rust pin table with `pins.toml`. Every published cell's url, size and sha256 must sit
/// together in one entry; the table must pin no other rootfs asset and no unpublished one.
fn check_rust_table(pins: &Pins, source: &str) -> Vec<String> {
    let mut problems = Vec::new();
    let literals = string_literals(source);
    let rootfs_urls: Vec<&(usize, usize, &str)> = literals
        .iter()
        .filter(|(_, _, s)| {
            s.starts_with("https://github.com/") && s.contains("/releases/download/rootfs-")
        })
        .collect();
    let prefix = format!(
        "https://github.com/{}/releases/download/{}/",
        pins.repo, pins.tag
    );
    let mut found: BTreeSet<&str> = BTreeSet::new();
    for &&(start, _, url) in &rootfs_urls {
        // A fragment (`concat!(prefix, image, ..)`) can't be matched to a cell, but it must not point at
        // another repository or another release than pins.toml publishes.
        if !url.ends_with(".tar.xz") {
            if !url.starts_with(&prefix) {
                problems.push(format!(
                    "the Rust table builds urls from `{url}`, pins.toml says `{prefix}`"
                ));
            }
            continue;
        }
        let Some(cell) = pins.cells.iter().find(|c| c.asset.url == url) else {
            problems.push(format!(
                "the Rust table pins `{url}`, which pins.toml does not list"
            ));
            continue;
        };
        let id = format!("{}/{}", cell.image, cell.arch);
        if cell.asset.status != Status::Published {
            problems.push(format!(
                "{id}: the Rust table pins `{url}`, but pins.toml says it is unpublished"
            ));
            continue;
        }
        if !found.insert(url) {
            problems.push(format!("{id}: the Rust table pins `{url}` twice"));
            continue;
        }
        let Some((from, to)) = enclosing_group(source, start) else {
            problems.push(format!(
                "{id}: the url literal is not inside a `{{ }}` or `( )` entry with its size and sha256"
            ));
            continue;
        };
        let entry = &source[from..to];
        if !string_literals(entry)
            .iter()
            .any(|(_, _, s)| *s == cell.asset.sha256)
        {
            problems.push(format!(
                "{id}: the Rust entry has no sha256 `{}` (pins.toml)",
                cell.asset.sha256
            ));
        }
        if !integers_outside_strings(entry).contains(&cell.asset.size) {
            problems.push(format!(
                "{id}: the Rust entry has no size {} (pins.toml)",
                cell.asset.size
            ));
        }
    }
    for cell in pins
        .cells
        .iter()
        .filter(|c| c.asset.status == Status::Published)
    {
        if !found.contains(cell.asset.url.as_str()) {
            problems.push(format!(
                "{}/{}: pins.toml publishes `{}`, but the Rust table does not pin it as one literal url (with its size and sha256 in the same entry)",
                cell.image, cell.arch, cell.asset.url
            ));
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const REPO: &str = "pavel-purma/dockering";

    fn sha(n: usize) -> String {
        format!("{n:064x}")
    }

    fn cell_toml(published: bool, image: &str, arch: &str, n: usize) -> String {
        let (status, size, digest) = if published {
            ("published", 1000 + n, sha(n))
        } else {
            ("unpublished", 0, String::new())
        };
        format!(
            r#"
[images.{image}.arch.{arch}]
base_url = "https://example.org/{image}-{arch}.tar.gz"
base_size = 123456
base_sha256 = "{base}"
[images.{image}.arch.{arch}.asset]
status = "{status}"
name = "dockering-engine-{image}-{arch}.tar.xz"
url = "https://github.com/{REPO}/releases/download/rootfs-7/dockering-engine-{image}-{arch}.tar.xz"
size = {size}
sha256 = "{digest}"
"#,
            base = sha(900 + n)
        )
    }

    fn pins_toml(published: bool) -> String {
        let mut text = format!(
            "schema = 1\nrepo = \"{REPO}\"\n[release]\ntag = \"rootfs-7\"\n[images.alpine]\n[images.ubuntu]\n"
        );
        for (n, (image, arch)) in [
            ("alpine", "x86_64"),
            ("alpine", "aarch64"),
            ("ubuntu", "x86_64"),
            ("ubuntu", "aarch64"),
        ]
        .into_iter()
        .enumerate()
        {
            text.push_str(&cell_toml(published, image, arch, n + 1));
        }
        text
    }

    fn pins(published: bool) -> Pins {
        parse_pins(&pins_toml(published)).unwrap()
    }

    /// GitHub's `releases/tags/<tag>` shape for the sample pins.
    fn release_json(pins: &Pins) -> Value {
        let assets: Vec<Value> = pins
            .cells
            .iter()
            .map(|c| {
                json!({
                    "name": c.asset.name,
                    "browser_download_url": c.asset.url,
                    "size": c.asset.size,
                    "digest": format!("sha256:{}", c.asset.sha256),
                })
            })
            .collect();
        json!({ "tag_name": pins.tag, "draft": false, "assets": assets })
    }

    fn mentions(problems: &[String], needle: &str) -> bool {
        problems.iter().any(|p| p.contains(needle))
    }

    #[test]
    fn rel_089_committed_pins_file_has_a_valid_shape() {
        let text = fs::read_to_string(workspace_root().join(DEFAULT_PINS)).unwrap();
        let pins = parse_pins(&text).unwrap();
        assert_eq!(pins.cells.len(), 4, "alpine and ubuntu, x86_64 and aarch64");
        let problems = validate(&pins, true);
        assert!(problems.is_empty(), "{problems:?}");
        // The base pins are what the build verifies, so they are never placeholders.
        assert!(
            pins.cells
                .iter()
                .all(|c| c.base_size > 0 && is_sha256(&c.base_sha256))
        );
    }

    #[test]
    fn rel_089_shape_accepts_a_fully_published_table() {
        assert!(validate(&pins(true), false).is_empty());
    }

    #[test]
    fn rel_089_shape_unpublished_needs_the_flag() {
        let unpublished = pins(false);
        assert!(validate(&unpublished, true).is_empty());
        let strict = validate(&unpublished, false);
        assert_eq!(strict.len(), 4, "{strict:?}");
        assert!(mentions(&strict, "--allow-unpublished"));
    }

    #[test]
    fn rel_089_shape_requires_every_image_and_arch() {
        let mut p = pins(true);
        p.cells
            .retain(|c| !(c.image == "ubuntu" && c.arch == "aarch64"));
        assert!(mentions(
            &validate(&p, false),
            "missing [images.ubuntu.arch.aarch64]"
        ));
    }

    #[test]
    fn rel_089_shape_rejects_bad_digest_size_and_urls() {
        type Edit = Box<dyn Fn(&mut Cell)>;
        let edits: Vec<(&str, Edit, &str)> = vec![
            (
                "upper-case hex",
                Box::new(|c: &mut Cell| c.asset.sha256 = format!("{:064X}", 0xabc)),
                "64 lowercase hex",
            ),
            (
                "short hex",
                Box::new(|c| c.asset.sha256 = "abc".into()),
                "64 lowercase hex",
            ),
            (
                "zero size",
                Box::new(|c| c.asset.size = 0),
                "greater than 0",
            ),
            (
                "http",
                Box::new(|c| c.asset.url = c.asset.url.replacen("https", "http", 1)),
                "asset url",
            ),
            (
                "other host",
                Box::new(|c| c.asset.url = c.asset.url.replace("github.com", "evil.example")),
                "asset url",
            ),
            (
                "other tag",
                Box::new(|c| c.asset.url = c.asset.url.replace("rootfs-7", "rootfs-8")),
                "asset url",
            ),
            (
                "other repo",
                Box::new(|c| c.asset.url = c.asset.url.replace(REPO, "someone/else")),
                "asset url",
            ),
            (
                "other name",
                Box::new(|c| c.asset.name = "x.tar.xz".into()),
                "asset name",
            ),
            (
                "base sha",
                Box::new(|c| c.base_sha256 = String::new()),
                "base_sha256",
            ),
            ("base size", Box::new(|c| c.base_size = 0), "base_size"),
            (
                "base url",
                Box::new(|c| c.base_url = "http://example.org/x".into()),
                "base_url",
            ),
        ];
        for (what, edit, needle) in edits {
            let mut p = pins(true);
            edit(&mut p.cells[0]);
            let problems = validate(&p, false);
            assert!(mentions(&problems, needle), "{what}: {problems:?}");
        }
    }

    #[test]
    fn rel_089_shape_rejects_a_bad_tag_and_a_duplicate_digest() {
        let mut p = pins(true);
        p.tag = "rootfs-07".into();
        assert!(mentions(&validate(&p, false), "rootfs-<n>"));
        let mut p = pins(true);
        p.cells[1].asset.sha256 = p.cells[0].asset.sha256.clone();
        assert!(mentions(&validate(&p, false), "same sha256"));
    }

    #[test]
    fn rel_089_parse_rejects_a_wrong_schema_and_unknown_status() {
        assert!(parse_pins(&pins_toml(true).replace("schema = 1", "schema = 2")).is_err());
        assert!(parse_pins(&pins_toml(true).replace("\"published\"", "\"maybe\"")).is_err());
    }

    #[test]
    fn rel_089_published_release_matches() {
        let p = pins(true);
        let problems = compare_published(&p, &release_json(&p));
        assert!(problems.is_empty(), "{problems:?}");
    }

    #[test]
    fn rel_089_published_release_mismatch_is_detected() {
        let p = pins(true);
        type Edit = Box<dyn Fn(&mut Value)>;
        let edits: [(&str, Edit, &str); 7] = [
            (
                "size",
                Box::new(|r| r["assets"][0]["size"] = json!(1)),
                "published size is 1",
            ),
            (
                "digest",
                Box::new(|r| r["assets"][1]["digest"] = json!(format!("sha256:{}", sha(99)))),
                "published digest",
            ),
            (
                "url",
                Box::new(|r| {
                    r["assets"][2]["browser_download_url"] = json!("https://example.org/x")
                }),
                "published url",
            ),
            (
                "missing asset",
                Box::new(|r| {
                    r["assets"].as_array_mut().unwrap().remove(3);
                }),
                "has no asset",
            ),
            (
                "no digest",
                Box::new(|r| {
                    r["assets"][0].as_object_mut().unwrap().remove("digest");
                }),
                "no digest",
            ),
            (
                "draft",
                Box::new(|r| r["draft"] = json!(true)),
                "is a draft",
            ),
            (
                "other tag",
                Box::new(|r| r["tag_name"] = json!("rootfs-9")),
                "pins.toml says",
            ),
        ];
        for (what, edit, needle) in edits {
            let mut release = release_json(&p);
            edit(&mut release);
            let problems = compare_published(&p, &release);
            assert!(mentions(&problems, needle), "{what}: {problems:?}");
        }
    }

    #[test]
    fn rel_089_unpublished_cells_are_not_compared() {
        let p = pins(false);
        let release = json!({ "tag_name": p.tag, "draft": false, "assets": [] });
        assert!(compare_published(&p, &release).is_empty());
    }

    fn rust_table(p: &Pins) -> String {
        let mut s = String::from("pub const PINS: &[Pin] = &[\n");
        for c in &p.cells {
            s.push_str(&format!(
                "    Pin {{ image: \"{}\", arch: \"{}\", url: \"{}\", size: {}_{:03}, sha256: \"{}\" }},\n",
                c.image,
                c.arch,
                c.asset.url,
                c.asset.size / 1000,
                c.asset.size % 1000,
                c.asset.sha256
            ));
        }
        s.push_str("];\n");
        s
    }

    #[test]
    fn rel_089_rust_table_matches() {
        let p = pins(true);
        let problems = check_rust_table(&p, &rust_table(&p));
        assert!(problems.is_empty(), "{problems:?}");
    }

    #[test]
    fn rel_089_rust_table_mismatch_is_detected() {
        let p = pins(true);
        let table = rust_table(&p);
        let c = &p.cells[2];
        let wrong_sha = table.replace(&c.asset.sha256, &sha(77));
        assert!(mentions(&check_rust_table(&p, &wrong_sha), "no sha256"));
        let wrong_size = table.replace("1_003", "1_004");
        assert!(mentions(&check_rust_table(&p, &wrong_size), "no size 1003"));
        let other_tag = table.replacen("rootfs-7", "rootfs-8", 1);
        let problems = check_rust_table(&p, &other_tag);
        assert!(mentions(&problems, "does not list"), "{problems:?}");
        assert!(mentions(&problems, "does not pin it"), "{problems:?}");
        let missing: String = table
            .lines()
            .filter(|l| !l.contains("ubuntu") || !l.contains("aarch64"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(mentions(&check_rust_table(&p, &missing), "does not pin it"));
        let top_level = format!("const URL: &str = \"{}\";", c.asset.url);
        assert!(mentions(&check_rust_table(&p, &top_level), "not inside"));
    }

    #[test]
    fn rel_089_rust_table_pairs_fields_within_one_entry() {
        // A sha256 and a size from ANOTHER entry must not satisfy this one.
        let p = pins(true);
        let a = &p.cells[0];
        let b = &p.cells[1];
        let table = format!(
            "[Pin {{ url: \"{}\", size: {}, sha256: \"{}\" }}, Pin {{ url: \"{}\", size: {}, sha256: \"{}\" }}]",
            a.asset.url, b.asset.size, b.asset.sha256, b.asset.url, a.asset.size, a.asset.sha256
        );
        let problems = check_rust_table(&p, &table);
        assert!(mentions(&problems, "no sha256"), "{problems:?}");
        assert!(mentions(&problems, "no size"), "{problems:?}");
    }

    #[test]
    fn rel_089_rust_table_must_not_pin_an_unpublished_cell() {
        let p = pins(false);
        let table = format!(
            "Pin {{ url: \"{}\", size: 1, sha256: \"x\" }}",
            p.cells[0].asset.url
        );
        assert!(mentions(&check_rust_table(&p, &table), "unpublished"));
    }

    #[test]
    fn rel_089_rust_table_with_only_unpublished_cells_is_empty_and_fine() {
        let p = pins(false);
        assert!(check_rust_table(&p, "pub const PINS: &[Pin] = &[];").is_empty());
    }

    #[test]
    fn rel_089_rust_table_url_fragments_are_checked_for_repo_and_tag_only() {
        // The shape the table has before the first publish: `concat!(<prefix>, image, "-", arch, ".tar.xz")`.
        let p = pins(false);
        let ok = format!(
            "url: concat!(\"https://github.com/{REPO}/releases/download/rootfs-7/dockering-engine-\", $image)"
        );
        assert!(check_rust_table(&p, &ok).is_empty());
        let stale = ok.replace("rootfs-7", "rootfs-6");
        assert!(mentions(&check_rust_table(&p, &stale), "builds urls from"));
        let other_repo = ok.replace(REPO, "someone/else");
        assert!(mentions(
            &check_rust_table(&p, &other_repo),
            "builds urls from"
        ));
        // Once a cell is published, a fragment cannot stand for it.
        let published = pins(true);
        assert!(mentions(
            &check_rust_table(&published, &ok),
            "does not pin it"
        ));
    }

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "xtask-rootfs-pins-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn write(&self, name: &str, text: &str) -> String {
            let path = self.0.join(name);
            fs::write(&path, text).unwrap();
            path.to_string_lossy().into_owned()
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn rel_089_command_with_an_absent_rust_table_skips_it() {
        // The default table path does not exist in this checkout yet; the command must not fail for that.
        if workspace_root().join(DEFAULT_RUST_TABLE).is_file() {
            return;
        }
        let dir = Scratch::new();
        let pins_file = dir.write("pins.toml", &pins_toml(true));
        run(&args(&["--pins", &pins_file])).unwrap();
        // An explicitly named table that is absent is an error, not a skip.
        let missing = dir.0.join("nope.rs").to_string_lossy().into_owned();
        assert!(run(&args(&["--pins", &pins_file, "--rust-table", &missing])).is_err());
    }

    #[test]
    fn rel_089_command_end_to_end_with_a_recorded_release() {
        let dir = Scratch::new();
        let p = pins(true);
        let pins_file = dir.write("pins.toml", &pins_toml(true));
        let table = dir.write("pins.rs", &rust_table(&p));
        let release = dir.write("release.json", &release_json(&p).to_string());
        run(&args(&[
            "--pins",
            &pins_file,
            "--rust-table",
            &table,
            "--release-json",
            &release,
        ]))
        .unwrap();

        let mut bad = release_json(&p);
        bad["assets"][0]["size"] = json!(5);
        let bad_file = dir.write("bad.json", &bad.to_string());
        let err = run(&args(&[
            "--pins",
            &pins_file,
            "--rust-table",
            &table,
            "--release-json",
            &bad_file,
        ]))
        .unwrap_err();
        assert!(
            err.to_string().contains("1 engine rootfs pin problem"),
            "{err}"
        );
    }

    #[test]
    fn rel_089_command_refuses_unpublished_unless_allowed() {
        let dir = Scratch::new();
        let pins_file = dir.write("pins.toml", &pins_toml(false));
        let table = dir.write("pins.rs", "pub const PINS: &[Pin] = &[];");
        assert!(run(&args(&["--pins", &pins_file, "--rust-table", &table])).is_err());
        run(&args(&[
            "--pins",
            &pins_file,
            "--rust-table",
            &table,
            "--allow-unpublished",
        ]))
        .unwrap();
    }

    #[test]
    fn rel_089_absent_release_fails_only_when_something_is_published() {
        assert!(mentions(&absent_release(&pins(true)), "does not exist"));
        assert!(absent_release(&pins(false)).is_empty());
    }

    #[test]
    fn rel_089_literals_and_groups() {
        let src = r#"a("x\"y", [1, (2, "z")]) b"#;
        let lits = string_literals(src);
        assert_eq!(
            lits.iter().map(|l| l.2).collect::<Vec<_>>(),
            [r#"x\"y"#, "z"]
        );
        let z = lits[1].0;
        let (from, to) = enclosing_group(src, z).unwrap();
        assert_eq!(&src[from..to], r#"(2, "z")"#);
        assert_eq!(
            integers_outside_strings(r#"{ "9 99", size: 1_000u64, x: 0x10, id7: 5, n: 42 }"#),
            BTreeSet::from([0, 5, 42, 1000])
        );
    }
}
