//! Release metadata (spec `features/distribution.md`):
//! - `update-manifest`: `dockering-update.json` from the built assets (UPD-002)
//! - `sign-manifest`: minisign signature `<file>.minisig` (UPD-003)
//! - `gen-update-keys`: one-time minisign key pair for the updater (UPD-003)
//! - `checksums`: `SHA256SUMS` over every asset (REL-012)

use std::fmt::Write as _;
use std::fs;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::util::Args;

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
    validate_version(version)?;
    if !repo
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/'))
    {
        bail!("invalid repo `{repo}`");
    }
    let mut platforms = Map::new();
    for (key, name, kind) in PLATFORMS {
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
    let sk = minisign::SecretKeyBox::from_string(&key_text)
        .and_then(|b| b.into_secret_key(password))
        .map_err(|e| anyhow::anyhow!("invalid ${KEY_ENV}: {e}"))?;
    let data = fs::read(file).with_context(|| format!("reading {file}"))?;
    let sig = sign_bytes(&sk, &data)?;
    let out = format!("{file}.minisig");
    fs::write(&out, sig).with_context(|| format!("writing {out}"))?;
    println!("xtask: wrote {out}");
    Ok(())
}

fn sign_bytes(sk: &minisign::SecretKey, data: &[u8]) -> anyhow::Result<String> {
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

/// `cargo xtask gen-update-keys <dir>`: writes `<name>.key` (secret) and `<name>.pub` for the
/// `current` and `next` keys. Prompts for nothing: keys are unencrypted, so store them only in
/// the GitHub `release` environment and a password manager, never in the repo.
pub fn run_gen_update_keys(args: &[String]) -> anyhow::Result<()> {
    let [dir] = args else {
        bail!("usage: cargo xtask gen-update-keys <dir>");
    };
    let dir = Path::new(dir);
    fs::create_dir_all(dir)?;
    for name in ["current", "next"] {
        let kp = minisign::KeyPair::generate_unencrypted_keypair()
            .map_err(|e| anyhow::anyhow!("key generation failed: {e}"))?;
        let sk_box = kp
            .sk
            .to_box(Some("Dockering update signing key (secret)"))
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let pk_box = kp.pk.to_box().map_err(|e| anyhow::anyhow!("{e}"))?;
        let sk_path = dir.join(format!("{name}.key"));
        if sk_path.exists() {
            bail!("{} exists; refusing to overwrite a key", sk_path.display());
        }
        fs::write(&sk_path, sk_box.into_string())?;
        fs::write(dir.join(format!("{name}.pub")), pk_box.into_string())?;
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

fn hash_file(path: &Path) -> anyhow::Result<(String, u64)> {
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
fn validate_version(v: &str) -> anyhow::Result<()> {
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

fn now_rfc3339() -> anyhow::Result<String> {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .context("formatting the current time")
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
