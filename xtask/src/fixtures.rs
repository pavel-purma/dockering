//! `cargo xtask record-fixtures --engine <id>` (60-quality, Fixtures).
//!
//! Recording happens inside each engine crate's ignored `record*` tests, so it can reuse the crate's
//! transport and sanitiser. This command only selects the crate and passes the engine through the
//! environment:
//! - `DOCKERING_RECORD_FIXTURES=1` turns recording on (otherwise the tests are no-ops),
//! - `DOCKERING_ENGINE=<id>` selects the engine (a configured engine id or a connection preset).

use crate::util::{Args, cargo, run as run_cmd, validate_ident};
use anyhow::bail;

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let parsed = Args::new(args);
    parsed.reject_unknown(&["--engine"], &[])?;
    let Some(engine) = parsed.value("--engine")? else {
        bail!("usage: cargo xtask record-fixtures --engine <id>");
    };
    validate_ident("engine id", engine)?;

    let mut cmd = cargo();
    cmd.env("DOCKERING_RECORD_FIXTURES", "1")
        .env("DOCKERING_ENGINE", engine);
    if engine == "wslc" {
        // WSLC fixtures (CLI output + COM responses) need Windows with WSL >= 2.9.3.
        cmd.args(["test", "-p", "dk-engine-wslc", "--", "--ignored", "record"]);
    } else {
        cmd.args([
            "test",
            "-p",
            "dk-engine-docker",
            "--features",
            "it",
            "--",
            "--ignored",
            "record_fixtures",
        ]);
    }
    run_cmd(&mut cmd)
}
