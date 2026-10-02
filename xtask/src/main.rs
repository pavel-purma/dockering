//! Dockering build helpers: `cargo xtask <command> [args]` (alias in `.cargo/config.toml`).
//! Each subcommand is a module exposing `pub fn run(args: &[String]) -> anyhow::Result<()>`.

mod wslc_abi;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = args.get(1..).unwrap_or_default();
    match args.first().map(String::as_str) {
        Some("wslc-abi-check") => wslc_abi::run(rest),
        _ => {
            eprintln!("usage: cargo xtask wslc-abi-check <tag|latest> [--against <vendored-tag>]");
            Ok(())
        }
    }
}
