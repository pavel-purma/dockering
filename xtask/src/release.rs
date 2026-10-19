//! Release metadata (spec `features/distribution.md`):
//! - `update-manifest`: `dockering-update.json` from the built assets (UPD-002)
//! - `sign-manifest`: minisign signature `<file>.minisig` (UPD-003)
//! - `verify-manifest`: that signature against the public keys compiled into the app (UPD-003)
//! - `verify-update-artifacts`: the draft gate's checks on the update manifest (REL-084): made by the
//!   *current* key, the release's version, every entry equal to its asset
//! - `gen-update-keys`: one-time minisign key pair for the updater (UPD-003)
//! - the manifest, signing and key-file helpers are also used by `update_e2e` (REL-085), which
//!   writes the end-to-end fixture with a throwaway key through the same code
//! - `checksums`: `SHA256SUMS` over every asset (REL-012)
//! - `verify-assets`: require every distribution file before collecting metadata (REL-017)

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs;
use std::io::{Cursor, Read, Write as _};
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::package::{ArtifactKind, stable_name};
use crate::util::{Args, workspace_root};

/// The app's compiled-in manifest keys (`PUBLIC_KEYS`), relative to the workspace root.
pub const KEYS_SOURCE: &str = "crates/dk-update/src/keys.rs";

/// Six supported release targets and their mandatory package formats (REL-012/017).
const DISTRIBUTIONS: &[(&str, &[ArtifactKind])] = &[
    (
        "x86_64-pc-windows-msvc",
        &[ArtifactKind::Installer, ArtifactKind::Archive],
    ),
    (
        "aarch64-pc-windows-msvc",
        &[ArtifactKind::Installer, ArtifactKind::Archive],
    ),
    ("x86_64-apple-darwin", &[ArtifactKind::Dmg]),
    ("aarch64-apple-darwin", &[ArtifactKind::Dmg]),
    (
        "x86_64-unknown-linux-gnu",
        &[
            ArtifactKind::AppImage,
            ArtifactKind::Deb,
            ArtifactKind::Archive,
        ],
    ),
    (
        "aarch64-unknown-linux-gnu",
        &[
            ArtifactKind::AppImage,
            ArtifactKind::Deb,
            ArtifactKind::Archive,
        ],
    ),
];

/// `cargo xtask verify-assets --assets <dir> [--target <triple>]`.
/// This is deliberately separate from `update-manifest`, which supports partial local builds.
pub fn run_verify_assets(args: &[String]) -> anyhow::Result<()> {
    let parsed = Args::new(args);
    parsed.reject_unknown(&["--assets", "--target"], &[])?;
    let assets = Path::new(parsed.value("--assets")?.context("--assets is required")?);
    let count = verify_assets(assets, parsed.value("--target")?)?;
    println!(
        "xtask: verified {count} nonempty distribution assets in {}",
        assets.display()
    );
    Ok(())
}

fn required_assets(target: Option<&str>) -> anyhow::Result<BTreeSet<String>> {
    if let Some(target) = target
        && !DISTRIBUTIONS.iter().any(|(triple, _)| *triple == target)
    {
        bail!("unsupported release target `{target}`");
    }
    let mut names = BTreeSet::new();
    for (triple, kinds) in DISTRIBUTIONS {
        if target.is_some_and(|target| target != *triple) {
            continue;
        }
        for kind in *kinds {
            let name = stable_name(triple, *kind)?;
            if !names.insert(name.clone()) {
                bail!("duplicate required distribution name `{name}`");
            }
        }
    }
    Ok(names)
}

fn verify_assets(assets: &Path, target: Option<&str>) -> anyhow::Result<usize> {
    let required = required_assets(target)?;
    for name in &required {
        let path = assets.join(name);
        let metadata = fs::symlink_metadata(&path).with_context(|| {
            format!("required distribution asset {} is missing", path.display())
        })?;
        if !metadata.is_file() || metadata.len() == 0 {
            bail!(
                "required distribution asset {} must be a nonempty regular file",
                path.display()
            );
        }
    }
    for entry in fs::read_dir(assets)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if entry.file_type()?.is_file()
            && name.starts_with("Dockering-")
            && !required.contains(name)
        {
            bail!(
                "unexpected distribution asset `{name}` in {}",
                assets.display()
            );
        }
    }
    Ok(required.len())
}

/// Update-manifest platform key → (stable asset name, kind). Must match
/// `package::stable_name` (REL-012) and `dk-update`'s manifest model.
const PLATFORMS: &[(&str, &str, &str)] = &[
    ("windows-x86_64", "Dockering-Setup-x64.exe", "inno"),
    ("windows-aarch64", "Dockering-Setup-arm64.exe", "inno"),
    ("macos-aarch64", "Dockering-aarch64.dmg", "dmg"),
    ("macos-x86_64", "Dockering-x86_64.dmg", "dmg"),
    ("linux-x86_64", "Dockering-x86_64.AppImage", "appimage"),
    ("linux-aarch64", "Dockering-aarch64.AppImage", "appimage"),
];

/// Secret key text (the full `minisign.key` file content) for `sign-manifest`.
const KEY_ENV: &str = "UPDATE_SIGNING_KEY";
/// Password for an encrypted key (optional).
const KEY_PASSWORD_ENV: &str = "UPDATE_SIGNING_KEY_PASSWORD";
/// The update manifest asset (UPD-002).
pub(crate) const MANIFEST: &str = "dockering-update.json";

/// `cargo xtask update-manifest --assets <dir> --version <semver> [--repo owner/name] [--out <file>]`
pub fn run_update_manifest(args: &[String]) -> anyhow::Result<()> {
    let parsed = Args::new(args);
    parsed.reject_unknown(
        &["--assets", "--version", "--repo", "--out", "--pub-date"],
        &[],
    )?;
    let assets = PathBuf::from(parsed.value("--assets")?.context("--assets is required")?);
    let version = parsed
        .value("--version")?
        .context("--version is required")?;
    let version = version.strip_prefix('v').unwrap_or(version);
    let repo = parsed.value("--repo")?.unwrap_or("pavel-purma/dockering");
    let out = parsed
        .value("--out")?
        .map(PathBuf::from)
        .unwrap_or_else(|| assets.join("dockering-update.json"));
    let pub_date = match parsed.value("--pub-date")? {
        Some(d) => d.to_owned(),
        None => now_rfc3339()?,
    };
    let manifest = build_manifest(&assets, version, repo, &pub_date)?;
    fs::write(&out, manifest).with_context(|| format!("writing {}", out.display()))?;
    println!("xtask: wrote {}", out.display());
    Ok(())
}

fn build_manifest(
    assets: &Path,
    version: &str,
    repo: &str,
    pub_date: &str,
) -> anyhow::Result<String> {
    build_manifest_for(assets, version, repo, pub_date, None)
}

/// [`build_manifest`], optionally restricted to the one platform key `only` (the end-to-end
/// fixture has a single AppImage: the other platforms are neither looked for nor reported).
pub(crate) fn build_manifest_for(
    assets: &Path,
    version: &str,
    repo: &str,
    pub_date: &str,
    only: Option<&str>,
) -> anyhow::Result<String> {
    validate_version(version)?;
    if !repo
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/'))
    {
        bail!("invalid repo `{repo}`");
    }
    let mut platforms = Map::new();
    for (key, name, kind) in PLATFORMS {
        if only.is_some_and(|only| only != *key) {
            continue;
        }
        let path = assets.join(name);
        if !path.is_file() {
            eprintln!("xtask: {name} not found; `{key}` is left out of the manifest");
            continue;
        }
        let (sha256, size) = hash_file(&path)?;
        platforms.insert(
            (*key).to_owned(),
            json!({
                "kind": kind,
                "url": format!("https://github.com/{repo}/releases/download/v{version}/{name}"),
                "sha256": sha256,
                "size": size,
            }),
        );
    }
    if platforms.is_empty() {
        bail!("no release assets found in {}", assets.display());
    }
    let manifest = json!({
        "schema": 1,
        "version": version,
        "pub_date": pub_date,
        "notes_url": format!("https://github.com/{repo}/releases/tag/v{version}"),
        "platforms": Value::Object(platforms),
    });
    Ok(serde_json::to_string_pretty(&manifest)? + "\n")
}

/// `cargo xtask sign-manifest <file>`: reads the secret key from `$UPDATE_SIGNING_KEY`.
pub fn run_sign_manifest(args: &[String]) -> anyhow::Result<()> {
    let [file] = args else {
        bail!("usage: cargo xtask sign-manifest <file>");
    };
    let key_text = std::env::var(KEY_ENV).with_context(|| format!("${KEY_ENV} is not set"))?;
    let password = std::env::var(KEY_PASSWORD_ENV)
        .ok()
        .filter(|p| !p.is_empty());
    let sk = load_secret_key(&key_text, password)?;
    let data = fs::read(file).with_context(|| format!("reading {file}"))?;
    let sig = sign_bytes(&sk, &data)?;
    let out = format!("{file}.minisig");
    fs::write(&out, sig).with_context(|| format!("writing {out}"))?;
    println!("xtask: wrote {out}");
    Ok(())
}

/// Without a password the key must be unencrypted (what `gen-update-keys` writes); with one it must
/// be encrypted. Never prompts: a missing password for an encrypted key is an error.
pub(crate) fn load_secret_key(
    text: &str,
    password: Option<String>,
) -> anyhow::Result<minisign::SecretKey> {
    let boxed = minisign::SecretKeyBox::from_string(text)
        .map_err(|e| anyhow::anyhow!("invalid ${KEY_ENV}: {e}"))?;
    match password {
        Some(password) => boxed.into_secret_key(Some(password)).map_err(|e| {
            anyhow::anyhow!("${KEY_ENV} can't be opened with ${KEY_PASSWORD_ENV}: {e}")
        }),
        None => boxed.into_unencrypted_secret_key().map_err(|e| {
            anyhow::anyhow!("${KEY_ENV} is not an unencrypted key (set ${KEY_PASSWORD_ENV} if it is encrypted): {e}")
        }),
    }
}

pub(crate) fn sign_bytes(sk: &minisign::SecretKey, data: &[u8]) -> anyhow::Result<String> {
    let sig = minisign::sign(
        None,
        sk,
        Cursor::new(data),
        Some("dockering-update.json"),
        Some("signature from the Dockering release key"),
    )
    .map_err(|e| anyhow::anyhow!("signing failed: {e}"))?;
    Ok(sig.into_string())
}

/// `cargo xtask verify-manifest --assets <dir> [--keys <file>]`: checks `dockering-update.json.minisig`
/// against the public keys compiled into the app, with the app's own verifier (UPD-003).
pub fn run_verify_manifest(args: &[String]) -> anyhow::Result<()> {
    let parsed = Args::new(args);
    parsed.reject_unknown(&["--assets", "--keys"], &[])?;
    let assets = Path::new(parsed.value("--assets")?.context("--assets is required")?);
    let keys_path = parsed
        .value("--keys")?
        .map_or_else(|| workspace_root().join(KEYS_SOURCE), PathBuf::from);
    let source = fs::read_to_string(&keys_path)
        .with_context(|| format!("reading {}", keys_path.display()))?;
    let keys = parse_public_keys(&source)?;
    if keys.is_empty() {
        bail!(
            "{} has no public keys: the updater would reject every manifest (UPD-003)",
            keys_path.display()
        );
    }
    let signature = assets.join(format!("{MANIFEST}.minisig"));
    if !signature.is_file() {
        bail!("{} is missing: an unsigned release", signature.display());
    }
    let data = fs::read(assets.join(MANIFEST)).with_context(|| format!("reading {MANIFEST}"))?;
    let signature = fs::read_to_string(&signature).context("reading the signature")?;
    let index = verifying_key(&data, &signature, &keys)?;
    let role = if index == 0 { "current" } else { "next" };
    println!("xtask: {MANIFEST} verifies with public key #{index} ({role})");
    Ok(())
}

/// The string literals of `const PUBLIC_KEYS` in `crates/dk-update/src/keys.rs`; comments are skipped.
pub(crate) fn parse_public_keys(source: &str) -> anyhow::Result<Vec<String>> {
    let after = source
        .split_once("const PUBLIC_KEYS")
        .context("no `const PUBLIC_KEYS` in the keys source")?
        .1;
    let value = after.split_once('=').context("PUBLIC_KEYS has no value")?.1;
    let body = value.split_once('[').context("PUBLIC_KEYS has no `[`")?.1;
    let mut keys = Vec::new();
    let mut chars = body.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            ']' => return Ok(keys),
            '"' => {
                let mut key = String::new();
                loop {
                    match chars.next().context("unterminated string in PUBLIC_KEYS")? {
                        '"' => break,
                        '\\' => match chars.next().context("dangling escape in PUBLIC_KEYS")? {
                            'n' => key.push('\n'),
                            'r' => key.push('\r'),
                            't' => key.push('\t'),
                            other => key.push(other),
                        },
                        other => key.push(other),
                    }
                }
                keys.push(key);
            }
            '/' if chars.peek() == Some(&'/') => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                let mut previous = ' ';
                for c in chars.by_ref() {
                    if previous == '*' && c == '/' {
                        break;
                    }
                    previous = c;
                }
            }
            _ => {}
        }
    }
    bail!("PUBLIC_KEYS is not closed by `]`")
}

/// The base64 line of a public key given bare or as a whole `.pub` file (as `dk_update::verify`).
pub(crate) fn key_line(key: &str) -> &str {
    key.lines()
        .map(str::trim)
        .rfind(|l| !l.is_empty() && !l.starts_with("untrusted comment:"))
        .unwrap_or("")
}

/// Index of the first key that verifies `data`, using the verifier the app uses.
pub(crate) fn verifying_key(
    data: &[u8],
    signature: &str,
    keys: &[String],
) -> anyhow::Result<usize> {
    let signature = minisign_verify::Signature::decode(signature)
        .map_err(|e| anyhow::anyhow!("malformed signature: {e}"))?;
    keys.iter()
        .position(|key| {
            minisign_verify::PublicKey::from_base64(key_line(key))
                .is_ok_and(|pk| pk.verify(data, &signature, false).is_ok())
        })
        .context("no compiled-in public key verifies the manifest signature")
}

/// The channel a release is verified for (REL-083): `none` ships no updater, `all` ships it in
/// every target and therefore needs a signed manifest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Updater {
    None,
    All,
}

impl Updater {
    fn parse(value: &str) -> anyhow::Result<Self> {
        match value {
            "none" => Ok(Self::None),
            "all" => Ok(Self::All),
            other => bail!("--updater must be `none` or `all`, not `{other}` (REL-083)"),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::All => "all",
        }
    }
}

/// The repository the app's updater trusts (`dk_update::source::REPO`, UPD-001). `xtask` does not
/// depend on `dk-update`, so the URL pin below replicates `release_asset_name` exactly; a test
/// compares the two sources.
pub(crate) const UPDATER_REPO: &str = "pavel-purma/dockering";

/// The asset file name of `url` if it has the shape the app accepts (UPD-001):
/// `https://github.com/<UPDATER_REPO>/releases/download/v<version>/<plain name>`, where a plain name
/// is non-empty, does not start with a dot, and uses only ASCII letters, digits, `-`, `_` and `.`.
pub(crate) fn pinned_asset_name<'a>(url: &'a str, version: &str) -> Option<&'a str> {
    let prefix = format!("https://github.com/{UPDATER_REPO}/releases/download/v{version}/");
    url.strip_prefix(&prefix).filter(|name| {
        !name.is_empty()
            && !name.starts_with('.')
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    })
}

/// What `verify-update-artifacts` found: lines that passed and problems, all reported together.
#[derive(Default)]
struct Findings {
    passed: Vec<String>,
    problems: Vec<String>,
}

impl Findings {
    fn pass(&mut self, line: impl Into<String>) {
        self.passed.push(line.into());
    }

    fn fail(&mut self, line: impl Into<String>) {
        self.problems.push(line.into());
    }
}

/// `cargo xtask verify-update-artifacts --assets <dir> --version <X.Y.Z> [--keys <file>] [--updater none|all]`
///
/// The draft gate's update checks (REL-084), run by the `/release` skill and the signing rehearsal
/// next to `scripts/verify-release.ps1`, which has no minisign:
/// - the `.minisig`, when there is one, verifies against the compiled-in keys **and** was made by the
///   first (current) key, so a secret set to the rotation key fails; with `--updater all` it must exist;
///   with `--updater none` a missing one is allowed and nothing else about the signature is checked;
/// - the manifest `version` equals `--version`;
/// - every platform entry's kind, URL, size and SHA-256 match the file of that name in `--assets`;
/// - all six platform keys of REL-012 are present (and no other).
pub fn run_verify_update_artifacts(args: &[String]) -> anyhow::Result<()> {
    let parsed = Args::new(args);
    parsed.reject_unknown(&["--assets", "--keys", "--version", "--updater"], &[])?;
    let assets = Path::new(parsed.value("--assets")?.context("--assets is required")?);
    let version = parsed
        .value("--version")?
        .context("--version is required")?;
    let version = version.strip_prefix('v').unwrap_or(version);
    let updater = parsed
        .value("--updater")?
        .map_or(Ok(Updater::None), Updater::parse)?;
    let keys_path = parsed
        .value("--keys")?
        .map_or_else(|| workspace_root().join(KEYS_SOURCE), PathBuf::from);
    for line in verify_update_artifacts(assets, &keys_path, version, updater)? {
        println!("xtask: {line}");
    }
    Ok(())
}

fn verify_update_artifacts(
    assets: &Path,
    keys_path: &Path,
    version: &str,
    updater: Updater,
) -> anyhow::Result<Vec<String>> {
    validate_version(version)?;
    let manifest_path = assets.join(MANIFEST);
    let data =
        fs::read(&manifest_path).with_context(|| format!("reading {}", manifest_path.display()))?;
    let mut findings = Findings::default();

    check_manifest_signature(&data, assets, keys_path, updater, &mut findings);
    match serde_json::from_slice::<Value>(&data) {
        Ok(manifest) => {
            check_manifest_version(&manifest, version, &mut findings);
            check_manifest_entries(&manifest, assets, version, &mut findings);
        }
        Err(error) => findings.fail(format!("{MANIFEST} is not valid JSON: {error}")),
    }

    if findings.problems.is_empty() {
        findings.pass(format!(
            "update artifacts of v{version} are consistent (--updater {})",
            updater.as_str()
        ));
        return Ok(findings.passed);
    }
    let mut message = format!(
        "update artifacts in {} do not match release v{version} (--updater {}):",
        assets.display(),
        updater.as_str()
    );
    for problem in &findings.problems {
        message.push_str("\n  - ");
        message.push_str(problem);
    }
    bail!("{message}")
}

/// (a) of REL-084: the `.minisig` verifies and was made by the first (current) public key.
fn check_manifest_signature(
    data: &[u8],
    assets: &Path,
    keys_path: &Path,
    updater: Updater,
    findings: &mut Findings,
) {
    let signature_path = assets.join(format!("{MANIFEST}.minisig"));
    if !signature_path.is_file() {
        match updater {
            Updater::All => findings.fail(format!(
                "{MANIFEST}.minisig is missing: a build with the updater (--updater all) needs a signed manifest (REL-083)"
            )),
            Updater::None => findings.pass(format!(
                "no {MANIFEST}.minisig (--updater none): the signature is not checked"
            )),
        }
        return;
    }
    let signature = match fs::read_to_string(&signature_path) {
        Ok(text) => text,
        Err(error) => {
            findings.fail(format!("reading {}: {error}", signature_path.display()));
            return;
        }
    };
    let keys = match fs::read_to_string(keys_path)
        .with_context(|| format!("reading {}", keys_path.display()))
        .and_then(|source| parse_public_keys(&source))
    {
        Ok(keys) if keys.is_empty() => {
            findings.fail(format!(
                "{} has no public keys: the updater would reject every manifest (UPD-003)",
                keys_path.display()
            ));
            return;
        }
        Ok(keys) => keys,
        Err(error) => {
            findings.fail(format!("{error:#}"));
            return;
        }
    };
    if minisign_verify::PublicKey::from_base64(key_line(&keys[0])).is_err() {
        findings.fail(
            "the first (current) public key in keys.rs is not a valid minisign public key"
                .to_owned(),
        );
        return;
    }
    match verifying_key(data, &signature, &keys) {
        Ok(0) => findings.pass(format!(
            "{MANIFEST}.minisig verifies with the current public key (#0)"
        )),
        Ok(index) => findings.fail(format!(
            "{MANIFEST}.minisig was made by public key #{index}, not the current key (#0): the secret {KEY_ENV} is not the current key (rotate as in crates/dk-update/keys/README.md)"
        )),
        Err(error) => findings.fail(format!(
            "{MANIFEST}.minisig does not verify against the public keys in {}: {error:#}",
            keys_path.display()
        )),
    }
}

/// (b) of REL-084: schema 1 and `version` equals the release's version.
fn check_manifest_version(manifest: &Value, version: &str, findings: &mut Findings) {
    match manifest.get("schema").and_then(Value::as_u64) {
        Some(1) => findings.pass("manifest schema is 1".to_owned()),
        other => findings.fail(format!(
            "manifest schema is {other:?}, expected 1 (UPD-002)"
        )),
    }
    match manifest.get("version").and_then(Value::as_str) {
        Some(found) if found == version => {
            findings.pass(format!("manifest version is {version}"));
        }
        Some(found) => findings.fail(format!(
            "manifest version is `{found}`, but the release is `{version}`"
        )),
        None => findings.fail("manifest has no `version` string".to_owned()),
    }
}

/// (c) and (d) of REL-084: all six platform entries, each equal to its asset in `assets`.
fn check_manifest_entries(manifest: &Value, assets: &Path, version: &str, findings: &mut Findings) {
    let Some(platforms) = manifest.get("platforms").and_then(Value::as_object) else {
        findings.fail("manifest has no `platforms` object".to_owned());
        return;
    };
    for (key, name, kind) in PLATFORMS {
        let Some(entry) = platforms.get(*key) else {
            findings.fail(format!(
                "platform `{key}` is missing from the manifest (REL-012 lists six platforms)"
            ));
            continue;
        };
        let mismatches = entry_mismatches(entry, assets, version, name, kind);
        if mismatches.is_empty() {
            findings.pass(format!(
                "platform `{key}`: {name} ({kind}) matches kind, URL, size and SHA-256"
            ));
        } else {
            findings.fail(format!(
                "platform `{key}` ({name}): {}",
                mismatches.join("; ")
            ));
        }
    }
    for key in platforms.keys() {
        if !PLATFORMS.iter().any(|(known, ..)| known == key) {
            findings.fail(format!(
                "unexpected platform `{key}` in the manifest: no REL-012 asset can be checked against it"
            ));
        }
    }
}

/// What differs between one manifest entry and the asset it names; empty means they agree.
fn entry_mismatches(
    entry: &Value,
    assets: &Path,
    version: &str,
    name: &str,
    kind: &str,
) -> Vec<String> {
    let mut problems = Vec::new();
    match entry.get("kind").and_then(Value::as_str) {
        Some(found) if found == kind => {}
        found => problems.push(format!("kind is {found:?}, expected `{kind}`")),
    }
    match entry.get("url").and_then(Value::as_str) {
        Some(url) => match pinned_asset_name(url, version) {
            Some(found) if found == name => {}
            Some(found) => problems.push(format!("url names `{found}`, expected `{name}`")),
            None => problems.push(format!(
                "url `{url}` is not https://github.com/{UPDATER_REPO}/releases/download/v{version}/<plain file name>"
            )),
        },
        None => problems.push("url is missing".to_owned()),
    }
    let path = assets.join(name);
    if !fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.is_file()) {
        problems.push(format!("{name} is missing from the assets directory"));
        return problems;
    }
    match hash_file(&path) {
        Ok((sha256, size)) => {
            match entry.get("size").and_then(Value::as_u64) {
                Some(found) if found == size => {}
                found => problems.push(format!("size is {found:?}, the file has {size} bytes")),
            }
            match entry.get("sha256").and_then(Value::as_str) {
                Some(found) if found.eq_ignore_ascii_case(&sha256) => {}
                found => problems.push(format!("sha256 is {found:?}, the file has {sha256}")),
            }
        }
        Err(error) => problems.push(format!("{error:#}")),
    }
    problems
}

/// `OpenOptions` for a new key file: `create_new` (never overwrites, never follows a planted
/// symlink) and, for a secret on Unix, mode `0600` from the first byte (not the default mode).
fn new_file_options(secret: bool) -> fs::OpenOptions {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(if secret { 0o600 } else { 0o644 });
    }
    #[cfg(not(unix))]
    let _ = secret;
    options
}

pub(crate) fn write_new_file(path: &Path, contents: &str, secret: bool) -> anyhow::Result<()> {
    let mut file = new_file_options(secret)
        .open(path)
        .with_context(|| format!("creating {} (refusing to overwrite a key)", path.display()))?;
    file.write_all(contents.as_bytes())
        .with_context(|| format!("writing {}", path.display()))
}

/// `cargo xtask gen-update-keys <dir>`: writes `<name>.key` (secret) and `<name>.pub` for the
/// `current` and `next` keys. Prompts for nothing: keys are unencrypted, so store them only in
/// the GitHub `release` environment and a password manager, never in the repo. Files are created
/// with `create_new`, so an existing key is never overwritten, and the secret ones with mode 0600
/// on Unix.
pub fn run_gen_update_keys(args: &[String]) -> anyhow::Result<()> {
    let [dir] = args else {
        bail!("usage: cargo xtask gen-update-keys <dir>");
    };
    let dir = Path::new(dir);
    fs::create_dir_all(dir)?;
    // Refuse before writing anything, so a leftover `next.key` can't leave a half-new set behind.
    for name in ["current", "next"] {
        for extension in ["key", "pub"] {
            let path = dir.join(format!("{name}.{extension}"));
            if fs::symlink_metadata(&path).is_ok() {
                bail!("{} exists; refusing to overwrite a key", path.display());
            }
        }
    }
    for name in ["current", "next"] {
        let kp = minisign::KeyPair::generate_unencrypted_keypair()
            .map_err(|e| anyhow::anyhow!("key generation failed: {e}"))?;
        let sk_box = kp
            .sk
            .to_box(Some("Dockering update signing key (secret)"))
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let pk_box = kp.pk.to_box().map_err(|e| anyhow::anyhow!("{e}"))?;
        write_new_file(
            &dir.join(format!("{name}.key")),
            &sk_box.into_string(),
            true,
        )?;
        write_new_file(
            &dir.join(format!("{name}.pub")),
            &pk_box.into_string(),
            false,
        )?;
        println!(
            "xtask: {name} public key (paste into crates/dk-update/src/keys.rs): {}",
            kp.pk.to_base64()
        );
    }
    println!(
        "xtask: store current.key as the `{KEY_ENV}` secret in the `release` environment; keep next.key offline"
    );
    Ok(())
}

/// `cargo xtask checksums <dir>`: `SHA256SUMS` (GNU `sha256sum` format, sorted by name).
pub fn run_checksums(args: &[String]) -> anyhow::Result<()> {
    let [dir] = args else {
        bail!("usage: cargo xtask checksums <dir>");
    };
    let dir = Path::new(dir);
    let mut names: Vec<String> = fs::read_dir(dir)?
        .filter_map(Result::ok)
        .filter(|e| e.path().is_file())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n != "SHA256SUMS")
        .collect();
    names.sort();
    let mut out = String::new();
    for name in &names {
        let (sha, _) = hash_file(&dir.join(name))?;
        let _ = writeln!(out, "{sha}  {name}");
    }
    fs::write(dir.join("SHA256SUMS"), out)?;
    println!(
        "xtask: wrote {} ({} files)",
        dir.join("SHA256SUMS").display(),
        names.len()
    );
    Ok(())
}

pub(crate) fn hash_file(path: &Path) -> anyhow::Result<(String, u64)> {
    let mut f = fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    let mut size = 0u64;
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        size += n as u64;
    }
    Ok((hex::encode(hasher.finalize()), size))
}

/// SemVer without build metadata: digits, dots, and an optional `-pre.release` suffix.
pub(crate) fn validate_version(v: &str) -> anyhow::Result<()> {
    let (core, pre) = v.split_once('-').unwrap_or((v, ""));
    let parts: Vec<&str> = core.split('.').collect();
    let core_ok = parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()));
    let pre_ok = pre
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    if !core_ok || !pre_ok || v.contains('+') {
        bail!("invalid version `{v}`");
    }
    Ok(())
}

pub(crate) fn now_rfc3339() -> anyhow::Result<String> {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .context("formatting the current time")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct AssetFixture(PathBuf);

    impl AssetFixture {
        fn new(target: Option<&str>) -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "xtask-assets-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&dir).unwrap();
            for name in required_assets(target).unwrap() {
                fs::write(dir.join(name), b"package").unwrap();
            }
            Self(dir)
        }
    }

    impl Drop for AssetFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn rel_017_all_twelve_distribution_assets_required() {
        let fixture = AssetFixture::new(None);
        assert_eq!(verify_assets(&fixture.0, None).unwrap(), 12);
        // Installer presence alone must not hide a missing portable download.
        fs::remove_file(fixture.0.join("Dockering-arm64.zip")).unwrap();
        let error = verify_assets(&fixture.0, None).unwrap_err().to_string();
        assert!(error.contains("Dockering-arm64.zip"), "{error}");
    }

    #[test]
    fn rel_017_each_target_requires_every_format() {
        for (target, kinds) in DISTRIBUTIONS {
            let fixture = AssetFixture::new(Some(target));
            assert_eq!(
                verify_assets(&fixture.0, Some(target)).unwrap(),
                kinds.len()
            );
        }
        let fixture = AssetFixture::new(Some("aarch64-unknown-linux-gnu"));
        fs::remove_file(fixture.0.join("Dockering-aarch64.deb")).unwrap();
        assert!(verify_assets(&fixture.0, Some("aarch64-unknown-linux-gnu")).is_err());
        assert!(verify_assets(&fixture.0, Some("aarch64-unknown-linux-musl")).is_err());
    }

    #[test]
    fn rel_017_empty_and_non_file_assets_rejected() {
        let fixture = AssetFixture::new(None);
        let path = fixture.0.join("Dockering-aarch64.dmg");
        fs::write(&path, b"").unwrap();
        assert!(verify_assets(&fixture.0, None).is_err());
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(verify_assets(&fixture.0, None).is_err());
    }

    #[test]
    fn rel_017_unexpected_distribution_rejected_metadata_allowed() {
        let fixture = AssetFixture::new(None);
        fs::write(fixture.0.join("dockering-update.json"), b"{}").unwrap();
        fs::write(fixture.0.join("SHA256SUMS"), b"checksums").unwrap();
        assert!(verify_assets(&fixture.0, None).is_ok());
        fs::write(fixture.0.join("Dockering-old-version.exe"), b"stale").unwrap();
        assert!(verify_assets(&fixture.0, None).is_err());
    }

    #[test]
    fn upd_002_manifest_from_assets() {
        let dir = std::env::temp_dir().join(format!("xtask-manifest-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("Dockering-Setup-x64.exe"), b"installer").unwrap();
        fs::write(dir.join("Dockering-aarch64.dmg"), b"dmg!").unwrap();
        let text = build_manifest(&dir, "1.2.3", "o/r", "2026-10-03T00:00:00Z").unwrap();
        fs::remove_dir_all(&dir).unwrap();

        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["schema"], 1);
        assert_eq!(v["version"], "1.2.3");
        assert_eq!(v["notes_url"], "https://github.com/o/r/releases/tag/v1.2.3");
        let win = &v["platforms"]["windows-x86_64"];
        assert_eq!(win["kind"], "inno");
        assert_eq!(
            win["url"],
            "https://github.com/o/r/releases/download/v1.2.3/Dockering-Setup-x64.exe"
        );
        assert_eq!(win["size"], 9);
        assert_eq!(
            win["sha256"],
            "9c0d294c05fc1d88d698034609bb81c0c69196327594e4c69d2915c80fd9850c"
        );
        assert_eq!(v["platforms"]["macos-aarch64"]["kind"], "dmg");
        assert!(v["platforms"].get("linux-x86_64").is_none());
    }

    #[test]
    fn upd_003_signed_manifest_verifies() {
        let kp = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let sig = sign_bytes(&kp.sk, b"{\"schema\":1}").unwrap();
        let sig_box = minisign::SignatureBox::from_string(&sig).unwrap();
        minisign::verify(
            &kp.pk,
            &sig_box,
            Cursor::new(b"{\"schema\":1}"),
            true,
            false,
            false,
        )
        .unwrap();
        assert!(
            minisign::verify(
                &kp.pk,
                &sig_box,
                Cursor::new(b"{\"schema\":2}"),
                true,
                false,
                false
            )
            .is_err()
        );
    }

    #[test]
    fn rel_012_versions_validated() {
        assert!(validate_version("0.2.0").is_ok());
        assert!(validate_version("0.2.0-rc.1").is_ok());
        assert!(validate_version("0.2").is_err());
        assert!(validate_version("0.2.0+meta").is_err());
        assert!(validate_version("0.2.0;rm").is_err());
    }

    fn keys_source(keys: &[&str]) -> String {
        let list: Vec<String> = keys.iter().map(|k| format!("{k:?}")).collect();
        format!(
            "// \"RWQdoc/not/a/key\"\npub const PUBLIC_KEYS: &[&str] = &[{}];\n",
            list.join(", ")
        )
    }

    #[test]
    fn upd_003_parse_public_keys() {
        assert_eq!(
            parse_public_keys("pub const PUBLIC_KEYS: &[&str] = &[];").unwrap(),
            Vec::<String>::new()
        );
        let source = "/// PUBLIC_KEYS: docs\npub const PUBLIC_KEYS: &[&str] = &[\n    // \"RWQcommentedOut\",\n    \"RWQ//a+b/c\", // current\n    /* \"RWQblock\" */ \"RWQnext\",\n];\n";
        assert_eq!(
            parse_public_keys(source).unwrap(),
            ["RWQ//a+b/c", "RWQnext"]
        );
        let file = "pub const PUBLIC_KEYS: &[&str] = &[\"untrusted comment: x\\nRWQfile\\n\"];";
        assert_eq!(
            parse_public_keys(file).unwrap(),
            ["untrusted comment: x\nRWQfile\n"]
        );
        assert!(parse_public_keys("const OTHER: &[&str] = &[];").is_err());
        assert!(parse_public_keys("const PUBLIC_KEYS: &[&str] = &[\"RWQ\"").is_err());
    }

    #[test]
    fn upd_003_committed_keys_source_parses() {
        let source = include_str!("../../crates/dk-update/src/keys.rs");
        assert!(parse_public_keys(source).is_ok());
    }

    #[test]
    fn upd_003_verify_manifest_finds_the_signing_key() {
        let (current, next) = (
            minisign::KeyPair::generate_unencrypted_keypair().unwrap(),
            minisign::KeyPair::generate_unencrypted_keypair().unwrap(),
        );
        let manifest = b"{\"schema\":1}";
        let keys = parse_public_keys(&keys_source(&[
            &current.pk.to_base64(),
            &next.pk.to_base64(),
        ]))
        .unwrap();
        let by_current = sign_bytes(&current.sk, manifest).unwrap();
        let by_next = sign_bytes(&next.sk, manifest).unwrap();
        assert_eq!(verifying_key(manifest, &by_current, &keys).unwrap(), 0);
        assert_eq!(verifying_key(manifest, &by_next, &keys).unwrap(), 1);
        assert!(verifying_key(b"{\"schema\":2}", &by_current, &keys).is_err());
        assert!(verifying_key(manifest, "not a signature", &keys).is_err());
        assert!(verifying_key(manifest, &by_current, &[]).is_err());
        let stranger = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let by_stranger = sign_bytes(&stranger.sk, manifest).unwrap();
        assert!(verifying_key(manifest, &by_stranger, &keys).is_err());
    }

    #[test]
    fn upd_003_generated_keys_sign_through_the_ci_path() {
        let kp = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let text = kp
            .sk
            .to_box(Some("Dockering update signing key (secret)"))
            .unwrap()
            .into_string();
        let pub_key = kp.pk.to_base64();
        let sk = load_secret_key(&text, None).unwrap();
        let sig = sign_bytes(&sk, b"manifest").unwrap();
        assert_eq!(verifying_key(b"manifest", &sig, &[pub_key]).unwrap(), 0);
        // the secret holds the key with its trailing newline, as `gh secret set < file` stores it
        assert!(load_secret_key(&format!("{text}\n"), None).is_ok());
        assert!(load_secret_key("not a key", None).is_err());
    }

    #[test]
    fn upd_003_encrypted_key_needs_its_password() {
        let kp = minisign::KeyPair::generate_encrypted_keypair(Some("pw".into())).unwrap();
        let text = kp.sk.to_box(None).unwrap().into_string();
        let sk = load_secret_key(&text, Some("pw".into())).unwrap();
        let sig = sign_bytes(&sk, b"manifest").unwrap();
        assert_eq!(
            verifying_key(b"manifest", &sig, &[kp.pk.to_base64()]).unwrap(),
            0
        );
        let missing = load_secret_key(&text, None).err().unwrap().to_string();
        assert!(missing.contains(KEY_PASSWORD_ENV), "{missing}");
        assert!(load_secret_key(&text, Some("wrong".into())).is_err());
        // an unencrypted key with a password set is refused rather than silently accepted
        let plain = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let plain = plain.sk.to_box(None).unwrap().into_string();
        assert!(load_secret_key(&plain, Some("pw".into())).is_err());
    }

    #[test]
    fn upd_003_verify_manifest_accepts_a_whole_pub_file() {
        let kp = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let file = kp.pk.to_box().unwrap().into_string();
        let sig = sign_bytes(&kp.sk, b"manifest").unwrap();
        assert_eq!(verifying_key(b"manifest", &sig, &[file]).unwrap(), 0);
    }

    /// A release directory the way `assemble` makes it: the six manifest assets (each with its own
    /// content, so a swapped hash is caught), `dockering-update.json` from `build_manifest`, and a
    /// `keys.rs` listing a throwaway current key and a throwaway rotation key. The manifest is not
    /// signed until a test asks for it.
    struct UpdateFixture {
        root: PathBuf,
        current: minisign::KeyPair,
        next: minisign::KeyPair,
    }

    const FIXTURE_VERSION: &str = "1.2.3";

    impl UpdateFixture {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let root = std::env::temp_dir().join(format!(
                "xtask-update-artifacts-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(root.join("assets")).unwrap();
            let fixture = Self {
                root,
                current: minisign::KeyPair::generate_unencrypted_keypair().unwrap(),
                next: minisign::KeyPair::generate_unencrypted_keypair().unwrap(),
            };
            for (key, name, _) in PLATFORMS {
                fs::write(
                    fixture.assets().join(name),
                    format!("content of {key} at {FIXTURE_VERSION}"),
                )
                .unwrap();
            }
            let manifest = build_manifest(
                &fixture.assets(),
                FIXTURE_VERSION,
                UPDATER_REPO,
                "2026-10-19T00:00:00Z",
            )
            .unwrap();
            fs::write(fixture.assets().join(MANIFEST), manifest).unwrap();
            fs::write(
                fixture.keys(),
                keys_source(&[
                    &fixture.current.pk.to_base64(),
                    &fixture.next.pk.to_base64(),
                ]),
            )
            .unwrap();
            fixture
        }

        fn assets(&self) -> PathBuf {
            self.root.join("assets")
        }

        fn keys(&self) -> PathBuf {
            self.root.join("keys.rs")
        }

        fn minisig(&self) -> PathBuf {
            self.assets().join(format!("{MANIFEST}.minisig"))
        }

        /// Signs the manifest as it is on disk now.
        fn sign_with(&self, secret: &minisign::SecretKey) {
            let data = fs::read(self.assets().join(MANIFEST)).unwrap();
            fs::write(self.minisig(), sign_bytes(secret, &data).unwrap()).unwrap();
        }

        /// Rewrites the manifest and signs the result with the current key, so a test breaks one
        /// thing at a time and the signature is never what fails.
        fn edit(&self, change: impl FnOnce(&mut Value)) {
            let path = self.assets().join(MANIFEST);
            let mut manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            change(&mut manifest);
            fs::write(&path, serde_json::to_string_pretty(&manifest).unwrap()).unwrap();
            self.sign_with(&self.current.sk);
        }

        fn check(&self, updater: Updater) -> anyhow::Result<Vec<String>> {
            verify_update_artifacts(&self.assets(), &self.keys(), FIXTURE_VERSION, updater)
        }

        fn failure(&self, updater: Updater) -> String {
            self.check(updater).unwrap_err().to_string()
        }
    }

    impl Drop for UpdateFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn rel_084_manifest_signed_by_current_key() {
        let fixture = UpdateFixture::new();
        fixture.sign_with(&fixture.current.sk);
        for updater in [Updater::All, Updater::None] {
            let passed = fixture.check(updater).unwrap();
            assert!(
                passed.iter().any(|l| l.contains("current public key (#0)")),
                "{passed:?}"
            );
        }

        // Tampering with the manifest after it was signed breaks the signature.
        let path = fixture.assets().join(MANIFEST);
        let text = fs::read_to_string(&path).unwrap();
        fs::write(&path, text.replace("pub_date", "pub_date2")).unwrap();
        let error = fixture.failure(Updater::All);
        assert!(error.contains("does not verify"), "{error}");

        // A key nobody compiled in is no better than a bad signature.
        let stranger = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        fixture.sign_with(&stranger.sk);
        let error = fixture.failure(Updater::All);
        assert!(error.contains("does not verify"), "{error}");
    }

    #[test]
    fn rel_084_rotation_key_signature_fails() {
        let fixture = UpdateFixture::new();
        // The app would accept this (UPD-003 trusts both keys), but the secret is not the current
        // key, which is what the gate is for.
        fixture.sign_with(&fixture.next.sk);
        for updater in [Updater::All, Updater::None] {
            let error = fixture.failure(updater);
            assert!(error.contains("UPDATE_SIGNING_KEY"), "{error}");
            assert!(error.contains("not the current key"), "{error}");
            assert!(error.contains("#1"), "{error}");
        }
        // After a rotation the old rotation key is #0, and the same file passes.
        fs::write(
            fixture.keys(),
            keys_source(&[
                &fixture.next.pk.to_base64(),
                &fixture.current.pk.to_base64(),
            ]),
        )
        .unwrap();
        fixture.check(Updater::All).unwrap();
        // ... and a first key that cannot be parsed is reported instead of skipped.
        fs::write(
            fixture.keys(),
            keys_source(&["not a key", &fixture.next.pk.to_base64()]),
        )
        .unwrap();
        let error = fixture.failure(Updater::All);
        assert!(error.contains("not a valid minisign public key"), "{error}");
    }

    #[test]
    fn rel_084_updater_all_needs_a_minisig_and_none_does_not() {
        let fixture = UpdateFixture::new();
        let error = fixture.failure(Updater::All);
        assert!(
            error.contains("dockering-update.json.minisig is missing"),
            "{error}"
        );
        assert!(error.contains("--updater all"), "{error}");

        // `none` with no signature checks nothing about signing, not even the keys file.
        fs::remove_file(fixture.keys()).unwrap();
        let passed = fixture.check(Updater::None).unwrap();
        assert!(
            passed
                .iter()
                .any(|l| l.contains("signature is not checked")),
            "{passed:?}"
        );
        // The default of the command line is `none`.
        run_verify_update_artifacts(&[
            "--assets".into(),
            fixture.assets().display().to_string(),
            "--version".into(),
            "v1.2.3".into(),
            "--keys".into(),
            fixture.keys().display().to_string(),
        ])
        .unwrap();
        // The rest of the manifest is still checked.
        fixture.edit(|m| m["version"] = json!("9.9.9"));
        fs::remove_file(fixture.minisig()).unwrap();
        assert!(fixture.failure(Updater::None).contains("manifest version"));
    }

    #[test]
    fn rel_084_a_present_minisig_is_checked_even_for_none() {
        let fixture = UpdateFixture::new();
        fixture.sign_with(&fixture.next.sk);
        assert!(
            fixture
                .failure(Updater::None)
                .contains("not the current key")
        );
        // A signature with no usable keys file is a failure too, not a skip.
        fs::write(fixture.keys(), keys_source(&[])).unwrap();
        let error = fixture.failure(Updater::None);
        assert!(error.contains("has no public keys"), "{error}");
        fs::remove_file(fixture.keys()).unwrap();
        assert!(fixture.failure(Updater::All).contains("reading"));
    }

    #[test]
    fn rel_084_manifest_version_equals_release() {
        let fixture = UpdateFixture::new();
        fixture.sign_with(&fixture.current.sk);
        let wrong = |version: &str| {
            verify_update_artifacts(&fixture.assets(), &fixture.keys(), version, Updater::All)
                .unwrap_err()
                .to_string()
        };
        // Both a newer and an older release number than the manifest's must fail.
        for version in ["1.2.4", "1.2.2", "1.2.3-rc.1"] {
            let error = wrong(version);
            assert!(error.contains("manifest version is `1.2.3`"), "{error}");
        }
        // Entries pinned to another version fail as well, even when the version field was edited.
        fixture.edit(|m| m["version"] = json!("1.2.4"));
        let error = wrong("1.2.4");
        assert!(
            error.contains(
                "is not https://github.com/pavel-purma/dockering/releases/download/v1.2.4/"
            ),
            "{error}"
        );
        // The command line takes the tag form too.
        fixture.edit(|m| {
            m["version"] = json!("1.2.3");
        });
        for version in ["1.2.3", "v1.2.3"] {
            run_verify_update_artifacts(&[
                "--assets".into(),
                fixture.assets().display().to_string(),
                "--version".into(),
                version.into(),
                "--keys".into(),
                fixture.keys().display().to_string(),
                "--updater".into(),
                "all".into(),
            ])
            .unwrap();
        }
        assert!(wrong("1.2").contains("invalid version"));
    }

    #[test]
    fn rel_084_entries_match_assets() {
        let fixture = UpdateFixture::new();
        fixture.sign_with(&fixture.current.sk);
        let passed = fixture.check(Updater::All).unwrap();
        assert_eq!(
            passed
                .iter()
                .filter(|l| l.contains("matches kind, URL, size and SHA-256"))
                .count(),
            6,
            "{passed:?}"
        );

        fn url(name: &str) -> String {
            format!("https://github.com/pavel-purma/dockering/releases/download/v1.2.3/{name}")
        }
        type Mutation = Box<dyn Fn(&mut Value)>;
        let cases: Vec<(&str, Mutation, &str)> = vec![
            (
                "wrong size",
                Box::new(|m| {
                    let size = m["platforms"]["linux-x86_64"]["size"].as_u64().unwrap();
                    m["platforms"]["linux-x86_64"]["size"] = json!(size + 1);
                }),
                "`linux-x86_64` (Dockering-x86_64.AppImage): size is",
            ),
            (
                "size as a string",
                Box::new(|m| m["platforms"]["linux-x86_64"]["size"] = json!("5")),
                "size is",
            ),
            (
                "another file's hash",
                Box::new(|m| {
                    let other = m["platforms"]["macos-aarch64"]["sha256"].clone();
                    m["platforms"]["windows-x86_64"]["sha256"] = other;
                }),
                "`windows-x86_64` (Dockering-Setup-x64.exe): sha256 is",
            ),
            (
                "wrong kind",
                Box::new(|m| m["platforms"]["linux-aarch64"]["kind"] = json!("deb")),
                "kind is Some(\"deb\"), expected `appimage`",
            ),
            (
                "kind not in the mapping",
                Box::new(|m| m["platforms"]["windows-aarch64"]["kind"] = json!("zip")),
                "expected `inno`",
            ),
            (
                "another repository",
                Box::new(|m| {
                    m["platforms"]["macos-x86_64"]["url"] = json!(
                        "https://github.com/attacker/dockering/releases/download/v1.2.3/Dockering-x86_64.dmg"
                    );
                }),
                "`macos-x86_64` (Dockering-x86_64.dmg): url",
            ),
            (
                "another version",
                Box::new(|m| {
                    m["platforms"]["macos-x86_64"]["url"] = json!(
                        "https://github.com/pavel-purma/dockering/releases/download/v1.2.2/Dockering-x86_64.dmg"
                    );
                }),
                "is not https://github.com/pavel-purma/dockering/releases/download/v1.2.3/<plain file name>",
            ),
            (
                "plain http",
                Box::new(|m| {
                    m["platforms"]["macos-x86_64"]["url"] = json!(
                        "http://github.com/pavel-purma/dockering/releases/download/v1.2.3/Dockering-x86_64.dmg"
                    );
                }),
                "<plain file name>",
            ),
            (
                "not a plain name",
                Box::new(|m| {
                    m["platforms"]["macos-x86_64"]["url"] = json!(url("../Dockering-x86_64.dmg"));
                }),
                "<plain file name>",
            ),
            (
                "another asset's name",
                Box::new(|m| {
                    m["platforms"]["macos-x86_64"]["url"] = json!(url("Dockering-aarch64.dmg"));
                }),
                "url names `Dockering-aarch64.dmg`, expected `Dockering-x86_64.dmg`",
            ),
            (
                "a query on the url",
                Box::new(|m| {
                    m["platforms"]["macos-x86_64"]["url"] = json!(url("Dockering-x86_64.dmg?x=1"));
                }),
                "<plain file name>",
            ),
            (
                "no url",
                Box::new(|m| {
                    m["platforms"]["macos-x86_64"]
                        .as_object_mut()
                        .unwrap()
                        .remove("url");
                }),
                "url is missing",
            ),
            (
                "a platform left out",
                Box::new(|m| {
                    m["platforms"]
                        .as_object_mut()
                        .unwrap()
                        .remove("windows-aarch64");
                }),
                "platform `windows-aarch64` is missing from the manifest",
            ),
            (
                "a platform nobody can check",
                Box::new(|m| {
                    m["platforms"]["linux-riscv64"] = m["platforms"]["linux-x86_64"].clone();
                }),
                "unexpected platform `linux-riscv64`",
            ),
            (
                "a schema the app does not read",
                Box::new(|m| m["schema"] = json!(2)),
                "manifest schema is Some(2), expected 1",
            ),
        ];
        for (what, mutate, expected) in cases {
            let fixture = UpdateFixture::new();
            fixture.edit(mutate);
            let error = fixture.failure(Updater::All);
            assert!(error.contains(expected), "{what}: {error}");
            // The same breakage is found with the updater off: the entries are always checked.
            assert!(fixture.failure(Updater::None).contains(expected), "{what}");
        }

        // An asset that changed after the manifest was generated (same size, other bytes).
        let fixture = UpdateFixture::new();
        fixture.sign_with(&fixture.current.sk);
        let name = "Dockering-Setup-arm64.exe";
        let original = fs::read(fixture.assets().join(name)).unwrap();
        let mut changed = original.clone();
        changed[0] ^= 1;
        fs::write(fixture.assets().join(name), changed).unwrap();
        let error = fixture.failure(Updater::All);
        assert!(
            error.contains("`windows-aarch64` (Dockering-Setup-arm64.exe): sha256 is"),
            "{error}"
        );
        assert!(!error.contains("size is"), "{error}");

        // An asset that is missing, and a directory in its place.
        fs::remove_file(fixture.assets().join(name)).unwrap();
        let error = fixture.failure(Updater::All);
        assert!(
            error.contains("Dockering-Setup-arm64.exe is missing from the assets directory"),
            "{error}"
        );
        fs::create_dir(fixture.assets().join(name)).unwrap();
        assert!(
            fixture
                .failure(Updater::All)
                .contains("is missing from the assets directory")
        );

        // Every problem is listed, not only the first.
        fs::remove_dir(fixture.assets().join(name)).unwrap();
        fixture.edit(|m| {
            m["version"] = json!("2.0.0");
            m["platforms"]
                .as_object_mut()
                .unwrap()
                .remove("linux-x86_64");
        });
        let error = fixture.failure(Updater::All);
        assert!(error.contains("manifest version is `2.0.0`"), "{error}");
        assert!(error.contains("`linux-x86_64` is missing"), "{error}");
        assert!(
            error.contains("Dockering-Setup-arm64.exe is missing"),
            "{error}"
        );

        // An upper-case digest is what the app accepts too.
        let fixture = UpdateFixture::new();
        fixture.edit(|m| {
            let sha = m["platforms"]["macos-aarch64"]["sha256"]
                .as_str()
                .unwrap()
                .to_uppercase();
            m["platforms"]["macos-aarch64"]["sha256"] = json!(sha);
        });
        fixture.check(Updater::All).unwrap();
    }

    #[test]
    fn rel_084_malformed_manifest_and_missing_files_are_errors() {
        let fixture = UpdateFixture::new();
        fixture.sign_with(&fixture.current.sk);
        fs::write(fixture.assets().join(MANIFEST), "not json").unwrap();
        fixture.sign_with(&fixture.current.sk);
        assert!(fixture.failure(Updater::All).contains("is not valid JSON"));
        fs::write(fixture.assets().join(MANIFEST), "[]").unwrap();
        fixture.sign_with(&fixture.current.sk);
        assert!(
            fixture
                .failure(Updater::All)
                .contains("no `platforms` object")
        );
        fs::remove_file(fixture.assets().join(MANIFEST)).unwrap();
        assert!(fixture.failure(Updater::All).contains("reading"));
    }

    #[test]
    fn rel_084_arguments_are_validated() {
        let fixture = UpdateFixture::new();
        let run = |extra: &[&str]| {
            let mut args = vec![
                "--assets".to_owned(),
                fixture.assets().display().to_string(),
                "--keys".to_owned(),
                fixture.keys().display().to_string(),
            ];
            args.extend(extra.iter().map(|a| (*a).to_owned()));
            run_verify_update_artifacts(&args)
        };
        assert!(
            run(&[])
                .unwrap_err()
                .to_string()
                .contains("--version is required")
        );
        for bad in ["both", "ALL", "", "signed"] {
            let error = run(&["--version", "1.2.3", "--updater", bad])
                .unwrap_err()
                .to_string();
            assert!(
                error.contains("--updater must be `none` or `all`"),
                "{bad}: {error}"
            );
        }
        assert!(run(&["--version", "1.2.3", "--unknown", "x"]).is_err());
        assert!(run(&["--version", "1.2.3"]).is_ok());
        assert!(
            run_verify_update_artifacts(&["--version".into(), "1.2.3".into()])
                .unwrap_err()
                .to_string()
                .contains("--assets is required")
        );
    }

    /// `pinned_asset_name` replicates `dk_update::source::release_asset_name`; this fails when the
    /// app's pin (repository, path shape or the plain-name rule) changes and the copy does not.
    #[test]
    fn rel_084_url_pin_replicates_the_apps() {
        let source = include_str!("../../crates/dk-update/src/source.rs");
        assert!(
            source.contains(&format!("pub const REPO: &str = \"{UPDATER_REPO}\";")),
            "dk-update's REPO changed: update UPDATER_REPO and pinned_asset_name"
        );
        for needle in [
            "https://github.com/{REPO}/releases/download/v{version}/",
            "!n.is_empty()",
            "!n.starts_with('.')",
            "c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')",
        ] {
            assert!(
                source.contains(needle),
                "dk-update's release_asset_name changed (`{needle}`): update pinned_asset_name"
            );
        }
        let base = "https://github.com/pavel-purma/dockering/releases/download/v0.2.0/";
        assert_eq!(
            pinned_asset_name(&format!("{base}Dockering-Setup-x64.exe"), "0.2.0"),
            Some("Dockering-Setup-x64.exe")
        );
        for bad in [
            base.to_owned(),
            format!("{base}.hidden"),
            format!("{base}a/b.exe"),
            format!("{base}../x.exe"),
            format!("{base}a b.exe"),
            "http://github.com/pavel-purma/dockering/releases/download/v0.2.0/a.exe".to_owned(),
            "https://github.com/attacker/x/releases/download/v0.2.0/a.exe".to_owned(),
            "https://github.com/pavel-purma/dockering/releases/download/v0.1.0/a.exe".to_owned(),
        ] {
            assert_eq!(pinned_asset_name(&bad, "0.2.0"), None, "{bad}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn upd_003_gen_update_keys_secret_files_are_0600_and_never_overwritten() {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        let root = std::env::temp_dir().join(format!("xtask-gen-keys-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let dir = root.join("keys");
        let args = [dir.display().to_string()];
        run_gen_update_keys(&args).unwrap();
        for name in ["current", "next"] {
            assert_eq!(mode(&dir.join(format!("{name}.key"))), 0o600, "{name}.key");
            let public = mode(&dir.join(format!("{name}.pub")));
            assert_eq!(public & 0o022, 0, "{name}.pub is {public:o}");
        }
        // The generated key signs and verifies through the CI path.
        let secret = fs::read_to_string(dir.join("current.key")).unwrap();
        let sk = load_secret_key(&secret, None).unwrap();
        let public = fs::read_to_string(dir.join("current.pub")).unwrap();
        let sig = sign_bytes(&sk, b"manifest").unwrap();
        assert_eq!(verifying_key(b"manifest", &sig, &[public]).unwrap(), 0);

        // A second run refuses, and the keys on disk are untouched.
        let before = fs::read(dir.join("current.key")).unwrap();
        let error = run_gen_update_keys(&args).unwrap_err().to_string();
        assert!(error.contains("refusing to overwrite"), "{error}");
        assert_eq!(fs::read(dir.join("current.key")).unwrap(), before);

        // A leftover next.key stops the run before current.key is replaced by a new pair.
        fs::remove_file(dir.join("current.key")).unwrap();
        fs::remove_file(dir.join("current.pub")).unwrap();
        let error = run_gen_update_keys(&args).unwrap_err().to_string();
        assert!(
            error.contains("next.key") || error.contains("next.pub"),
            "{error}"
        );
        assert!(!dir.join("current.key").exists());

        // The secret is created with the mode, whatever the umask would have given: it is never
        // readable by anyone else, not even for an instant. A planted symlink is not followed.
        let planted = dir.join("current.key");
        std::os::unix::fs::symlink(root.join("elsewhere"), &planted).unwrap();
        assert!(write_new_file(&planted, "secret", true).is_err());
        assert!(!root.join("elsewhere").exists());
        let _ = fs::remove_dir_all(&root);
    }
}
