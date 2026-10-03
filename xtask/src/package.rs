//! `cargo xtask package` / `cargo xtask dist` (spec 50, Packaging; REL-002).
//!
//! `package` turns an already-built release binary into the per-OS artifacts:
//! - Windows: Inno Setup installer `Dockering-Setup-<x64|arm64>.exe` (REL-020; format `inno`,
//!   built by `ISCC.exe` from `packaging/windows/dockering.iss`), plus a portable `.zip`
//! - macOS: `.dmg` with `Dockering.app`
//! - Linux: `.deb`, `.AppImage`, plus a portable `.tar.gz`
//!
//! Every shipped artifact gets a stable, version-less name (REL-012), so
//! `releases/latest/download/<name>` links never change. `wix`/`nsis` still work when asked for
//! explicitly with `--formats` (they keep cargo-packager's names).
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
const INNO_SCRIPT: &str = "packaging/windows/dockering.iss";
/// Path to `ISCC.exe`, if it isn't on `PATH` or in a standard install dir.
const ISCC_ENV: &str = "ISCC";
/// Inno `SignTool` command (passed as `/Ssigntool=<value>`), e.g. `signtool sign /fd sha256 … $f`.
/// When set, ISCC signs the setup and the uninstaller it embeds.
const INNO_SIGNTOOL_ENV: &str = "DOCKERING_INNO_SIGNTOOL";

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
        TargetOs::Windows => &["inno"],
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

    let (inno, packager_formats): (Vec<&str>, Vec<&str>) = opts
        .formats
        .iter()
        .map(String::as_str)
        .partition(|f| *f == "inno");
    if !inno.is_empty() {
        if os != TargetOs::Windows {
            bail!("format `inno` is Windows-only");
        }
        inno_installer(&root, &opts.triple, &meta.version, &bin_path, &out_dir)?;
    }

    if !packager_formats.is_empty() {
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
            .args(["--formats", &packager_formats.join(",")]);
        run_cmd(&mut cmd)?;
        rename_packager_outputs(&out_dir, &opts.triple, &meta.version)?;
    }

    if opts.archive && os != TargetOs::MacOs {
        portable_archive(&root, os, &opts.triple, &bin_path, &out_dir)?;
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
    bin_path: &Path,
    out_dir: &Path,
) -> anyhow::Result<()> {
    let name = format!("{BIN}-{triple}");
    let stage = out_dir.join(&name);
    if stage.exists() {
        fs::remove_dir_all(&stage).with_context(|| format!("cleaning {}", stage.display()))?;
    }
    fs::create_dir_all(&stage)?;

    let file_name = bin_path.file_name().context("binary has no file name")?;
    let mut files: Vec<(PathBuf, PathBuf)> = vec![
        (bin_path.to_path_buf(), stage.join(file_name)),
        (root.join("LICENSE"), stage.join("LICENSE")),
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

    let archive = stable_name(triple, ArtifactKind::Archive)?;
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

/// What a release asset is, for its stable name (REL-012).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactKind {
    /// Windows Inno Setup installer.
    Installer,
    /// Portable `.zip` (Windows) / `.tar.gz` (Linux).
    Archive,
    Dmg,
    AppImage,
    Deb,
}

/// Windows names use Windows architecture names (`x64`, `arm64`); macOS and Linux keep the
/// Rust ones (`x86_64`, `aarch64`).
fn arch_label(triple: &str) -> anyhow::Result<&'static str> {
    let windows = target_os(triple)? == TargetOs::Windows;
    Ok(match (triple.split('-').next().unwrap_or(""), windows) {
        ("x86_64", true) => "x64",
        ("aarch64", true) => "arm64",
        ("x86_64", false) => "x86_64",
        ("aarch64", false) => "aarch64",
        (arch, _) => bail!("unsupported architecture `{arch}` in `{triple}`"),
    })
}

/// Stable, version-less asset name (REL-012), e.g. `Dockering-Setup-x64.exe`.
pub fn stable_name(triple: &str, kind: ArtifactKind) -> anyhow::Result<String> {
    let os = target_os(triple)?;
    let arch = arch_label(triple)?;
    Ok(match (os, kind) {
        (TargetOs::Windows, ArtifactKind::Installer) => format!("Dockering-Setup-{arch}.exe"),
        (TargetOs::Windows, ArtifactKind::Archive) => format!("Dockering-{arch}.zip"),
        (TargetOs::Linux, ArtifactKind::Archive) => format!("Dockering-{arch}.tar.gz"),
        (TargetOs::MacOs, ArtifactKind::Dmg) => format!("Dockering-{arch}.dmg"),
        (TargetOs::Linux, ArtifactKind::AppImage) => format!("Dockering-{arch}.AppImage"),
        (TargetOs::Linux, ArtifactKind::Deb) => format!("Dockering-{arch}.deb"),
        (os, kind) => bail!("no {kind:?} artifact for {os:?}"),
    })
}

/// cargo-packager names outputs after the product, version, and arch. Rename the formats we ship
/// to their stable names; anything else (explicit `wix`/`nsis`) keeps its name.
fn rename_packager_outputs(out_dir: &Path, triple: &str, version: &str) -> anyhow::Result<()> {
    for entry in fs::read_dir(out_dir)? {
        let path = entry?.path();
        let Some(file) = path.file_name().and_then(|f| f.to_str()) else {
            continue;
        };
        if !path.is_file() || !file.contains(version) {
            continue;
        }
        let kind = if file.ends_with(".dmg") {
            ArtifactKind::Dmg
        } else if file.ends_with(".AppImage") {
            ArtifactKind::AppImage
        } else if file.ends_with(".deb") {
            ArtifactKind::Deb
        } else {
            continue;
        };
        let target = out_dir.join(stable_name(triple, kind)?);
        if target.exists() {
            fs::remove_file(&target)?;
        }
        fs::rename(&path, &target)
            .with_context(|| format!("renaming {} -> {}", path.display(), target.display()))?;
        println!("xtask: wrote {}", target.display());
    }
    Ok(())
}

/// Builds the Inno Setup installer (REL-020) from `packaging/windows/dockering.iss`.
fn inno_installer(
    root: &Path,
    triple: &str,
    version: &str,
    bin_path: &Path,
    out_dir: &Path,
) -> anyhow::Result<()> {
    let iscc = find_iscc()?;
    // ISCC takes the payload from one directory: the exe plus the licence files (REL-023).
    let stage = out_dir.join("inno-stage");
    if stage.exists() {
        fs::remove_dir_all(&stage)?;
    }
    fs::create_dir_all(&stage)?;
    let file_name = bin_path.file_name().context("binary has no file name")?;
    fs::copy(bin_path, stage.join(file_name))?;
    for f in ["LICENSE", "THIRD_PARTY_LICENSES.html"] {
        fs::copy(root.join(f), stage.join(f)).with_context(|| format!("copying {f}"))?;
    }

    let mut cmd = Command::new(&iscc);
    cmd.arg("/Q")
        .arg(format!("/DAppVersion={version}"))
        .arg(format!("/DArch={}", arch_label(triple)?))
        .arg(define("SourceDir", &stage))
        .arg(define("OutputDir", out_dir))
        .arg(define("RepoRoot", root));
    if let Some(signtool) = std::env::var(INNO_SIGNTOOL_ENV)
        .ok()
        .filter(|s| !s.trim().is_empty())
    {
        cmd.arg(format!("/Ssigntool={signtool}")).arg("/DSign");
    }
    cmd.arg(root.join(INNO_SCRIPT));
    run_cmd(&mut cmd)?;
    fs::remove_dir_all(&stage)?;
    println!(
        "xtask: wrote {}",
        out_dir
            .join(stable_name(triple, ArtifactKind::Installer)?)
            .display()
    );
    Ok(())
}

fn define(name: &str, path: &Path) -> String {
    format!("/D{name}={}", path.display())
}

/// `ISCC.exe`: `$ISCC`, `PATH`, then the per-machine and per-user Inno Setup 6 install dirs.
fn find_iscc() -> anyhow::Result<PathBuf> {
    if let Some(p) = std::env::var_os(ISCC_ENV).map(PathBuf::from) {
        if p.is_file() {
            return Ok(p);
        }
        bail!("${ISCC_ENV} points to {}, which doesn't exist", p.display());
    }
    let mut candidates: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| {
            std::env::split_paths(&p)
                .map(|d| d.join("ISCC.exe"))
                .collect()
        })
        .unwrap_or_default();
    for (var, sub) in [
        ("ProgramFiles(x86)", "Inno Setup 6"),
        ("ProgramFiles", "Inno Setup 6"),
        ("LOCALAPPDATA", "Programs/Inno Setup 6"),
    ] {
        if let Some(base) = std::env::var_os(var) {
            candidates.push(Path::new(&base).join(sub).join("ISCC.exe"));
        }
    }
    candidates.into_iter().find(|p| p.is_file()).context(
        "ISCC.exe (Inno Setup 6.3+) not found; install it (`winget install JRSoftware.InnoSetup`) or set $ISCC",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rel_012_stable_asset_names_per_target() {
        let cases = [
            (
                "x86_64-pc-windows-msvc",
                ArtifactKind::Installer,
                "Dockering-Setup-x64.exe",
            ),
            (
                "aarch64-pc-windows-msvc",
                ArtifactKind::Installer,
                "Dockering-Setup-arm64.exe",
            ),
            (
                "x86_64-pc-windows-msvc",
                ArtifactKind::Archive,
                "Dockering-x64.zip",
            ),
            (
                "aarch64-pc-windows-msvc",
                ArtifactKind::Archive,
                "Dockering-arm64.zip",
            ),
            (
                "aarch64-apple-darwin",
                ArtifactKind::Dmg,
                "Dockering-aarch64.dmg",
            ),
            (
                "x86_64-apple-darwin",
                ArtifactKind::Dmg,
                "Dockering-x86_64.dmg",
            ),
            (
                "x86_64-unknown-linux-gnu",
                ArtifactKind::AppImage,
                "Dockering-x86_64.AppImage",
            ),
            (
                "aarch64-unknown-linux-gnu",
                ArtifactKind::Deb,
                "Dockering-aarch64.deb",
            ),
            (
                "x86_64-unknown-linux-gnu",
                ArtifactKind::Archive,
                "Dockering-x86_64.tar.gz",
            ),
        ];
        for (triple, kind, expected) in cases {
            assert_eq!(
                stable_name(triple, kind).unwrap(),
                expected,
                "{triple} {kind:?}"
            );
        }
        assert!(stable_name("aarch64-apple-darwin", ArtifactKind::Installer).is_err());
        assert!(stable_name("x86_64-pc-windows-msvc", ArtifactKind::Dmg).is_err());
    }

    #[test]
    fn rel_formats_per_target() {
        assert_eq!(
            default_formats("x86_64-pc-windows-msvc").unwrap(),
            &["inno"]
        );
        assert_eq!(
            default_formats("aarch64-pc-windows-msvc").unwrap(),
            &["inno"]
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
