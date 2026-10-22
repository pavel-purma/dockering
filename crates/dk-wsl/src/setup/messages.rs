//! User-facing setup errors: plain words and a hint (ENG-156). The UI shows `to_string()` and
//! `hint()`, which only an `Unreachable` error carries.

use std::path::Path;

use dk_core::EngineError;

pub(crate) fn fail(reason: impl Into<String>, hint: impl Into<String>) -> EngineError {
    EngineError::unreachable_with_hint(reason, hint)
}

/// Request rejected before any I/O.
pub(crate) fn rejected(message: &str) -> EngineError {
    EngineError::Api {
        status: 0,
        message: message.into(),
    }
}

pub(crate) fn not_windows() -> EngineError {
    EngineError::unreachable("WSL is only available on Windows")
}

pub(crate) fn wsl_missing() -> EngineError {
    fail(
        "Windows Subsystem for Linux (WSL) isn't installed.",
        "Install WSL from the Microsoft Store, or see https://aka.ms/wsl, then try again.",
    )
}

pub(crate) fn wsl_too_old(version: &str) -> EngineError {
    fail(
        format!("Your WSL is too old (version {version})."),
        "Run `wsl --update` and try again.",
    )
}

pub(crate) fn virtualization_off() -> EngineError {
    fail(
        "WSL can't start because virtualization isn't turned on.",
        "Turn on virtualization in your computer's firmware settings and enable \"Virtual Machine Platform\", then try again.",
    )
}

pub(crate) fn already_exists(distro: &str) -> EngineError {
    fail(
        format!("A distro named \"{distro}\" already exists."),
        "Nothing was changed. Use it from the engine list.",
    )
}

pub(crate) fn folder_in_use(dir: &Path) -> EngineError {
    fail(
        "The engine's folder already contains files.",
        format!("Remove or rename {} and try again.", dir.display()),
    )
}

/// `drive C:` for a Windows path, else `this drive`.
pub(crate) fn drive_label(path: &Path) -> String {
    let text = path.to_string_lossy();
    let mut chars = text.chars();
    match (chars.next(), chars.next()) {
        (Some(letter), Some(':')) if letter.is_ascii_alphabetic() => {
            format!("drive {}:", letter.to_ascii_uppercase())
        }
        _ => "this drive".to_owned(),
    }
}

pub(crate) fn gigabytes(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1e9)
}

pub(crate) fn low_disk(drive: &str, needed: u64) -> EngineError {
    fail(
        format!(
            "There isn't enough free space on {drive} (about {} needed).",
            gigabytes(needed)
        ),
        format!("Free up space on {drive} and try again."),
    )
}

pub(crate) fn no_network() -> EngineError {
    fail(
        "The download didn't finish.",
        "Check your internet connection and try again.",
    )
}

pub(crate) fn http_status(status: u16) -> EngineError {
    fail(
        format!("The download server answered with an error ({status})."),
        "Try again later.",
    )
}

pub(crate) fn verification_failed() -> EngineError {
    fail(
        "The download failed verification.",
        "Try again. If it keeps failing, check your connection and any proxy that changes downloads.",
    )
}

pub(crate) fn cannot_save(drive: &str) -> EngineError {
    fail(
        "The download couldn't be saved.",
        format!("Free up space on {drive} and try again."),
    )
}

pub(crate) fn stopped_unexpectedly() -> EngineError {
    fail("Setup stopped unexpectedly.", "Try again.")
}

pub(crate) fn docker_not_ready(seconds: u64) -> EngineError {
    fail(
        format!("Docker didn't start within {seconds} s."),
        "Try again. If it keeps failing, run `wsl --update` first.",
    )
}
