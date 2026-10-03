//! Windows executable resources (REL-027): the app icon as resource ID 1 (GPUI's Windows backend
//! loads icon 1 for the window and taskbar) and a `VERSIONINFO` block from the crate version.
//! GPUI embeds its own manifest (`RT_MANIFEST` 1); the types differ, so the IDs don't clash.

use std::env;
use std::fmt::Write as _;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let icon = manifest_dir.join("../../assets/app-icon/icon.ico");
    println!("cargo:rerun-if-changed={}", icon.display());

    let version = env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION");
    let numeric = |name: &str| env::var(name).ok().and_then(|v| v.parse::<u16>().ok()).unwrap_or(0);
    let (major, minor, patch) = (
        numeric("CARGO_PKG_VERSION_MAJOR"),
        numeric("CARGO_PKG_VERSION_MINOR"),
        numeric("CARGO_PKG_VERSION_PATCH"),
    );
    // rc.exe string literals: backslashes must be doubled.
    let icon_path = icon.display().to_string().replace('\\', "\\\\");

    let mut rc = String::new();
    let _ = writeln!(rc, "#pragma code_page(65001)");
    let _ = writeln!(rc, "1 ICON \"{icon_path}\"");
    let _ = writeln!(rc, "1 VERSIONINFO");
    let _ = writeln!(rc, "FILEVERSION {major},{minor},{patch},0");
    let _ = writeln!(rc, "PRODUCTVERSION {major},{minor},{patch},0");
    let _ = writeln!(rc, "FILEOS 0x40004");
    let _ = writeln!(rc, "FILETYPE 0x1");
    rc.push_str("BEGIN\n  BLOCK \"StringFileInfo\"\n  BEGIN\n    BLOCK \"040904B0\"\n    BEGIN\n");
    for (key, value) in [
        ("CompanyName", "Dockering contributors"),
        ("FileDescription", "Dockering"),
        ("ProductName", "Dockering"),
        ("InternalName", "dockering"),
        ("OriginalFilename", "dockering.exe"),
        (
            "LegalCopyright",
            "Copyright (c) 2026 Dockering contributors. MIT OR Apache-2.0.",
        ),
        ("FileVersion", version.as_str()),
        ("ProductVersion", version.as_str()),
    ] {
        let _ = writeln!(rc, "      VALUE \"{key}\", \"{value}\"");
    }
    rc.push_str("    END\n  END\n  BLOCK \"VarFileInfo\"\n  BEGIN\n    VALUE \"Translation\", 0x409, 1200\n  END\nEND\n");

    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR")).join("dockering.rc");
    std::fs::write(&out, rc).expect("write dockering.rc");
    embed_resource::compile_for(&out, ["dockering"], embed_resource::NONE)
        .manifest_optional()
        .expect("compile Windows resources");
}
