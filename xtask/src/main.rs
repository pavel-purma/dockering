//! Dockering build helpers: `cargo xtask <command> [args]` (alias in `.cargo/config.toml`).
//!
//! Every subcommand is a module exposing `pub fn run(args: &[String]) -> anyhow::Result<()>`,
//! where `args` are the arguments after the command name. To add one, declare the module and
//! add a single arm to the `match` in `main`.

mod check_blocking;
mod fixtures;
mod package;
mod util;
mod wslc_abi;

use anyhow::bail;

const USAGE: &str = "\
usage: cargo xtask <command> [args]

commands:
  check-blocking                      NFR-001 blocking-call check over UI-thread sources
  record-fixtures --engine <id>       record engine fixtures against a live engine (60-quality)
  package [--target <triple>] [--formats <list>] [--no-archive]
                                      package an already-built release binary (cargo-packager)
  dist [--target <triple>] [--formats <list>] [--no-archive]
                                      cargo build --release + package";

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((command, rest)) = args.split_first() else {
        println!("{USAGE}");
        return Ok(());
    };

    match command.as_str() {
        "check-blocking" => check_blocking::run(rest),
        "record-fixtures" => fixtures::run(rest),
        "package" => package::run(rest),
        "dist" => package::run_dist(rest),
        "wslc-abi-check" => wslc_abi::run(rest),
        "help" | "-h" | "--help" => {
            println!("{USAGE}");
            Ok(())
        }
        other => bail!("unknown xtask command `{other}`\n\n{USAGE}"),
    }
}
