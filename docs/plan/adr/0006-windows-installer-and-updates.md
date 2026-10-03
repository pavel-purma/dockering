# ADR-0006: Windows installer, signing, and in-app updates

- **Status:** proposed (2026-10-02)
- **Spec:** [features/distribution.md](../../spec/features/distribution.md) (REL-010…060, UPD-001…012)
- **Plan:** [windows-distribution](../features/windows-distribution.md)

## Context

v1 packages Windows with cargo-packager: a WiX `.msi` on x64 and NSIS on arm64 (WiX 3 can't
target ARM64). It signs with Azure Trusted Signing and has no update mechanism. NFR-023 and
spec 50 *Updates* forbid any update check. We now want:

- a polished per-user installer,
- winget,
- an auto-update that uses GitHub Releases as its only storage, inspired by Zed (and Delta, Zed
  Industries' GPUI app, which ships the same EXE-installer shape).

The maintainer is an individual in the EU.

Facts that shaped this (checked 2026-10-02, sources in the plan's Appendix):

- **Zed** uses Inno Setup 6 (`crates/zed/resources/windows/zed.iss`):
  - per-user by default, a separate AppId per channel;
  - signed with Azure Trusted Signing from GitHub Actions;
  - updates come from Zed's own API (`cloud.zed.dev`), not GitHub;
  - the app downloads the setup and runs it with `/verysilent /update=true`, which stages files
    into `{app}\install`;
  - on quit, `auto_update_helper.exe` swaps the files using the Restart Manager, with rollback,
    then relaunches.
  - `auto_update` and `auto_update_helper` are **GPL-3.0-or-later**, so they are reference-only
    (REL-003). We copy no code.
- **cargo-packager 0.11.8** has no Inno support. Its updater (Tauri-style, minisign) runs a whole
  installer and has no UI.
- **Velopack** (MIT, Rust crate, `GithubSource`, delta updates) is a good fit technically. But its
  installer is a one-click `Setup.exe` with no wizard or branding, it needs the .NET `vpk` tool in
  CI, and it owns the install layout (`%LocalAppData%\<id>\current`).
- **winget** accepts `inno` natively (silent switches implied) and doesn't require signing. It runs
  Defender/AV checks and URL-reputation checks. Unsigned, never-seen binaries raise SmartScreen
  warnings and Defender heuristics (spec 50 already notes this).
- **Azure Artifact Signing** is open to *individuals* only in the US and Canada. EU *organisations*
  are eligible. **SignPath Foundation** signs OSS projects for free from GitHub Actions; the
  certificate subject is "SignPath Foundation". **Certum Open Source** (€49/yr) issues a
  certificate in the developer's name, but its cloud HSM needs an interactive OTP, which is awkward
  in CI. EV no longer bypasses SmartScreen.

## Options

1. **Keep cargo-packager MSI/NSIS + cargo-packager-updater.** Least work. But it means two
   installer technologies, a plain wizard, a full installer UI on every update, and no
   per-user/all-users choice on x64.
2. **Velopack.** Delta updates and less updater code. But it has no install wizard, brings a .NET
   CI dependency, a foreign install layout, and less control over the UX the user asked for.
3. **Inno Setup (Zed-style) + our own small updater (`dk-update`) reading a minisign-signed
   manifest from the latest GitHub Release.**

## Decision

Option 3.

- **One installer technology** for x64 and arm64 (Inno 6.3+ supports arm64): branded modern
  wizard, per-user default with an all-users option. Silent switches work for winget, and the
  same installer is the update payload.
- **Updater v1 = "verified silent re-install".** Download → verify (minisign manifest + SHA-256 +
  Authenticode subject) → on *Restart to update*, spawn the installer with
  `/SILENT … /UPDATE /RELAUNCH` and quit. Inno's `AppMutex` and Restart Manager wait for the app,
  replace the files, and relaunch. No privileged helper and no custom file-swap code; this is
  about 1/10 of Zed's mechanism and has the same UX for the user apart from a brief progress
  window.
- **Upgrade path (MAY, later):** a Zed-style staged install (`/UPDATE` stages into `{app}\install`)
  plus a tiny Apache/MIT `dockering-update-helper.exe` that swaps the files after exit. This would
  give zero-UI updates. It is written from the design, never from Zed's GPL code.
- **Signing:** provider-agnostic two-stage signing in `release.yml` (binaries, then the setup).
  Target **SignPath Foundation** for the public launch. Keep the Azure wiring as an alternative if
  an eligible organisation exists later. Certum OSS is the fallback if SignPath declines.
- **Privacy:** the updater is compiled in only for public release builds and is opt-out at runtime
  (setting, env var, and Windows policy). It makes only anonymous HTTPS GETs to GitHub. NFR-023 is
  amended.

## Consequences

- `xtask package` gains an `inno` format and calls `ISCC.exe` (preinstalled on `windows-2025` runners ⚠ verify; otherwise `choco install innosetup`).
- `.msi` is dropped. Anyone deploying through GPO/Intune uses the setup's `/ALLUSERS /VERYSILENT` or winget.
- A new crate, `dk-update`:
  - no GPUI;
  - depends on `tokio`, `reqwest` (rustls, `rustls-platform-verifier`), `minisign-verify`, `semver`, `sha2`;
  - Windows-only `WinVerifyTrust` sits behind `cfg(windows)`, in a module with `unsafe` allowed and documented.
  - `dk-hub` owns it (layering: UI → hub → dk-update).
- A new secret, `UPDATE_SIGNING_KEY`. Losing it means shipping a release whose manifest is signed
  by a new key, which old builds would reject. **Mitigation:** the app embeds **two** public keys
  (current + next) from the start.
- If SignPath signs, the publisher shown is "SignPath Foundation", and UPD-003's subject pinning
  uses that subject.
