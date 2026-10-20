//! `cargo xtask update-e2e-fixture`: the files the Linux update end-to-end test serves
//! (REL-085; plan `linux-appimage-updates` §6.5 "Fixture and server").
//!
//! It runs on the host and only **writes files**: no server, no process, no network. Into `--out`
//! it writes
//! - a throwaway minisign key pair: `e2e.key` (secret, created `create_new` with mode `0600` on
//!   Unix, never overwritten), `e2e.pub` and `public-key.txt`, which holds the one base64 line the
//!   old AppImage takes as `DOCKERING_UPDATE_E2E_KEY`;
//! - `dockering-update.json` (schema 1, the one platform `linux-<arch>` of kind `appimage`, the
//!   **real** GitHub-style asset URL, the SHA-256 and size of `--appimage`), made by the same
//!   `build_manifest_for` as `update-manifest`, and its signature `dockering-update.json.minisig`,
//!   made by the same `sign_bytes` as `sign-manifest`;
//! - `root/`, a server root that mirrors GitHub's paths, because the seam (`update-e2e`) rewrites
//!   only scheme and host and keeps the path:
//!   `root/pavel-purma/dockering/releases/latest/download/dockering-update.json[.minisig]` and
//!   `root/pavel-purma/dockering/releases/download/v<version>/Dockering-<arch>.AppImage`.
//!
//! `--claimed-version` is for scenario S2: the manifest, its URL and the layout name that version
//! while the AppImage still prints `--version`'s, so the app's pre-swap `--version` self-test
//! fails.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, bail};

use crate::package::{ArtifactKind, stable_name};
use crate::release::{
    MANIFEST, UPDATER_REPO, build_manifest_for, now_rfc3339, sign_bytes, validate_version,
    write_new_file,
};
use crate::util::{Args, validate_ident};

const SECRET_KEY: &str = "e2e.key";
const PUBLIC_KEY: &str = "e2e.pub";
/// The bare base64 public key, the value of `DOCKERING_UPDATE_E2E_KEY`.
const PUBLIC_KEY_LINE: &str = "public-key.txt";
/// The server root, relative to `--out`.
const ROOT: &str = "root";
const VALUE_OPTS: &[&str] = &[
    "--out",
    "--appimage",
    "--version",
    "--arch",
    "--claimed-version",
];
const DEFAULT_ARCH: &str = "x86_64";

struct Options {
    out: PathBuf,
    appimage: PathBuf,
    /// What the AppImage prints for `--version`.
    version: String,
    /// What the manifest says instead, to make the self-test fail (S2).
    claimed: Option<String>,
    arch: String,
}

/// What a run wrote, for the summary and the tests.
#[derive(Debug)]
struct Fixture {
    /// The base64 public key (not a secret).
    public_key: String,
    /// The version the manifest names (`--claimed-version`, else `--version`).
    manifest_version: String,
    /// The AppImage under the server root.
    asset: PathBuf,
    sha256: String,
}

/// `cargo xtask update-e2e-fixture --out <dir> --appimage <file> --version <X.Y.Z>
/// [--arch x86_64|aarch64] [--claimed-version <X.Y.Z>]`
pub fn run(args: &[String]) -> anyhow::Result<()> {
    let opts = parse(args)?;
    let fixture = write_fixture(&opts, &now_rfc3339()?)?;
    let out = opts.out.display();
    println!("xtask: update end-to-end fixture written to {out}");
    println!("xtask:   server root:  {out}/{ROOT}   (scripts/linux-update/serve.py --root)");
    println!(
        "xtask:   public key:   {out}/{PUBLIC_KEY_LINE}   (DOCKERING_UPDATE_E2E_KEY={})",
        fixture.public_key
    );
    println!(
        "xtask:   manifest:     v{} linux-{} {} sha256 {}",
        fixture.manifest_version,
        opts.arch,
        fixture.asset.display(),
        fixture.sha256
    );
    match &opts.claimed {
        Some(claimed) => println!(
            "xtask:   S2: the manifest claims {claimed} but the AppImage must print `dockering {}`: its --version self-test must fail",
            opts.version
        ),
        None => println!(
            "xtask:   the AppImage must print `dockering {}`",
            opts.version
        ),
    }
    Ok(())
}

fn parse(args: &[String]) -> anyhow::Result<Options> {
    let parsed = Args::new(args);
    parsed.reject_unknown(VALUE_OPTS, &[])?;
    let out = PathBuf::from(parsed.value("--out")?.context("--out is required")?);
    let appimage = PathBuf::from(
        parsed
            .value("--appimage")?
            .context("--appimage is required")?,
    );
    let version =
        version_arg("--version", parsed.value("--version")?)?.context("--version is required")?;
    let claimed = version_arg("--claimed-version", parsed.value("--claimed-version")?)?;
    if claimed.as_deref() == Some(version.as_str()) {
        bail!(
            "--claimed-version {version} equals --version: the self-test would pass, which is not scenario S2"
        );
    }
    let arch = parsed.value("--arch")?.unwrap_or(DEFAULT_ARCH).to_owned();
    validate_ident("architecture", &arch)?;
    // The asset name of the platform; an unsupported architecture fails here.
    stable_name(&triple(&arch), ArtifactKind::AppImage)?;
    Ok(Options {
        out,
        appimage,
        version,
        claimed,
        arch,
    })
}

/// A SemVer given as `X.Y.Z` or `vX.Y.Z`.
fn version_arg(name: &str, raw: Option<&str>) -> anyhow::Result<Option<String>> {
    let Some(raw) = raw else { return Ok(None) };
    let version = raw.strip_prefix('v').unwrap_or(raw);
    validate_version(version).with_context(|| format!("{name} must be X.Y.Z"))?;
    Ok(Some(version.to_owned()))
}

fn triple(arch: &str) -> String {
    format!("{arch}-unknown-linux-gnu")
}

/// Everything below is written with `create_new` or after a check that nothing is in the way, and
/// all of it is checked **before** the first byte is written, so a leftover key can't end up next
/// to a new manifest it didn't sign.
fn write_fixture(opts: &Options, pub_date: &str) -> anyhow::Result<Fixture> {
    let meta = fs::metadata(&opts.appimage)
        .with_context(|| format!("reading {}", opts.appimage.display()))?;
    if !meta.is_file() || meta.len() == 0 {
        bail!(
            "--appimage {} must be a nonempty file",
            opts.appimage.display()
        );
    }
    let manifest_version = opts.claimed.as_deref().unwrap_or(&opts.version);
    let asset_name = stable_name(&triple(&opts.arch), ArtifactKind::AppImage)?;
    let platform = format!("linux-{}", opts.arch);

    // GitHub's own paths, below the server root.
    let github = UPDATER_REPO
        .split('/')
        .fold(opts.out.join(ROOT), |path, part| path.join(part))
        .join("releases");
    let release_dir = github.join("download").join(format!("v{manifest_version}"));
    let latest_dir = github.join("latest").join("download");

    let secret_key = opts.out.join(SECRET_KEY);
    let public_key = opts.out.join(PUBLIC_KEY);
    let public_key_line = opts.out.join(PUBLIC_KEY_LINE);
    let manifest_path = opts.out.join(MANIFEST);
    let signature_path = opts.out.join(format!("{MANIFEST}.minisig"));
    for path in [
        &secret_key,
        &public_key,
        &public_key_line,
        &manifest_path,
        &signature_path,
        &opts.out.join(ROOT),
    ] {
        if fs::symlink_metadata(path).is_ok() {
            bail!(
                "{} exists; refusing to overwrite a key or a fixture (write to a new --out)",
                path.display()
            );
        }
    }

    fs::create_dir_all(&release_dir)
        .with_context(|| format!("creating {}", release_dir.display()))?;
    fs::create_dir_all(&latest_dir)
        .with_context(|| format!("creating {}", latest_dir.display()))?;
    let asset = release_dir.join(&asset_name);
    fs::copy(&opts.appimage, &asset)
        .with_context(|| format!("copying {} to {}", opts.appimage.display(), asset.display()))?;

    // The manifest is made from the copy under the server root, so its hash and size are those of
    // the bytes the server will send. Only this platform is in it.
    let manifest = build_manifest_for(
        &release_dir,
        manifest_version,
        UPDATER_REPO,
        pub_date,
        Some(&platform),
    )?;
    let sha256 = sha256_of_manifest_entry(&manifest, &platform)?;

    let kp = minisign::KeyPair::generate_unencrypted_keypair()
        .map_err(|e| anyhow::anyhow!("key generation failed: {e}"))?;
    let signature = sign_bytes(&kp.sk, manifest.as_bytes())?;
    let public_text = kp
        .pk
        .to_box()
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .into_string();
    let secret_text = kp
        .sk
        .to_box(Some("Dockering update end-to-end test key (throwaway)"))
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .into_string();
    let public_line = kp.pk.to_base64();

    write_new_file(&secret_key, &secret_text, true)?;
    write_new_file(&public_key, &public_text, false)?;
    write_new_file(&public_key_line, &format!("{public_line}\n"), false)?;
    for dir in [opts.out.as_path(), latest_dir.as_path()] {
        write_new_file(&dir.join(MANIFEST), &manifest, false)?;
        write_new_file(&dir.join(format!("{MANIFEST}.minisig")), &signature, false)?;
    }
    Ok(Fixture {
        public_key: public_line,
        manifest_version: manifest_version.to_owned(),
        asset,
        sha256,
    })
}

/// The `sha256` of the one platform entry, read back from the manifest text.
fn sha256_of_manifest_entry(manifest: &str, platform: &str) -> anyhow::Result<String> {
    let value: serde_json::Value =
        serde_json::from_str(manifest).context("the manifest is not JSON")?;
    value["platforms"][platform]["sha256"]
        .as_str()
        .map(str::to_owned)
        .with_context(|| format!("the manifest has no `{platform}` entry"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::release::{hash_file, key_line, load_secret_key, pinned_asset_name, verifying_key};
    use serde_json::Value;
    use sha2::{Digest, Sha256};
    use std::collections::BTreeSet;
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const VERSION: &str = "1.2.3";
    const PUB_DATE: &str = "2026-10-19T00:00:00Z";
    const GITHUB: &str = "pavel-purma/dockering/releases";

    /// A scratch directory holding a fake "new AppImage" and, below `out`, the fixture.
    struct Scratch {
        dir: PathBuf,
    }

    impl Scratch {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "xtask-update-e2e-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("new.AppImage"), APPIMAGE).unwrap();
            Self { dir }
        }

        fn appimage(&self) -> PathBuf {
            self.dir.join("new.AppImage")
        }

        fn out(&self) -> PathBuf {
            self.dir.join("fixture")
        }

        fn args(&self, extra: &[&str]) -> Vec<String> {
            let mut args = vec![
                "--out".to_owned(),
                self.out().display().to_string(),
                "--appimage".to_owned(),
                self.appimage().display().to_string(),
                "--version".to_owned(),
                VERSION.to_owned(),
            ];
            args.extend(extra.iter().map(|s| (*s).to_owned()));
            args
        }

        fn write(&self, extra: &[&str]) -> anyhow::Result<Fixture> {
            write_fixture(&parse(&self.args(extra))?, PUB_DATE)
        }

        fn read(&self, relative: &str) -> Vec<u8> {
            fs::read(self.out().join(relative)).unwrap()
        }

        /// Every file below `out`, as `/`-separated relative paths.
        fn files(&self) -> BTreeSet<String> {
            fn walk(dir: &Path, prefix: &str, into: &mut BTreeSet<String>) {
                for entry in fs::read_dir(dir).unwrap() {
                    let entry = entry.unwrap();
                    let name = format!("{prefix}{}", entry.file_name().to_string_lossy());
                    if entry.file_type().unwrap().is_dir() {
                        walk(&entry.path(), &format!("{name}/"), into);
                    } else {
                        into.insert(name);
                    }
                }
            }
            let mut files = BTreeSet::new();
            walk(&self.out(), "", &mut files);
            files
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    /// Not a real AppImage, and no test runs it: only its bytes matter.
    const APPIMAGE: &[u8] = b"\x7fELF fake AppImage of the new version, bytes only";

    fn sha256_hex(data: &[u8]) -> String {
        hex::encode(Sha256::digest(data))
    }

    fn manifest_of(scratch: &Scratch) -> Value {
        serde_json::from_slice(&scratch.read(MANIFEST)).unwrap()
    }

    #[test]
    fn update_e2e_fixture_layout_and_signature() {
        let scratch = Scratch::new();
        let fixture = scratch.write(&[]).unwrap();

        // ONLY these files, and the server root mirrors GitHub's paths.
        let latest = format!("root/{GITHUB}/latest/download");
        let asset = format!("root/{GITHUB}/download/v{VERSION}/Dockering-x86_64.AppImage");
        let expected: BTreeSet<String> = [
            "e2e.key".to_owned(),
            "e2e.pub".to_owned(),
            "public-key.txt".to_owned(),
            "dockering-update.json".to_owned(),
            "dockering-update.json.minisig".to_owned(),
            format!("{latest}/dockering-update.json"),
            format!("{latest}/dockering-update.json.minisig"),
            asset.clone(),
        ]
        .into();
        assert_eq!(scratch.files(), expected);
        assert_eq!(fixture.asset, scratch.out().join(&asset));

        // The AppImage the server sends is byte for byte the one given.
        assert_eq!(scratch.read(&asset), APPIMAGE);
        // The copies under the server root are the files next to the keys.
        for name in [MANIFEST.to_owned(), format!("{MANIFEST}.minisig")] {
            assert_eq!(
                scratch.read(&format!("{latest}/{name}")),
                scratch.read(&name)
            );
        }

        // public-key.txt is exactly one line, the base64 key, with nothing around it.
        let line = String::from_utf8(scratch.read("public-key.txt")).unwrap();
        assert_eq!(line, format!("{}\n", fixture.public_key));
        assert_eq!(line.trim_end().lines().count(), 1);
        let public = String::from_utf8(scratch.read("e2e.pub")).unwrap();
        assert_eq!(
            key_line(&public),
            fixture.public_key,
            "the same key as e2e.pub"
        );
        assert!(
            minisign_verify::PublicKey::from_base64(line.trim_end()).is_ok(),
            "the line parses as the app parses its key"
        );

        // The manifest: schema 1, only this platform, the real GitHub URL, the file's hash and size.
        let manifest = manifest_of(&scratch);
        assert_eq!(manifest["schema"], 1);
        assert_eq!(manifest["version"], VERSION);
        assert_eq!(manifest["pub_date"], PUB_DATE);
        assert_eq!(
            manifest["notes_url"],
            format!("https://github.com/pavel-purma/dockering/releases/tag/v{VERSION}")
        );
        let platforms = manifest["platforms"].as_object().unwrap();
        assert_eq!(platforms.keys().collect::<Vec<_>>(), ["linux-x86_64"]);
        let entry = &platforms["linux-x86_64"];
        let url = entry["url"].as_str().unwrap();
        assert_eq!(
            url,
            format!(
                "https://github.com/pavel-purma/dockering/releases/download/v{VERSION}/Dockering-x86_64.AppImage"
            )
        );
        assert_eq!(entry["kind"], "appimage");
        assert_eq!(entry["size"], APPIMAGE.len());
        assert_eq!(entry["sha256"], sha256_hex(APPIMAGE));
        assert_eq!(fixture.sha256, sha256_hex(APPIMAGE));
        assert_eq!(
            hash_file(&scratch.appimage()).unwrap().0,
            sha256_hex(APPIMAGE),
            "the same hash the release tooling computes"
        );
        // The app's URL pin (UPD-001) accepts it, and the path of the layout is the URL's path.
        assert_eq!(
            pinned_asset_name(url, VERSION),
            Some("Dockering-x86_64.AppImage")
        );
        // The layout is the URL's path below the server root: what the seam's host rewrite needs.
        let path = url.strip_prefix("https://github.com/").unwrap();
        assert_eq!(scratch.out().join("root").join(path), fixture.asset);

        // The signature verifies with the generated key, through the verifier the app uses, and
        // with nothing else.
        let data = scratch.read(MANIFEST);
        let signature = String::from_utf8(scratch.read("dockering-update.json.minisig")).unwrap();
        assert_eq!(
            verifying_key(&data, &signature, std::slice::from_ref(&fixture.public_key)).unwrap(),
            0
        );
        assert_eq!(
            verifying_key(&data, &signature, std::slice::from_ref(&public)).unwrap(),
            0
        );
        let stranger = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        assert!(verifying_key(&data, &signature, &[stranger.pk.to_base64()]).is_err());
        let mut tampered = data.clone();
        tampered.extend_from_slice(b" ");
        assert!(verifying_key(&tampered, &signature, &[fixture.public_key]).is_err());

        // e2e.key is the secret half: it signs what e2e.pub verifies.
        let secret = String::from_utf8(scratch.read("e2e.key")).unwrap();
        let sk = load_secret_key(&secret, None).unwrap();
        let again = sign_bytes(&sk, b"another manifest").unwrap();
        assert_eq!(
            verifying_key(b"another manifest", &again, &[public]).unwrap(),
            0
        );
    }

    #[cfg(unix)]
    #[test]
    fn update_e2e_fixture_secret_key_is_0600() {
        use std::os::unix::fs::PermissionsExt as _;
        let scratch = Scratch::new();
        scratch.write(&[]).unwrap();
        let mode = |name: &str| {
            fs::metadata(scratch.out().join(name))
                .unwrap()
                .permissions()
                .mode()
                & 0o777
        };
        assert_eq!(mode("e2e.key"), 0o600);
        assert_eq!(mode("e2e.pub") & 0o022, 0);
    }

    #[test]
    fn update_e2e_fixture_claimed_version() {
        let scratch = Scratch::new();
        let fixture = scratch.write(&["--claimed-version", "9.9.9"]).unwrap();
        assert_eq!(fixture.manifest_version, "9.9.9");

        // The manifest, its URL and the layout name the claimed version, and only that one.
        let manifest = manifest_of(&scratch);
        assert_eq!(manifest["version"], "9.9.9");
        let url = manifest["platforms"]["linux-x86_64"]["url"]
            .as_str()
            .unwrap();
        assert_eq!(
            url,
            "https://github.com/pavel-purma/dockering/releases/download/v9.9.9/Dockering-x86_64.AppImage"
        );
        assert_eq!(
            pinned_asset_name(url, "9.9.9"),
            Some("Dockering-x86_64.AppImage"),
            "the URL pin of the app still passes, so only the self-test can fail"
        );
        assert_eq!(pinned_asset_name(url, VERSION), None);
        let served = format!("root/{GITHUB}/download/v9.9.9/Dockering-x86_64.AppImage");
        assert_eq!(scratch.read(&served), APPIMAGE);
        assert!(
            !scratch
                .files()
                .iter()
                .any(|file| file.contains(&format!("v{VERSION}"))),
            "no directory for the version the AppImage really prints: {:?}",
            scratch.files()
        );
        // Signed like any other fixture, and hashes still match the bytes served.
        let data = scratch.read(MANIFEST);
        let signature = String::from_utf8(scratch.read("dockering-update.json.minisig")).unwrap();
        assert_eq!(
            verifying_key(&data, &signature, &[fixture.public_key]).unwrap(),
            0
        );
        assert_eq!(
            manifest["platforms"]["linux-x86_64"]["sha256"],
            sha256_hex(APPIMAGE)
        );

        // A claim equal to the real version would pass the self-test: that is not S2.
        let error = parse(&scratch.args(&["--claimed-version", "v1.2.3"]))
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("equals --version"), "{error}");
        assert!(parse(&scratch.args(&["--claimed-version", "9.9"])).is_err());
    }

    #[test]
    fn update_e2e_fixture_refuses_to_overwrite_keys() {
        let scratch = Scratch::new();
        let first = scratch.write(&[]).unwrap();
        let key = scratch.read("e2e.key");
        let before = scratch.files();

        // A second run into the same directory is refused and changes nothing.
        let error = scratch.write(&[]).unwrap_err().to_string();
        assert!(error.contains("refusing to overwrite"), "{error}");
        assert_eq!(scratch.read("e2e.key"), key);
        assert_eq!(scratch.files(), before);
        assert_eq!(
            String::from_utf8(scratch.read("public-key.txt")).unwrap(),
            format!("{}\n", first.public_key)
        );

        // A leftover key alone stops the run before anything else is written.
        let other = Scratch::new();
        fs::create_dir_all(other.out()).unwrap();
        fs::write(other.out().join("e2e.key"), "precious").unwrap();
        let error = other.write(&[]).unwrap_err().to_string();
        assert!(error.contains("e2e.key"), "{error}");
        assert_eq!(other.read("e2e.key"), b"precious");
        assert_eq!(other.files(), BTreeSet::from(["e2e.key".to_owned()]));

        // Through the command line too.
        assert!(run(&scratch.args(&[])).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn update_e2e_fixture_does_not_follow_a_planted_symlink() {
        let scratch = Scratch::new();
        fs::create_dir_all(scratch.out()).unwrap();
        let victim = scratch.dir.join("victim");
        std::os::unix::fs::symlink(&victim, scratch.out().join("e2e.key")).unwrap();
        assert!(scratch.write(&[]).is_err());
        assert!(!victim.exists(), "a key is never written through a symlink");
        assert!(!scratch.out().join("root").exists());
    }

    #[test]
    fn update_e2e_fixture_other_architecture_and_prefixed_versions() {
        let scratch = Scratch::new();
        let mut args = scratch.args(&["--arch=aarch64"]);
        args[5] = "v1.2.3".to_owned();
        write_fixture(&parse(&args).unwrap(), PUB_DATE).unwrap();
        let manifest = manifest_of(&scratch);
        assert_eq!(manifest["version"], "1.2.3", "a leading v is dropped");
        assert_eq!(
            manifest["platforms"]
                .as_object()
                .unwrap()
                .keys()
                .collect::<Vec<_>>(),
            ["linux-aarch64"]
        );
        assert!(scratch.files().contains(&format!(
            "root/{GITHUB}/download/v1.2.3/Dockering-aarch64.AppImage"
        )));
    }

    #[test]
    fn update_e2e_fixture_arguments_are_validated() {
        let scratch = Scratch::new();
        let without = |name: &str| {
            let args = scratch.args(&[]);
            let at = args.iter().position(|a| a == name).unwrap();
            [&args[..at], &args[at + 2..]].concat()
        };
        for name in ["--out", "--appimage", "--version"] {
            let error = parse(&without(name)).err().unwrap().to_string();
            assert!(error.contains(name), "{name}: {error}");
        }
        for extra in [
            vec!["--bogus", "1"],
            vec!["stray"],
            vec!["--arch", "x86_64;rm"],
            vec!["--arch", "riscv64"],
            vec!["--claimed-version", "not-a-version"],
        ] {
            assert!(parse(&scratch.args(&extra)).is_err(), "{extra:?}");
        }
        let mut bad_version = scratch.args(&[]);
        bad_version[5] = "1.2".to_owned();
        assert!(parse(&bad_version).is_err());

        // The AppImage must be a nonempty file; nothing is written otherwise.
        fs::write(scratch.appimage(), b"").unwrap();
        assert!(scratch.write(&[]).is_err());
        fs::remove_file(scratch.appimage()).unwrap();
        assert!(scratch.write(&[]).is_err());
        assert!(
            !scratch.out().exists(),
            "nothing is written for a bad input"
        );
    }
}
