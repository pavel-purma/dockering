//! Dockering build helpers: `cargo xtask <command> [args]` (alias in `.cargo/config.toml`).
//!
//! Every subcommand is a module exposing `pub fn run(args: &[String]) -> anyhow::Result<()>`,
//! where `args` are the arguments after the command name. To add one, declare the module and
//! add a single arm to the `match` in `main`.

mod check_blocking;
mod fixtures;
mod icons;
mod package;
mod release;
mod util;
mod wslc_abi;

use anyhow::bail;

const USAGE: &str = "\
usage: cargo xtask <command> [args]

commands:
  check-blocking                      NFR-001 blocking-call check over UI-thread sources
  icons [--check] [--social]          generate or verify REL-050 icon and branding assets
  record-fixtures --engine <id>       record engine fixtures against a live engine (60-quality)
  package [--target <triple>] [--formats <list>] [--no-archive]
                                      package an already-built release binary (cargo-packager)
  dist [--target <triple>] [--formats <list>] [--no-archive]
                                      cargo build --release + package
  update-manifest --assets <dir> --version <semver> [--repo <owner/name>] [--out <file>]
                                      write dockering-update.json from release assets (UPD-002)
  verify-assets --assets <dir> [--target <triple>]
                                      require every nonempty distribution package (REL-017)
  sign-manifest <file>                minisign-sign <file> with $UPDATE_SIGNING_KEY (UPD-003)
  gen-update-keys <dir>               generate the current + next updater key pairs (UPD-003)
  checksums <dir>                     write <dir>/SHA256SUMS (REL-012)";

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((command, rest)) = args.split_first() else {
        println!("{USAGE}");
        return Ok(());
    };

    match command.as_str() {
        "check-blocking" => check_blocking::run(rest),
        "icons" => icons::run(rest),
        "record-fixtures" => fixtures::run(rest),
        "package" => package::run(rest),
        "dist" => package::run_dist(rest),
        "wslc-abi-check" => wslc_abi::run(rest),
        "update-manifest" => release::run_update_manifest(rest),
        "verify-assets" => release::run_verify_assets(rest),
        "sign-manifest" => release::run_sign_manifest(rest),
        "gen-update-keys" => release::run_gen_update_keys(rest),
        "checksums" => release::run_checksums(rest),
        "help" | "-h" | "--help" => {
            println!("{USAGE}");
            Ok(())
        }
        other => bail!("unknown xtask command `{other}`\n\n{USAGE}"),
    }
}
