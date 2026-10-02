//! `cargo xtask package` / `cargo xtask dist` (spec 50, Packaging; REL-002).
//!
//! `package` turns an already-built release binary into the per-OS artifacts:
//! - Windows: `.msi` (WiX) on x86_64, NSIS `.exe` on aarch64 (WiX 3, which cargo-packager
//!   uses, can't target ARM64), plus a portable `.zip`
//! - macOS: `.dmg` with `Dockering.app`
//! - Linux: `.deb`, `.AppImage`, plus a portable `.tar.gz`
//!
//! cargo-packager reads `packaging/packager.toml`. With `--config`, cargo-packager doesn't read
//! Cargo metadata, so this command writes `packaging/packager.generated.toml` (gitignored). It
//! adds the build-specific keys: `version`, `binariesDir`, and, if
//! `DOCKERING_MACOS_SIGNING_IDENTITY` is set, `macos.signingIdentity`. Signing secrets are only
//! ever read from the environment (GitHub environment `release`). They are never written to the repo.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, bail};

use crate::util::{Args, cargo, output, run as run_cmd, validate_ident, workspace_root};

/// cargo-packager version the config was written and checked against.
pub const PACKAGER_VERSION: &str = "0.11.8";
const CONFIG: &str = "packaging/packager.toml";
const GENERATED_CONFIG: &str = "packaging/packager.generated.toml";
const BIN: &str = "dockering";
const SIGNING_IDENTITY_ENV: &str = "DOCKERING_MACOS_SIGNING_IDENTITY";

const VALUE_OPTS: &[&str] = &["--target", "--formats"];
const FLAG_OPTS: &[&str] = &["--no-archive"];

struct Options {
    /// `Some` when the binary was built with an explicit `--target` (lives in `target/<triple>/`).
    explicit_target: Option<String>,
    triple: String,
    formats: Vec<String>,
    archive: bool,
}

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let opts = parse(args)?;
    package(&opts)
}

pub fn run_dist(args: &[String]) -> anyhow::Result<()> {
    let opts = parse(args)?;
    let mut build = cargo();
    build.args(["build", "--release", "--locked", "-p", BIN]);
    if let Some(t) = &opts.explicit_target {
        build.args(["--target", t]);
    }
    run_cmd(&mut build)?;
    package(&opts)
}

fn parse(args: &[String]) -> anyhow::Result<Options> {
    let parsed = Args::new(args);
    parsed.reject_unknown(VALUE_OPTS, FLAG_OPTS)?;
    let explicit_target = parsed.value("--target")?.map(str::to_owned);
    if let Some(t) = &explicit_target {
        validate_ident("target triple", t)?;
    }
    let triple = match &explicit_target {
        Some(t) => t.clone(),
        None => host_triple()?,
    };
    let formats: Vec<String> = match parsed.value("--formats")? {
        Some(list) => list
            .split(',')
            .map(str::trim)
            .filter(|f| !f.is_empty())
            .map(str::to_owned)
            .collect(),
        None => default_formats(&triple)?
            .iter()
            .map(|f| (*f).to_owned())
            .collect(),
    };
    for f in &formats {
        validate_ident("package format", f)?;
    }
    Ok(Options {
        explicit_target,
        triple,
        formats,
        archive: !parsed.flag("--no-archive"),
    })
}

/// Default cargo-packager formats per target (spec 50 Packaging table).
pub fn default_formats(triple: &str) -> anyhow::Result<&'static [&'static str]> {
    Ok(match target_os(triple)? {
        TargetOs::Windows if triple.starts_with("aarch64") => &["nsis"],
        TargetOs::Windows => &["wix"],
        TargetOs::MacOs => &["dmg"],
        TargetOs::Linux => &["deb", "appimage"],
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TargetOs {
    Windows,
    MacOs,
    Linux,
}

fn target_os(triple: &str) -> anyhow::Result<TargetOs> {
    if triple.contains("-windows-") {
        Ok(TargetOs::Windows)
    } else if triple.contains("-apple-darwin") {
        Ok(TargetOs::MacOs)
    } else if triple.contains("-linux-") {
        Ok(TargetOs::Linux)
    } else {
        bail!("unsupported packaging target `{triple}`")
    }
}

fn host_triple() -> anyhow::Result<String> {
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let out = output(Command::new(rustc).arg("-vV"))?;
    out.lines()
        .find_map(|l| l.strip_prefix("host: "))
        .map(|h| h.trim().to_owned())
        .context("`rustc -vV` did not report a host triple")
}

struct Metadata {
    version: String,
    target_dir: PathBuf,
}

fn metadata() -> anyhow::Result<Metadata> {
    let json = output(cargo().args(["metadata", "--no-deps", "--format-version", "1"]))?;
    let v: serde_json::Value = serde_json::from_str(&json).context("parsing cargo metadata")?;
    let version = v["packages"]
        .as_array()
        .and_then(|pkgs| pkgs.iter().find(|p| p["name"] == BIN))
        .and_then(|p| p["version"].as_str())
        .context("package `dockering` not found in cargo metadata")?
        .to_owned();
    let target_dir = v["target_directory"]
        .as_str()
        .context("cargo metadata has no target_directory")?
        .into();
    Ok(Metadata {
        version,
        target_dir,
    })
}

fn package(opts: &Options) -> anyhow::Result<()> {
    let root = workspace_root();
    let os = target_os(&opts.triple)?;
    let meta = metadata()?;

    let mut bin_dir = meta.target_dir.clone();
    if let Some(t) = &opts.explicit_target {
        bin_dir.push(t);
    }
    bin_dir.push("release");
    let exe = if os == TargetOs::Windows {
        format!("{BIN}.exe")
    } else {
        BIN.to_owned()
    };
    let bin_path = bin_dir.join(&exe);
    if !bin_path.is_file() {
        bail!(
            "{} not found; build it first (`cargo build --release{}`) or use `cargo xtask dist`",
            bin_path.display(),
            opts.explicit_target
                .as_deref()
                .map(|t| format!(" --target {t}"))
                .unwrap_or_default()
        );
    }
    let notices = root.join("THIRD_PARTY_LICENSES.html");
    if !notices.is_file() {
        bail!(
            "THIRD_PARTY_LICENSES.html is missing; run `cargo about generate about.hbs -o THIRD_PARTY_LICENSES.html` (REL-002)"
        );
    }

    let out_dir = meta.target_dir.join("dist").join(&opts.triple);
    fs::create_dir_all(&out_dir).with_context(|| format!("creating {}", out_dir.display()))?;

    if !opts.formats.is_empty() {
        ensure_packager()?;
        let identity = std::env::var(SIGNING_IDENTITY_ENV)
            .ok()
            .filter(|s| !s.trim().is_empty() && os == TargetOs::MacOs);
        let generated = root.join(GENERATED_CONFIG);
        let base = fs::read_to_string(root.join(CONFIG)).context("reading packaging config")?;
        let config = generate_config(&base, &meta.version, &bin_dir, identity.as_deref())?;
        fs::write(&generated, config)
            .with_context(|| format!("writing {}", generated.display()))?;

        let mut cmd = cargo();
        cmd.arg("packager")
            .arg("--config")
            .arg(&generated)
            .arg("--out-dir")
            .arg(&out_dir)
            .args(["--target", &opts.triple])
            .args(["--formats", &opts.formats.join(",")]);
        run_cmd(&mut cmd)?;
    }

    if opts.archive && os != TargetOs::MacOs {
        portable_archive(&root, os, &opts.triple, &meta.version, &bin_path, &out_dir)?;
    }
    println!("xtask: artifacts in {}", out_dir.display());
    Ok(())
}

fn ensure_packager() -> anyhow::Result<()> {
    let ok = cargo()
        .args(["packager", "--version"])
        .output()
        .is_ok_and(|o| o.status.success());
    if !ok {
        bail!(
            "cargo-packager is not installed; run `cargo install cargo-packager --locked --version {PACKAGER_VERSION}`"
        );
    }
    Ok(())
}

/// Adds the build-specific keys to the hand-written config. Top-level keys go first so they
/// can't land inside a table. The signing identity goes right after the `[macos]` header.
fn generate_config(
    base: &str,
    version: &str,
    bin_dir: &Path,
    signing_identity: Option<&str>,
) -> anyhow::Result<String> {
    for key in ["version", "binariesDir", "outDir", "signingIdentity"] {
        if base
            .lines()
            .any(|l| l.trim_start().starts_with(key) && l.contains('='))
        {
            bail!("{CONFIG} must not set `{key}`; xtask injects it per build");
        }
    }
    let mut out = format!(
        "# GENERATED by `cargo xtask package` from {CONFIG}. Do not edit or commit.\n\
         version = {}\nbinariesDir = {}\n\n",
        toml_string(version),
        toml_string(&bin_dir.to_string_lossy()),
    );
    match signing_identity {
        None => out.push_str(base),
        Some(identity) => {
            let mut inserted = false;
            for line in base.lines() {
                out.push_str(line);
                out.push('\n');
                if line.trim() == "[macos]" {
                    out.push_str(&format!("signingIdentity = {}\n", toml_string(identity)));
                    inserted = true;
                }
            }
            if !inserted {
                bail!("{CONFIG} has no [macos] table to receive the signing identity");
            }
        }
    }
    Ok(out)
}

/// TOML basic string with escaping.
fn toml_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Portable archive: `.zip` on Windows, `.tar.gz` on Linux. Bundles the binary, the licence
/// files, and the third-party notices (REL-002). On Linux it also bundles the desktop entry and
/// icon (SHL-023).
fn portable_archive(
    root: &Path,
    os: TargetOs,
    triple: &str,
    version: &str,
    bin_path: &Path,
    out_dir: &Path,
) -> anyhow::Result<()> {
    let name = format!("{BIN}-{version}-{triple}");
    let stage = out_dir.join(&name);
    if stage.exists() {
        fs::remove_dir_all(&stage).with_context(|| format!("cleaning {}", stage.display()))?;
    }
    fs::create_dir_all(&stage)?;

    let file_name = bin_path.file_name().context("binary has no file name")?;
    let mut files: Vec<(PathBuf, PathBuf)> = vec![
        (bin_path.to_path_buf(), stage.join(file_name)),
        (root.join("LICENSE-MIT"), stage.join("LICENSE-MIT")),
        (root.join("LICENSE-APACHE"), stage.join("LICENSE-APACHE")),
        (
            root.join("THIRD_PARTY_LICENSES.html"),
            stage.join("THIRD_PARTY_LICENSES.html"),
        ),
    ];
    if os == TargetOs::Linux {
        files.push((
            root.join("assets/linux/dev.dockering.Dockering.desktop"),
            stage.join("share/applications/dev.dockering.Dockering.desktop"),
        ));
        files.push((
            root.join("assets/app-icon/icon-512.png"),
            stage.join("share/icons/hicolor/512x512/apps/dev.dockering.Dockering.png"),
        ));
    }
    for (src, dst) in &files {
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(src, dst)
            .with_context(|| format!("copying {} -> {}", src.display(), dst.display()))?;
    }

    let archive = match os {
        TargetOs::Windows => format!("{name}.zip"),
        _ => format!("{name}.tar.gz"),
    };
    let archive_path = out_dir.join(&archive);
    if archive_path.exists() {
        fs::remove_file(&archive_path)?;
    }
    let mut tar = match os {
        // bsdtar from System32 writes zip with `-a`; avoid Git's GNU tar on PATH.
        TargetOs::Windows => {
            let system_root =
                std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
            let mut cmd = Command::new(Path::new(&system_root).join("System32").join("tar.exe"));
            cmd.args(["-a", "-c", "-f"]);
            cmd
        }
        _ => {
            let mut cmd = Command::new("tar");
            cmd.args(["-c", "-z", "-f"]);
            cmd
        }
    };
    tar.arg(&archive).arg(&name).current_dir(out_dir);
    run_cmd(&mut tar)?;
    fs::remove_dir_all(&stage)?;
    println!("xtask: wrote {}", archive_path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rel_formats_per_target() {
        assert_eq!(default_formats("x86_64-pc-windows-msvc").unwrap(), &["wix"]);
        assert_eq!(
            default_formats("aarch64-pc-windows-msvc").unwrap(),
            &["nsis"]
        );
        assert_eq!(default_formats("aarch64-apple-darwin").unwrap(), &["dmg"]);
        assert_eq!(
            default_formats("aarch64-unknown-linux-gnu").unwrap(),
            &["deb", "appimage"]
        );
        assert!(default_formats("wasm32-unknown-unknown").is_err());
    }

    #[test]
    fn generated_config_injects_build_keys() {
        let base = "productName = \"Dockering\"\n\n[macos]\nminimumSystemVersion = \"15.0\"\n";
        let out = generate_config(base, "0.1.0", Path::new("C:\\t\\release"), None).unwrap();
        assert!(out.contains("version = \"0.1.0\"\nbinariesDir = \"C:\\\\t\\\\release\"\n"));
        assert!(!out.contains("signingIdentity"));

        let out =
            generate_config(base, "0.1.0", Path::new("/t"), Some("Developer ID \"X\"")).unwrap();
        assert!(out.contains("[macos]\nsigningIdentity = \"Developer ID \\\"X\\\"\"\n"));

        assert!(generate_config("version = \"1\"\n", "0.1.0", Path::new("/t"), None).is_err());
    }

    #[test]
    fn checked_in_config_is_injectable() {
        let base = fs::read_to_string(workspace_root().join(CONFIG)).unwrap();
        generate_config(
            &base,
            "0.1.0",
            Path::new("/t"),
            Some("Developer ID Application: X"),
        )
        .unwrap();
    }
}
