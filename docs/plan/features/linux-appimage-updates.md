# Plan: One shared updater for Windows, macOS and Linux, installing without OS code signing

- **Slug:** `linux-appimage-updates` (kept so existing links work; the scope is no longer Linux-only)
- **Status:** approved (2026-10-19), in progress <!-- implementation follows §7 task order; each task updates this line when the plan finishes -->
- **Spec:** [distribution.md](../../spec/features/distribution.md) §7 (UPD) and §9 (REL-083…087) · [ADR-0007 (accepted)](../adr/0007-cross-platform-updater.md)
- **Milestone:** none; follows v0.3.0, the first updater release is the next release after approval
- **Requirement IDs:** UPD-013, UPD-014, UPD-015, UPD-017, REL-083…087 (new) · UPD-016 (new, **deferred**) · UPD-003, UPD-005…UPD-012, REL-012, REL-016, REL-017, REL-031, REL-032, REL-060, SET-090, KBD-076, SHL-022 (changed)
- **Created:** 2026-10-18 · **Revised:** 2026-10-19

## 1. Goal

Dockering finds new versions by itself, on every platform, the way most desktop apps do. It checks
GitHub Releases quietly on a schedule, downloads in the background where it can install the update
itself, shows **one notification** per version and a **title-bar button** (the same icon, place and
behaviour on Windows, macOS and Linux), and installs on the user's click. **OS code signing is not
needed**: updates are trusted through a minisign-signed manifest plus SHA-256, so unsigned Windows
and Linux builds update themselves too.

| Platform | Discovery, notification, button, Settings | *Restart to update* |
|---|---|---|
| Windows (Inno install, per user or all users) | shared | runs the verified installer (existing path, hardened) |
| Linux AppImage in a writable place | shared | replaces the AppImage in place and relaunches (new) |
| Everything else (macOS, `.deb`, `.tar.gz`, portable zip, read-only installs) | shared | not offered; the button opens the release page (*Download*) |

Today the updater exists (UPD-001…012, crate `dk-update`) but is compiled in only when OS signing is
on, so no published build ever checks; its trust keys are empty; Linux and macOS can only notify; and an
available update shows only as a status-bar button.

## 2. Scope

**In:** one hub coordinator and one UI for all platforms (UPD-004…012, UPD-015, UPD-017); a retained
offer that survives restarts and re-checks; the AppImage strategy (UPD-013, UPD-014); the Windows
installer hardening and unsigned-install trust (UPD-003, UPD-007); the independent release gate
(REL-083, REL-084); the Linux end-to-end test and the test seam (REL-085, REL-086); the Windows
verification (REL-087); the relaunch handshake (UPD-017, SHL-022).

**Out:** **in-app installation on macOS** (UPD-016, deferred: nothing produces a hardened-runtime,
notarised `.app` today, and a bundle exchange needs `unsafe` FFI; macOS gets the shared flow and
*Download*); `.deb`/`.tar.gz`/Flatpak/Snap/portable-zip installation; delta updates, zsync, Sparkle;
a *Preview* channel; a rollback UI or a kept previous copy; applying an update without the user's
click; replacing Inno. No production code, keys, releases or changelog edits come with this plan.

## 3. Decisions and assumptions

Settled by common practice instead of by questions; each can be reversed by the maintainer.

| # | Decision | Why |
|---|---|---|
| 1 | **Trust = signed manifest + size + SHA-256**; an OS signature is an extra pin only when the *running* app has one. An unsigned Windows release installs on manifest trust; a signed exe never accepts an unsigned installer. | OS signing isn't available; the manifest signature already protects installs the same way on every platform. |
| 2 | **Key custody** follows [signing.md Phase 4](../../signing.md), with the maintainer running key generation and `gh secret set` (no agent holds a private key); the agent only gets the two public keys. Tests use throwaway keys. | The earlier text had the agent generate the keys. Task 0. |
| 3 | **macOS offers *Download* only**; UPD-016 is deferred with its prerequisites listed. | No macOS scenario was requested. |
| 4 | **The offer mirrors the latest authenticated manifest** that is newer than the running version. Failed checks keep it; a successful check with nothing newer withdraws it; a manifest lower than the offer (a pulled release) makes the old offer non-actionable at once; an offer unconfirmed for 14 days expires, checked whenever it is read (also with checks off, also in a long offline session, also at apply), and a clock-skewed `confirmed_at` counts as expired. | Pulled releases must disappear (this project already withdrew a build); an offline copy mustn't linger. |
| 5 | **Button and notification appear when the update is actionable**: install strategies when the verified download is ready (a few seconds after it is found; progress is in the status bar), *Download* when it is found. | One toast, one button, never a button that can't act. |
| 6 | **A click restarts at once**, no confirmation (as today). Known gap unchanged: running terminal sessions and image pulls aren't listed first. | Matches the current status-bar button. |
| 7 | **The download starts automatically** for install strategies (12–19 MB) unless an opt-out applies; *Download* offers fetch only the manifest. | Same as VS Code or Zed. |
| 8 | **No previous copy** is kept. The `--version` self-test before the swap is the safety net; every release stays downloadable. | A copy with no restore path is dead weight. |
| 9 | **A failed install degrades to *Download*** for that version (self-test or changed payload); a disk or permission error keeps *Ready* for a retry. | No endless retry loop. |
| 10 | **The first updater release is installed by hand** (v0.1.0…v0.3.0 have no updater); a live proof needs a second release after it. | Nothing older can update itself. |
| 11 | **Windows behaviour beyond the existing installer smoke tests is unverified** and is spike W1 (§6.6). This Linux session proves nothing about Windows. | Skill rule: never invent native behaviour. |
| 12 | **Settings, `state.json`:** only `last_check`, `last_result`, `last_run_version`, `notified_version` stay in `state.json`; the offer and payload live under `<data-local>/updates/` (`state.json` is in the roaming profile on Windows). | Local paths and a signed manifest must not roam or be rewritten on every window move. |
| 13 | **Local runs use Docker on a developer machine**; on a non-Ubuntu host (for example Arch) AppImages built locally may fail in an Ubuntu 24.04 container (glibc), so the runner takes CI artifacts or builds inside the container (§6.5). | Observed constraint. |
| 14 | **The install smoke tests set `DOCKERING_DISABLE_UPDATES=1`**: once `UPDATER=all` ships, a real-mode launch would check GitHub 30 s in, which is noise in REL-075 and REL-078 (task 12). | The end-to-end test is the only updater scenario. |

## 4. Requirements (as written into the spec)

Full text: [distribution.md §7 and §9](../../spec/features/distribution.md).

| ID | Requirement (short) | New/Changed |
|---|---|---|
| UPD-013 | Linux AppImage install kind: a real AppImage in `APPIMAGE`, the exe under `APPDIR`, a writable directory; otherwise *Download*. | New |
| UPD-014 | AppImage apply: stage in the target's directory, `--version` self-test, `rename`, scrubbed-environment relaunch with `--update-relaunch=1`; failures leave the old file. | New |
| UPD-015 | Title-bar button derived from the offer's stage; identical on every platform. | New |
| UPD-016 | macOS in-app install. | New, **deferred** |
| UPD-017 | Retained offer (`offer.json`): restore, keep, withdraw, expire; stdin-EOF relaunch wait. | New |
| REL-083 | `UPDATER=none\|all` (+ dispatch input `inherit\|none\|all`), independent of OS signing; `sign_manifest` formula. | New |
| REL-084 | Per-target updater evidence, `verify-update-artifacts`, current-key check, draft gate. | New |
| REL-085 | `linux-update-e2e.yml` and `scripts/linux-update/run.sh`: scenarios S1–S4 on a real AppImage. | New |
| REL-086 | The test seam never ships; one `app_version()`. | New |
| REL-087 | Native acceptance: Linux S1–S4; Windows spike W1, installer-smoke update leg, Windows tests. | New |
| UPD-003, UPD-005…UPD-012 | Unsigned installs on manifest trust; shared gate; install-strategy-only differences; share-locked installer; one notification and the title-bar button; `ApplyOutcome`, apply flag; files under `<data-local>/updates/`. | Changed |
| REL-012, REL-016, REL-017, REL-031, REL-032, REL-060, SET-090, KBD-076, SHL-022 | `.minisig` follows `sign_manifest`; unsigned mode skips only OS signing; rehearsal needs no signing; Settings and palette actions are shared; the update relaunch exception. | Changed |

## 5. Engine contract impact

None. No `Engine` method, DTO or `Capability` changes, and nothing branches on `EngineKind` (ENG-030).
The engine mapping table does not apply (Docker, WSL distro, WSLC COM and CLI: N/A).

Updater types (`dk-update` and `dk-hub`; no GPUI, no new dependency):

```rust
// dk-update/src/install_kind.rs: macOS stays `Other` (UPD-016 is deferred)
pub enum InstallKind { InnoUser, InnoMachine, Portable, AppImage /* new */, Other }
pub struct AppImageEnv { pub appimage: Option<OsString>, pub appdir: Option<OsString>, pub exe: PathBuf }
pub fn detect_appimage(env: &AppImageEnv, probe: &dyn AppImageProbe) -> Option<PathBuf>; // rules pure; the probe reads magic bytes and the directory

// dk-update/src/offer.rs (new, pure + file store): the retained offer (UPD-017); only Downloading, Ready, InstalledRestart are saved
pub struct StoredOffer { manifest: Vec<u8>, signature: String, confirmed_at: String, stage: OfferStage, install_failed: bool }
pub enum OfferStage { Downloading, Ready, InstalledRestart }
pub fn restore(dir: &Path, keys: &[&str], running: &Version, platform: &str, now: SystemTime) -> Option<StoredOffer>; // signature, version, 14-day rule
pub fn is_fresh(offer: &StoredOffer, now: SystemTime) -> bool; // used by restore, every snapshot and apply
pub fn after_check(prev: Option<StoredOffer>, outcome: &CheckOutcome, running: &Version) -> OfferChange; // keep | refresh | replace | withdraw | make_non_actionable

// dk-update/src/apply/{mod,appimage}.rs (apply.rs becomes a directory): cfg(target_os = "linux"), other OSes an `Unsupported` stub
pub fn apply_appimage(job: AppImageJob<'_>) -> Result<AppImageOutcome, UpdateError>; // stage → self-test → rename → relaunch
// error.rs: + SelfTest(String), Stage(String); source.rs: GithubSource::with_base(app_version, base) under any(test, test-support, update-e2e)

// dk-hub: the only public change. These DTOs live ungated next to `UpdateStatus` (dk-update is an optional dependency
// behind the `updater` feature, so a UI build without it must still compile); the hub maps them from dk-update's storage types.
pub enum ApplyOutcome { Restarting, InstalledRestartManually }
pub enum OfferMode { Install, Download }
pub enum OfferStage { Downloading, Ready, Applying /* in memory only */, InstalledRestart }
pub struct UpdateSnapshot { pub status: UpdateStatus /* activity only */, pub offer: Option<UpdateOffer>, pub revision: u64 }
pub struct UpdateOffer { pub version: String, pub notes_url: String, pub mode: OfferMode,
                         pub stage: OfferStage, pub needs_elevation: bool, pub install_failed: bool }
// HubHandle: update_status() -> HubStream<UpdateSnapshot>; check_for_updates() unchanged; apply_update() -> HubCall<ApplyOutcome>
// dk_hub::single_instance::wait_for_predecessor(reader, timeout) -> WaitResult { waited_ms, timed_out, outcome }
//   (reader injected; helper thread + recv_timeout; a read error counts as EOF; Unix)
```

`UpdateStatus` stays as the activity (`Disabled`, `Idle`, `Checking`, `Downloading`, `Error`); the offer is separate, so
checking never hides it. The UI sees only owned `Send` DTOs, no paths and no native types.

## 6. Design

### 6.1 Data flow & threading

```
offer.json (signed manifest + stage)  ──restore at start, offline: signature + version; freshness on every read──▶  button / Settings
check ok, newer ──▶ replace offer (install: after the new payload verifies)      check ok, nothing newer ──▶ withdraw offer + payload
check ok, lower than the offer (pulled) ──▶ old offer non-actionable at once      check failed ──▶ nothing changes
running version ≥ offer ──▶ delete offer                                           unconfirmed > 14 days (any read) ──▶ expired, dropped
stages:  Downloading ─▶ Ready ─click─▶ Applying (memory only) ─▶ [Windows: installer started, app quits] [Linux: swap, relaunch, app quits]
                                          └─ Linux: installed but new process not started ─▶ InstalledRestart (click retries the start)
```

- **Offer (UPD-017):** pure rules in `dk-update/src/offer.rs`, tested without I/O. `offer.json` is written with temp
  file, `fsync`, `rename` (and a directory `fsync` on Unix), by the hub on a blocking worker; an unreadable file is
  deleted, never moved to `.bak`. Policy, env, demo, no feature and no keys suppress actions and the button but keep
  the file. Freshness (14 days, unparsable or future `confirmed_at` = expired) is evaluated by `is_fresh` on every
  snapshot and at apply, so it also holds with checks off and in a long offline session; apply refuses an expired offer
  ('Check for updates first').
- **Cleanup (UPD-012):** the existing `cleanup()` deletes every entry but one `keep_version` directory, including
  `offer.json`, and `download()` calls it first; both change. Cleanup takes a keep set {offered, applying, downloading},
  removes only `*.part`, stale temp files and other version directories, never `offer.json`, runs at start and after a
  check under the check lock, and the call at the start of `download()` goes away. Replacement order: verify the new
  payload → write the new `offer.json` → delete the old directory.
- **Apply (UPD-011):** `apply_update()` runs on a hub blocking worker under its own `AtomicBool`, never under the check
  lock. It snapshots the *Ready* payload at the click, so a concurrent check may finish but must not replace or delete
  it; the check's result is held and reconciled when the apply ends, and a failed apply is recorded against the applied
  version only, so it can't degrade a newer offer. A second call fails fast with 'already being applied' and the UI
  ignores that error silently. In the *Installed, restart* stage the same call only re-runs the self-test and starts
  the installed file again; it never stages or hashes.
- **Install kind** is probed lazily with `spawn_blocking` at every check and at apply (`State::production` is
  synchronous and runs before the window). A lost eligibility downgrades the offer to *Download*.
- **Windows (UPD-007):** the verified installer is opened `FILE_SHARE_READ`-only, the hash and signature checks read
  through that handle, and it is kept until the app exits (safe Rust, `OpenOptionsExt::share_mode`); per-machine
  installs that can't hold it fall back to *Download*. Spawn returns `Restarting` (a handoff). There is no recovery
  journal: a cancelled installer leaves the old app with the same offer, which is offered again (user-driven).
  `authenticode::signature()` today maps every read failure to 'no signature', which skips the pin; it must map only
  'no certificate table' to that and everything else to invalid (UPD-003, task 7).
- **Linux (UPD-014):** stage in the target's directory, self-test, `rename`, directory `fsync`, start with
  `--update-relaunch=1` and a piped stdin whose write end is moved into a process-lifetime `static` (a destructor that
  dropped it during shutdown would let EOF reach the child before the single-instance socket is gone, and the new
  process would focus the dying one and exit); `try_wait` for 1 s catches an immediate crash. Every failure after the
  `rename` is `InstalledRestartManually`. The environment scrub (the self-test and the relaunch share it) is set on the
  child builder only: remove `APPIMAGE`, `APPDIR`, `ARGV0`, `OWD`; drop every entry under the old `APPDIR` from `PATH`,
  `LD_LIBRARY_PATH`, `XDG_DATA_DIRS`, `PYTHONPATH`, `QT_PLUGIN_PATH`, `GST_PLUGIN_SYSTEM_PATH*`, removing a variable that
  ends up empty. (Edition 2024: `remove_var` is `unsafe`, `unsafe_code` is denied, and a variable would leak into
  terminal shells and `docker` children, which is why the pid isn't passed that way.)
  Descriptors: both children also get every inherited fd from 3 up marked close-on-exec in a `pre_exec` closure
  (`apply/appimage/fds.rs`, the one `unsafe` block of this strategy, `libc` on Linux only; ADR-0007): the runtime's mount-directory
  descriptor would otherwise keep the old mount alive (found by S1's FUSE leg).
- **Relaunch wait (UPD-017, SHL-022):** `main.rs` order becomes panic hook → `wait_for_predecessor` (only with
  `--update-relaunch=1` and a non-terminal stdin; blocks until stdin EOF, a read error or 30 s, on a helper thread with
  `recv_timeout`) → `load_config` → logging → single-instance. The wait comes before the config and `state.json` reads,
  so the old instance's final save is never overwritten by a stale copy. It logs after logging starts. A timeout falls
  through to the normal single-instance rule. The code lives in `dk-hub`; a plain function call needs no
  `nfr-001-allow` marker. [Spec 10 §7](../../spec/10-architecture.md) has the real order.
- **UI:** `UpdateStore` already owns the status task and the manual-check task; it takes the snapshot stream and stale
  results are dropped by `revision` and the existing request id (NFR-005). `AppShell` owns the notification task;
  `mark_update_notified` is atomic and persisted. The status stream replays the current value. The *Updated to X.Y.Z*
  notice stays driven by `last_run_version` (`take_previous_version()`), whatever installed the new version.

### 6.2 UI

```
TitleBar:  Dockering                 [ ▣ Docker · Connected ▾ ]                [⬆] [⟳] [☾] [⚙] [≡]
                                                                                ^ UPD-015: first button of the action group (≡ not on macOS, SHL-021)
StatusBar: ● Connected │ Docker 29.8.1 · API 1.53 · linux/amd64                            ◌ Downloading update… 42%   (transient only)
Toast:     Dockering 0.4.1 is ready to install.                       [ Restart now ]   (body click: release notes)
```

| Offer | Button | Tooltip | Click |
|---|---|---|---|
| none, or *Downloading* | hidden (status bar shows progress) | – | – |
| install, *Ready* | accent | Restart to update (X.Y.Z), 'admin' on an all-users Windows install | `RestartToUpdate` |
| install, *Applying* | disabled, spinner | – | ignored |
| install, *InstalledRestart* | shown | Dockering was updated. Restart to finish | `RestartToUpdate` (only retries the start) |
| *Download* (macOS, `.deb`, portable, read-only, degraded) | shown | Dockering X.Y.Z is available | `ViewReleaseNotes` (release page) |

- **Components:** GPUI Kit `Button` (ghost, small, icon) like its neighbours, no custom element. Icon `Lucide::CircleArrowUp`,
  added to `ExtraIcons` in `crates/dockering/src/assets.rs` (the Lucide set in `gpui-kit-assets` 0.7.0 has it; fallback
  `IconName::ArrowUp`). Accessible name from `Button::accessibility_label` ('Update available: Dockering X.Y.Z';
  present in `gpui-component` 0.7.0, checked in the crate source 2026-10-18); without it an icon-only button has no
  name, which is why the Linux smoke test's AT-SPI helper can't see table icon buttons.
- **Strings:** reuse `upd_ready`, `upd_available`, `upd_restart`, `UPD_RESTART_NOW`; add `UPD_DOWNLOAD`, the accessible name, the tooltips and the messages of §6.3.
- **No platform branch** in the button or Settings; a test checks the same element id, icon and order on every OS.
- **Keyboard (mandatory):** no new action (`RestartToUpdate` and `ViewReleaseNotes` are palette entries, KBD-076). The button
  is in the title bar's Tab order and `F6` region, has no focus ring (KBD-003), dispatches through `ui::dispatch::on_click`
  with `title_focus`, has no chord to show in its tooltip, and focus returns to `title_focus` when it disappears.
- **Settings → Updates (SET-090, UPD-009):** the same section on every OS: switch, current version, last check and result,
  *Check now* (inline progress and error), *View release notes*, and the retained offer with *Restart to update* or
  *Download* and the reason. Checking and errors never replace the offer. Setting off keeps *Check now* and the offer;
  policy hides *Check now* and disables the switch; env, demo, no feature and no keys disable the actions with the real
  reason. The policy note says it is Windows-only.
- **States:** the button has none of its own; it is absent except in the three stages above.

### 6.3 Errors

| Condition | Result | Shown |
|---|---|---|
| Automatic check fails (network, HTTP, bad manifest or signature, hash or size mismatch) | offer unchanged, log line (UPD-004) | nothing |
| Manual check fails | `Error` activity; offer unchanged | status bar and Settings, inline |
| Check succeeds, nothing newer | offer and payload withdrawn | button disappears |
| Not an AppImage / `APPDIR` mismatch / read-only directory / any non-install kind | *Download* offer | button + toast with *Download*; no error |
| Apply while another apply runs | `Err` 'already being applied' | ignored (button disabled) |
| Apply: payload changed or self-test fails | staged file removed, old target untouched, offer degraded to *Download* | notification *Couldn't start the update* with the reason (`UPD_APPLY_FAILED`) |
| Apply: staging fails (disk full, permission) | staged file removed, offer stays *Ready* | notification *Couldn't start the update* + reason |
| Apply: `rename` fails | as above | the same |
| Apply on an expired offer (UPD-017) | refused | 'Check for updates first' |
| Apply: installed (`rename` done) but the directory `fsync` fails, or the new process can't start or dies at once (Linux) | `InstalledRestartManually`; stage *InstalledRestart*; app keeps running | button 'Restart to finish'; notification |
| Windows: a signed running exe rejects an unsigned installer, or the installer is removed or blocked (Defender, Smart App Control) | offer degrades to *Download* | button + Settings reason 'couldn't install automatically' |
| Windows: UAC declined or installer start fails | offer unchanged, app keeps running | *Couldn't start the update* |
| Windows: installer started, then cancelled or fails | next launch of the old version shows the same offer | offered again (user-driven) |
| Relaunch wait times out (30 s) | new process starts as a normal launch (single-instance rule) | log line |

### 6.4 Release flow

**The gate (REL-083).** `verify` reads the repository variable `UPDATER` (`none`|`all`, default `none`) and, for
manual runs, the input `updater` (`inherit`|`none`|`all`, default `inherit`), then computes:

- `updater` = on a branch, the input if it isn't `inherit`, else the variable; **on a `refs/tags/v*` ref the input must be `inherit`** (anything else fails the run: today the `unsigned` input is honoured on tags, and the updater must not be); any other value fails the run.
- `sign_manifest` = `updater == all` **or** the OS signed mode is on.
- `updater == all` ⇒ every target builds with `--features updater`, and the run needs `UPDATE_SIGNING_KEY` and a non-empty
  `PUBLIC_KEYS` in `keys.rs` (fails otherwise; today those checks sit under `unsigned == false`).

| Run | Variable / input | OS mode | `updater` | `sign_manifest` |
|---|---|---|---|---|
| tag (`PUBLIC_RELEASES` unset) | none | unsigned | none | false |
| tag (`PUBLIC_RELEASES` unset) | `UPDATER=all` | unsigned | **all** | **true** |
| tag (`PUBLIC_RELEASES=true`) | none | signed | none | true |
| tag (`PUBLIC_RELEASES=true`) | `UPDATER=all` | signed | all | true |
| dispatch from `main`, `unsigned=true` | input `inherit`, variable `all` | unsigned | all | true |
| dispatch from `main`, `unsigned=true` | input `none` | unsigned | none | false |
| dispatch from `main`, `unsigned=false` | input `all` | signed | all | true |
| dispatch on a `v*` tag | input `inherit` (a different input fails) | as a tag run | as a tag run | as a tag run |
| any | invalid value; or `updater=all` with no key or empty `PUBLIC_KEYS` | – | run fails | – |

(Feature-branch dispatches fail at job start today because `verify` uses `environment: release`; a rehearsal runs from `main`.)

**Evidence and draft gate (REL-084).** The build step scans the *unpackaged* `target/<triple>/release/dockering[.exe]` for the
updater marker (the manifest URL string) and the seam marker and writes `updater-evidence-<target>.txt`; `assemble` fails unless
all six agree with `updater` and none has the seam. `cargo xtask verify-update-artifacts` (called by `verify-release.ps1
-Updater none|all` next to `-Expect Signed|Unsigned`) checks: `.minisig` made by the **first** key (so a secret set to the
rotation key fails), manifest version equals the release version, every platform entry matches its asset's kind, URL, size and
SHA-256, and with `all` a `.minisig` exists and the notes carry the channel's wording. `scripts/verify-release.ps1` has no
cargo or minisign, so the `/release` skill and the signing rehearsal run the xtask command next to it, as they do for
`verify-manifest`; the script only gets `-Updater none|all` for the file set (it forbids a `.minisig` on an unsigned release today)
and the notes wording. The expected file set follows `sign_manifest`. The AppImage's `--version` is already checked by REL-072 and REL-079.

**Files that still assume 'unsigned means no updater' or '`.minisig` means signed'** (task 16 and 17 own them):
`.github/workflows/release.yml` (the `unsigned` input description, feature selection in the build step, the signing and
`verify-manifest` steps, the unsigned notes sentence 'Automatic updates are disabled…'), `scripts/verify-release.ps1` (it
forbids a `.minisig` on an unsigned release and requires that sentence), `scripts/tests/test_release_verify.py`,
`xtask/src/release.rs` (`gen-update-keys` writes the secret key with the default file mode: create it `create_new` with mode `0600` on Unix),
`.agents/skills/release/{SKILL,signing,changelog,checklist-template}.md` (`signing.md` Phase 4 is already corrected), `docs/release.md`, `docs/signing.md`,
README *Privacy & network*, and the smoke harnesses (`scripts/linux-smoke/`, `scripts/macos-smoke/`, which must set `DOCKERING_DISABLE_UPDATES=1`). Documentation that this plan already corrected is listed in the revision log.

**Testing the gate.** `test_release_verify.py` already runs the extracted `verify` step in many configurations: add a case per
table row, reading `GITHUB_OUTPUT`. A static YAML test checks the wiring (`package` and `assemble` read
`needs.verify.outputs.updater` and `sign_manifest`, and `vars.PUBLIC_RELEASES` no longer selects features).

### 6.5 Tests and the Linux end-to-end scenario

| Tier | Where | Proves | Runs |
|---|---|---|---|
| 1. In-process | `dk-update` + `dk-hub` tests, a tokio `TcpListener` release server and a real `reqwest` client through `GithubSource::with_base` | check, download, verify, offer restore/keep/withdraw/expire, apply flag, cleanup rules, staging, self-test, swap, relaunch spawn, every fault, symlinked install, opt-outs | every PR; the AppImage cases on Linux only |
| 2. View | `gpui::TestAppContext` in `dockering` | button stages and retention, once-per-version notification, status-bar trim, Settings, keyboard | every PR, three OSes |
| 3. Process E2E | `scripts/linux-update/run.sh`, `linux-update-e2e.yml` | the real AppImage on a real headless display: S1–S4 below | PR (paths filter), weekly, dispatch; locally through Docker |

**Seam (REL-086)**, Cargo feature `update-e2e` on `dk-update`, `dk-hub` and `dockering`, off everywhere else:
`DOCKERING_UPDATE_E2E_URL` rewrites scheme and host of every fetch (the signed manifest keeps its GitHub URLs, so it is identical to
a production one and the URL pin is still exercised); `DOCKERING_UPDATE_E2E_KEY` replaces the embedded keys;
`DOCKERING_UPDATE_E2E_FIRST_CHECK_SECS`, `…_INTERVAL_SECS` and `…_QUIT_DELAY_SECS` (delays the old app's exit until **after** `run()` returns and its destructors have run, so a pipe closed by shutdown ordering is caught, for S4)
shorten or stretch timing; build-time `DOCKERING_E2E_VERSION` (read only under the feature through `option_env!`) sets the version
of the **old** AppImage to `0.0.1`. The **new** AppImage is built exactly like a release (`updater`, real version), so only one
seam build exists. All eleven production `env!("CARGO_PKG_VERSION")` sites go through one `app_version()` (`main.rs` ×2, `app_shell.rs` ×3,
`settings/updates.rs`, `diagnostics.rs`, `hub.rs` ×2, `logging.rs`, `updates.rs`, which also feeds the `User-Agent`; `build.rs`
and the one test assertion in `dk-hub/src/tests.rs` are not production reads); a unit test scans the
crate sources and fails on any other use; it lives in `xtask`, because `check-blocking` scans whole files under `crates/dockering/src`, tests included. A `#[used]` marker string makes the release scan independent of optimisation.
Tests inject an environment source; none calls `set_var`.

**Fixture and server.** `cargo xtask update-e2e-fixture` runs on the host and only writes files: a throwaway key pair and the
signed `dockering-update.json` (`linux-x86_64`, the new AppImage's real GitHub-style URL and the workspace version). A small
Python static server (`scripts/linux-update/serve.py`) runs **inside** the container, serves the fixture, writes `requests.jsonl`
(method, path, `User-Agent`, time) and has a fault switch. No host loopback server, no host-built binary in the container.

**Layout, like this machine.** Clean Ubuntu 24.04 container (registry mirror, as the smoke test), **unprivileged user** (the smoke
harness runs as root with `HOME=/root`, and root ignores `chmod a-w`, which would make S3 impossible): `~/.local/bin/dockering` is the
old AppImage, `~/.local/share/applications/dockering.desktop` has that `Exec`, and the app starts the way the desktop entry starts it.
S1 runs twice: **extract-and-run** (`APPIMAGE_EXTRACT_AND_RUN=1`) and **FUSE** (`--device /dev/fuse --cap-add SYS_ADMIN
--security-opt apparmor=unconfined`), which proves the `rename` over a really mounted, running image. The display, session-bus and
AT-SPI bring-up (about 40 lines of `scripts/linux-smoke/run.sh`) is *copied* with `HOME`/`XDG_RUNTIME_DIR` parametrised, not
refactored out of the release-gating smoke test. AppImages come from the CI artifact (`gh run download`), a build inside an
Ubuntu container (`--build`), or an Arch image, because a locally built one may need a newer glibc than the container has.
`atspi.py` gains `update-button`, `toast` and `activate`; the app is tracked across the relaunch by pidfile (the smoke
runner follows only the first process); toasts auto-hide, so they are asserted by a log marker plus a best-effort AT-SPI check,
filtered by text.

| # | Scenario | Expected |
|---|---|---|
| S1 | Happy path (extract-and-run leg and FUSE leg) | Before the check: no button, no toast, no request. After it: one manifest, one `.minisig`, one asset request; one toast with *Restart now*; the title-bar button named 'Update available…'. *Restart now* (AT-SPI): the old process exits; `~/.local/bin/dockering` equals the new AppImage (SHA-256) and is executable; the new instance is the primary instance, keeps the old instance's saved window state, shows 'Updated to X.Y.Z' and no button; `User-Agent` is exactly `Dockering/0.0.1 (linux; x86_64)`. |
| S2 | New AppImage fails `--version` (the manifest names a version the binary doesn't have) | *Restart now* gives *Couldn't start the update*; the old file is byte-identical and still runs; the offer is now *Download*. |
| S3 | Read-only install directory | *Download* offer, toast with *Download*, button present, no asset request. |
| S4 | Predecessor exits late (`…_QUIT_DELAY_SECS`) | The relaunched process waits, then becomes the primary instance (not a focus request to the old one); `state.json` holds the old instance's final window bounds. |

Symlinked install, bad signature, bad hash, opt-outs, restart with an announced offer (also offline and with checks off), server
down then up, re-check and failed-check retention, withdrawal, expiry and apply-during-recheck are Tier 1 and 2 tests (§8). A
`selftest` of the runner (like `scripts/linux-smoke/selftest.sh`) injects defects (no swap, no button, a toast twice, a stale
state read) and checks that exactly the intended check fails.

**Workflow.** `linux-update-e2e.yml`: `permissions: contents: read`, no `environment`, no secrets, not a required check
(REL-080); `paths` filter `crates/dk-update/**`, `crates/dk-hub/src/{updates*,single_instance}.rs`, `crates/dockering/src/shell/**`,
`crates/dockering/src/state/updates.rs`, `scripts/linux-update/**`, `xtask/src/**` and itself; plus weekly and `workflow_dispatch`.
Its AppImages are run artifacts, never release assets.

### 6.6 Windows

Spike **W1** (a Windows machine; the interactive UAC parts are the maintainer's manual check, task 18) establishes what this plan
only expects: an installer written by the app's own HTTP client has no Mark-of-the-Web, so SmartScreen doesn't intervene; Defender
and Smart App Control don't block an unsigned app starting an unsigned installer; the UAC
prompt says *Unknown publisher* for an unsigned release; Inno starts while the app holds the file `FILE_SHARE_READ`-locked (the
per-machine fallback is the plan if it doesn't); cancelling the installer leaves the old app and the same offer; the all-users
relaunch goes through Explorer; and which `WinVerifyTrust` results mean 'no signature' rather than 'invalid' (task 7). The CI part is an **update leg in `scripts/installer-smoke.ps1`** on `windows-2025` x64 and ARM64:
install the old build per user (the last published release's installer), run the new installer with exactly `installer_args`, check files, version and relaunch, uninstall.
Windows-gated unit tests cover the share-locked handle, the unsigned and signed trust paths and the argv. None of this is claimed
before it runs on Windows.

### 6.7 Rollout

1. The maintainer runs signing.md Phase 4 (task 0), merges the work, runs a `workflow_dispatch` rehearsal from `main` with `updater=all`
   (unsigned is enough), and only then sets `UPDATER=all`.
2. The first updater release is installed by hand: `curl -fL …/releases/latest/download/Dockering-x86_64.AppImage -o ~/.local/bin/dockering &&
   chmod +x ~/.local/bin/dockering` (Windows: the installer).
3. Every release after it reaches the installed copy by itself: one toast, the button, *Restart now*. A live proof needs that second release.

### 6.8 Deferred: macOS install

Nothing is built. UPD-016 lists what a later plan needs: a Developer-ID, hardened-runtime, notarised `.app` (only the DMG is signed and
stapled today, and no hardened-runtime flag exists anywhere in `scripts/`, `packaging/` or `xtask/`), a whole-bundle exchange (an
`unsafe` FFI module, a dependency and `cargo deny` decision), permissions in `/Applications`, and the Launch Services relaunch.

## 7. Tasks

Sizes: S ≈ half a day, M ≈ one day. Every task has its own verification.

| # | Task | Owner | Size | IDs | Verify |
|---|---|---|---|---|---|
| 0 | **Maintainer (human):** [signing.md Phase 4](../../signing.md): generate the update keys outside the repo, `gh secret set UPDATE_SIGNING_KEY --env release < current.key`, give the agent only the two public keys for `keys.rs`, store `next.key` offline. Later set `UPDATER=all`. | maintainer | – | UPD-003, REL-083 | `cargo xtask verify-manifest` passes on a rehearsal; blocks only task 20 |
| 1 | `app_version()` at all sites plus the scan test; the `update-e2e` seam (`GithubSource::with_base`, key, schedule, quit delay, build-time version, `#[used]` marker, injected env source). **First: it touches many files.** | rust-core | M | UPD-005, UPD-010, REL-086 | `upd_005_e2e_seam_inert_without_env`, `app_version_has_no_direct_uses`, `upd_010_*` |
| 2 | `dk-update/src/offer.rs`: `offer.json` store (atomic: temp file, `fsync`, `rename`, directory `fsync` on Unix only; never `.bak`), `restore`, `is_fresh` (14 days, future or unparsable = expired), `after_check` (keep / refresh / replace / withdraw / non-actionable), the three persisted stages; trim `state.json`; `Paths::updates_dir()` so the hub stops deriving it from `log_dir`. | rust-core | M | UPD-012, UPD-017 | unit tests incl. proptest on the rules |
| 3a | `dk-hub` coordinator, part 1: ungated `UpdateSnapshot`/`UpdateOffer`/`OfferStage`/`OfferMode`/`ApplyOutcome` types and the snapshot stream, apply flag separate from the check lock, result reconciliation after an apply, silent duplicate-apply error. Depends on 2. | rust-core | M | UPD-011, UPD-017 | hub tests on the paused clock with `StaticSource` |
| 3b | `dk-hub` coordinator, part 2: restore at start, `cleanup` with a keep set and no `offer.json` removal (and no call at the start of `download()`), replacement order, degrade-to-*Download* (also for an installer signature the signed running exe rejects, which today only fails the check), lazy install-kind probe, expiry at every snapshot and at apply. Depends on 3a and 4. | rust-core | M | UPD-006, UPD-012, UPD-017 | hub tests on the paused clock |
| 4 | `InstallKind::AppImage`, `detect_appimage` with the magic-byte and writable-directory probe; macOS stays `Other`. | rust-core | S | UPD-006, UPD-013 | `upd_013_*` on every OS |
| 5 | AppImage apply: stage, `ETXTBSY` retry, self-test, `rename` + directory `fsync`, scrubbed-environment relaunch with the static stdin pipe and the 1 s `try_wait`, the *Installed, restart* retry (self-test and start only), staging sweep by age; `apply.rs` becomes a directory; `cfg(target_os = "linux")` with an `Unsupported` stub; a writer seam for disk-full tests. **Before 7** (it moves `apply.rs`). | rust-core | M | UPD-003, UPD-012, UPD-014 | unit tests on temp dirs with shell-script 'AppImages' |
| 6 | `wait_for_predecessor(reader, timeout)` and its call in `main.rs` before `load_config`; log after init; the real order in spec 10 §7. | rust-core | S | UPD-017, SHL-022 | unit tests: EOF, read error, timeout, terminal stdin, no flag, unknown flag value |
| 7 | Windows: share-locked installer handle in the verify-and-spawn path (checks read through it), per-machine fallback, unsigned and signed trust paths, and `authenticode::signature()` mapping only 'no certificate table' to *no signature* (every other failure is invalid). Depends on 5. | windows-platform | M | UPD-003, UPD-007 | Windows-gated tests on the CI Windows runner |
| 8 | Windows: spike W1 report and the `installer-smoke.ps1` update leg. The CI script is release-engineer plumbing; the interactive UAC checks are a human's (task 18). | windows-platform, release-engineer | M | REL-087 | CI leg green on x64 and ARM64; report in the release checklist |
| 9 | Title-bar button from the offer stage, notification once per version for *Ready* and *Download*, tooltips, disabled-while-applying, focus fallback, `ExtraIcons`, accessibility label, strings; the status bar keeps only the transient items. Depends on 3a. | gpui-ui | M | UPD-008, UPD-015, KBD-076 | view tests (§8), screenshots light and dark on a Linux window, `check-blocking` clean |
| 10 | Settings → Updates: offer row with *Restart to update* / *Download* and the reason; checks never erase it; Windows-only policy note. | gpui-ui | S | UPD-009, SET-090 | view tests |
| 11 | Tier 1 tests: the in-process release server and the scenario list of §8, including a macOS-style *Download* offer through a platform-key override (so it runs on all three CI OSes). | qa-engineer | M | UPD-001…004, UPD-006, UPD-011…014, UPD-017 | pass under `cargo nextest -p dk-hub -p dk-update --features dk-hub/updater,test-support` (the form `ci.yml` uses) on Linux; macOS and Windows compile and skip the AppImage cases |
| 12 | `xtask update-e2e-fixture`, `scripts/linux-update/serve.py`, the AppImage build script (CI artifact, `--build`, or Arch image); **and the smoke harnesses set `DOCKERING_DISABLE_UPDATES=1`** (`scripts/linux-smoke/`, `scripts/macos-smoke/`). | release-engineer | M | REL-085 | both AppImages built, `--version` checked; the smoke self-tests (30 controls) still green |
| 13 | `scripts/linux-update/run.sh`, `atspi.py` subcommands, S1 (both legs), the runner self-test. Depends on 5, 6, 9 and 12. | qa-engineer | M | REL-085, UPD-013…015 | green through Docker here; each self-test control fails exactly its check |
| 14 | S2–S4 and `scripts/tests/test_linux_update.py` (command line, validation, fake `docker`). | qa-engineer | S | REL-085, UPD-014, UPD-017 | green through Docker; the python tests pass |
| 15 | `linux-update-e2e.yml`. | release-engineer | S | REL-085 | actionlint; a green pull-request run with artifacts |
| 16 | Release workflow: `verify` outputs and validation, `package`/`assemble` read them, evidence files and marker scan, signing steps follow `sign_manifest`, notes wording, input description; `test_release_verify.py` rows and the static YAML test. | release-engineer | M | REL-016, REL-031, REL-032, REL-083, REL-084, REL-086 | `test_rel_083_*`, `test_rel_016_*`, `test_rel_031_*`, `test_rel_086_*`; a `main` rehearsal builds six targets with the feature |
| 17 | `xtask verify-update-artifacts`, `gen-update-keys` writing the secret key `create_new` with mode `0600` on Unix, `verify-release.ps1 -Updater` (file set and notes only), `/release` skill files including `signing.md`, `docs/release.md`, `docs/signing.md`, README privacy text (also the background asset download on installs that can update themselves). | release-engineer | M | REL-012, REL-017, REL-060, REL-084 | `rel_084_*`; `verify-release.ps1` against a fake bundle |
| 18 | Release-checklist rows and manual checks: this machine (Hyprland, real FUSE, launched from the `.desktop` entry, a symlinked install), a Windows machine (UAC, unsigned; **a human executes the interactive steps, the maintainer**), macOS shows *Download*. | qa-engineer, maintainer | M | UPD-013…015, REL-087 | checklist updated with results and screenshots |
| 19 | Review. | reviewer | M | all | verdict: approve |
| 20 | **First updater release** and the live proof on a second one. | maintainer, with `/release` | – | REL-083, REL-084 | the draft gate is green; the installed copy updates itself on the next release |
| 21 | `/feature-planning complete linux-appimage-updates`. | architect | S | all | spec reconciled, plan `done` |

Order: 1 first (it touches many files); 2 and 4 next, then 3a → 3b → 9, 10; 5 after 4 and before 7; 6 anywhere after 1; 7 → 8; 11 after 3b and 5; 12 after 1;
13 after 5, 6, 9, 12; 14 after 13; 15 after 14; 16 → 17 in parallel with 5…15; 18 after 13 and 8; 19 after everything but 20; 20 after 0, 17 and 19.

## 8. Test plan

| ID | Layer | Test names |
|---|---|---|
| UPD-003 | unit (`dk-update`, Windows-gated) | `upd_003_unsigned_exe_accepts_manifest_trust`, `upd_003_signed_exe_rejects_unsigned_installer`, `upd_003_unparsable_signature_on_running_exe_fails_closed`, `upd_003_appimage_staged_hash_checked`, `upd_003_appimage_self_test_is_required` |
| UPD-005 | unit, script | `upd_005_e2e_seam_inert_without_env`, `upd_005_no_keys_fails_closed_before_network`, `test_upd_005_feature_follows_updater_output` |
| UPD-006 | unit, Tier 1 | `upd_006_linux_appimage_downloads_and_is_ready`, `upd_006_deb_and_macos_get_download_offer`, `upd_006_lost_eligibility_downgrades_to_download` |
| UPD-007 | unit (Windows), Tier 1 | `upd_007_installer_held_share_locked_until_exit`, `upd_007_per_machine_falls_back_to_download_without_lock`, `upd_007_installer_args`, `upd_007_cancelled_installer_leaves_offer` |
| UPD-008 | view | `upd_008_status_bar_keeps_only_transient_items` (replaces `upd_008_status_bar_renders_each_state`), `upd_008_notification_once_per_version_for_ready_and_download`, `upd_008_restored_offer_shows_button_without_second_toast`, `upd_008_download_click_changes_no_state`, `upd_008_updated_notice_follows_last_run_version` |
| UPD-009, SET-090 | view | `set_090_offer_row_and_reason`, `set_090_checking_does_not_erase_offer`, `set_090_setting_off_keeps_check_now_and_offer`, `set_090_policy_note_is_windows_only` |
| UPD-010 | unit | `upd_010_user_agent_exact`, `upd_010_seam_rewrites_host_but_keeps_url_pin` |
| UPD-011 | hub, Tier 1 | `upd_011_apply_outcome_restarting`, `upd_011_apply_outcome_installed_restart_manually`, `upd_011_second_apply_fails_fast`, `upd_011_apply_during_recheck`, `upd_011_apply_failure_does_not_degrade_newer_offer`, `upd_011_check_result_during_apply_is_reconciled_after`, `upd_011_stale_snapshot_dropped` |
| UPD-012 | unit, hub | `upd_012_offer_written_atomically_and_never_bak`, `upd_012_unreadable_offer_is_deleted`, `upd_012_old_payload_kept_until_new_verified`, `upd_012_cleanup_keeps_offer_json_and_keep_set`, `upd_012_staging_sweep_by_age` |
| UPD-013 | unit | `upd_013_appimage_detected_from_env`, `upd_013_requires_appdir_and_exe_inside`, `upd_013_rejects_a_file_that_is_not_an_appimage`, `upd_013_read_only_dir_is_download`, `upd_013_non_linux_is_other`, `upd_013_asset_kind_must_be_appimage`, `upd_013_symlinked_install_resolves_to_real_file` |
| UPD-014 | unit, Tier 1 | `upd_014_swap_replaces_target_atomically`, `upd_014_failed_self_test_keeps_old_and_degrades_offer`, `upd_014_wrong_version_rejected`, `upd_014_self_test_timeout`, `upd_014_etxtbsy_retried`, `upd_014_hash_mismatch_while_staging`, `upd_014_disk_full_keeps_old_and_offer_ready`, `upd_014_mode_preserved`, `upd_014_self_test_and_relaunch_share_scrubbed_env`, `upd_014_relaunch_failure_is_installed_restart`, `upd_014_installed_restart_retry_starts_without_staging`, `upd_014_fsync_failure_after_rename_is_installed_restart`, `upd_014_immediate_child_death_is_installed_restart`, `upd_014_stdin_pipe_outlives_function_scope`, `upd_014_version_output_format_frozen`, `upd_014_scrub_removes_emptied_variables` |
| UPD-015 | view | `upd_015_button_follows_offer_stage`, `upd_015_button_survives_recheck_and_failed_check`, `upd_015_button_survives_checks_off_and_offline_start`, `upd_015_ready_click_restarts`, `upd_015_applying_is_disabled`, `upd_015_installed_restart_click_retries_start`, `upd_015_download_click_opens_release_page`, `upd_015_same_first_button_on_every_os`, `upd_015_accessible_name`, `upd_015_focus_returns_to_title_bar` |
| UPD-016 | – | deferred; no test |
| UPD-017 | unit, hub, Tier 1 | `upd_017_restore_verifies_signature_and_version`, `upd_017_expiry_applies_when_checks_are_off`, `upd_017_restore_expires_after_14_days`, `upd_017_future_confirmed_at_expires`, `upd_017_apply_refuses_expired_offer`, `upd_017_failed_check_keeps_offer`, `upd_017_withdrawn_release_clears_offer_and_payload`, `upd_017_older_manifest_after_pull_replaces_offer`, `upd_017_identical_entry_keeps_stage_and_failed_flag`, `upd_017_newer_manifest_replaces_after_payload_verified`, `upd_017_catching_up_deletes_offer`, `upd_017_failed_install_not_retried_until_newer`, `upd_017_signed_exe_rejecting_installer_degrades_to_download`, `upd_017_applying_stage_not_persisted`, `upd_017_hard_disable_hides_button_keeps_file`, `upd_008_download_click_changes_no_state` (view), `upd_017_wait_returns_on_stdin_eof`, `upd_017_wait_returns_on_read_error`, `upd_017_wait_times_out_and_continues`, `upd_017_wait_ignores_terminal_stdin_and_unknown_flag` |
| KBD-076 | view | `kbd_076_update_actions_in_palette` (kept), `kbd_076_title_update_button_in_tab_order_and_f6` |
| SHL-022 | hub (Unix), xtask | `shl_022_wait_runs_before_config_load` (xtask: the order scan of `main()`), `shl_022_relaunch_never_sends_focus_while_waiting` |
| REL-012, REL-017, REL-031, REL-032 | script, xtask | `test_rel_017_minisig_follows_sign_manifest`, `test_rel_031_unsigned_updater_all_needs_no_signpath`, `test_rel_031_signed_updater_none_still_needs_update_key`, `test_rel_032_manifest_checked_whenever_sign_manifest`, `rel_012_*` (existing, extended) |
| REL-016, REL-083 | script (`test_release_verify.py`, `verify` step outputs; static YAML test) | `test_rel_016_unsigned_default_unchanged`, `test_rel_083_truth_table` (every row of §6.4), `test_rel_083_needs_key_and_public_key`, `test_rel_083_rejects_unknown_value`, `test_rel_083_tag_requires_inherit`, `test_rel_083_package_reads_verify_outputs` |
| REL-060, REL-084 | xtask, PowerShell | `rel_084_manifest_signed_by_current_key`, `rel_084_rotation_key_signature_fails`, `rel_084_entries_match_assets`, `rel_084_manifest_version_equals_release`, `rel_084_evidence_agrees_with_updater_output`, `verify-release.ps1 -Updater all` against a fake bundle |
| REL-085 | Tier 3 | S1–S4 (S1 in both legs); `scripts/tests/test_linux_update.py`; the runner self-test controls |
| REL-086 | script | `test_rel_086_release_binary_has_no_seam`, `test_rel_086_release_workflow_lists_no_seam_feature`, `app_version_has_no_direct_uses` |
| REL-087 | CI (Windows) + manual | the `installer-smoke.ps1` update leg (x64, ARM64); W1 report; Linux S1–S4 |
| UPD-001…004 | Tier 1 | `upd_001_wrong_host_and_url_rejected_over_http`, `upd_002_unknown_schema_over_http`, `upd_003_bad_signature_over_http`, `upd_004_no_downgrade_over_http`, `upd_004_opt_outs_make_no_requests` |

## 9. Risks & spikes

| Risk | Mitigation / spike |
|---|---|
| ⚠ verify (W1): an unsigned installer from the app has no Mark-of-the-Web; the UAC prompt says *Unknown publisher*; Inno starts while the file is share-locked; cancellation leaves the old app; the Explorer relaunch works. | Spike W1 and the `installer-smoke.ps1` update leg. If the locked start fails: per-user retries after one more hash check, per-machine falls back to *Download* (UPD-007). |
| ⚠ verify (partly observed): replacing the file under a *running FUSE-mounted* AppImage keeps the old process healthy; `AppRun` and the runtime env facts of UPD-013 hold on other runtimes. | **Observed 2026-10-19** with the v0.3.0 AppImage (runtime `8f39b89`, one machine, the runtime's `--appimage-mount`, not a GUI run): `rename` of another AppImage over it left the mounted copy serving `dockering 0.3.0`, while the path then held the new file and printed `dockering 0.2.0`; the mount unmounted cleanly. S1's FUSE leg re-proves it with the real app on every CI image; task 18 repeats it on this machine. |
| ⚠ verify (partly observed): the stdin-EOF handshake. | **Observed 2026-10-19:** a child started in its own session with a piped stdin saw EOF when the parent ended with `os._exit` (no destructors), after about the parent's lifetime; stdin on `/dev/null` reads EOF at once, which is why the wait is gated on `--update-relaunch=1`; the app's argument parser ignores unknown arguments (`main.rs`). The Rust static-pipe and shutdown-order behaviour is proven by `upd_014_stdin_pipe_outlives_function_scope` and S4. |
| The relaunched process reads `config.toml` or `state.json` while the old one writes them. | The wait comes first in `main()` (S4 checks the saved window bounds). |
| A stray `--update-relaunch=1` or an unknown flag value hangs a launch. | Honoured only when stdin isn't a terminal, bounded at 30 s, unknown values ignored (UPD-017); frozen from the first updater release. |
| The scrub misses a variable and the new instance can't find its libraries. | S1 on the real AppImage; the new `AppRun` re-prepends its own paths. |
| A broken new AppImage strands the user. | Self-test before the swap, offer degraded to *Download* (S2). |
| `APPIMAGE` aimed at an arbitrary writable file. | ELF magic plus the type 2 marker, exe under `APPDIR`, and the signed hash first. |
| A crash during staging leaves a 19 MB hidden file in `~/.local/bin`. | The startup sweep removes `.dockering-update-*` files in the install directory that are more than an hour old (UPD-012). |
| A withdrawn or yanked release stays installable. | UPD-017: a successful check withdraws it; offline copies expire after 14 days. |
| Lost or mis-set update keys. | `keys/README.md` rotation; REL-084 requires the *current* key. Key compromise has no revocation: ship a new release with rotated keys (ADR-0007). |
| The test seam leaks into a release. | Explicit feature list, marker scan of the unpackaged binary, inert-without-variables test; the E2E workflow has no secrets and uploads no release asset. |
| ⚠ spike: the second AppImage build is slow in CI. | Only the old AppImage uses the seam feature; both reuse the cargo cache; measure in task 12. |
| *Restart now* drops running terminal sessions and image pulls. | Known gap, unchanged. |
| macOS users get no in-app install. | Deliberate (UPD-016 deferred); they get the same notification and button and a *Download*. |
| The install smoke tests (REL-075, REL-078) would check GitHub 30 s after launch once `UPDATER=all` ships. | They set `DOCKERING_DISABLE_UPDATES=1` (task 12); the end-to-end test is the only updater scenario. |
| A failing install-mode download keeps the button hidden until the next successful check (up to 24 h). | Accepted: nothing is installable yet and the log has the reason; a *Download* fallback after repeated failures is a possible follow-up. |
| A rehearsal signs a manifest for the next version with the production key. | The review bundle's artifact retention is kept short; the manifest names only assets of that run. |
| A signed running Windows exe meets an unsigned installer (an emergency unsigned release). | Fails closed: the offer degrades to *Download* with the reason (UPD-003, UPD-017). |

## 10. Revision log

- 2026-10-18: created as a Linux-only plan; spec edited; reviewed by an architect agent and revised.
- 2026-10-18: broadened to all platforms (a subagent run); added UPD-016/017 and REL-087 and a macOS install strategy.
- 2026-10-19: **simplified and the review's blockers resolved.** macOS in-app install is deferred (UPD-016); OS signing is not needed (trust is manifest + SHA-256; an unsigned Windows release installs, a signed exe still pins). Retained offer mirrors the latest authenticated manifest: pulled releases are withdrawn, offline copies expire after 14 days, the offer lives in `<data-local>/updates/offer.json` (atomic, fsynced, never `.bak`) instead of the roaming `state.json`; the transaction journal, PID+birth-identity+nonce handshake and the kept previous AppImage were cut. Apply has its own flag instead of the check lock, a newer download never deletes the offered payload before it verifies, and the button derives from the offer (new *Installed, restart* stage, failed installs degrade to *Download*). Relaunch waits on a piped stdin EOF (30 s) in `main()` before any state read. The Windows elevated installer is held share-locked from check to run. Gate: `UPDATER=none|all` plus a dispatch input `inherit|none|all` and `sign_manifest`, with a truth table and a testable `verify` step; REL-084 gets per-target evidence from the unpackaged binary; REL-085 shrinks to scenarios S1–S4 with one seam build; REL-087 is Windows and Linux only. Docs corrected: REL-012/016/017/031/032/060, README, signing.md (key custody, unsigned manifest wording), release.md, spec 10 §7, the release checklist; ADR-0007 rewritten with a threat model. Open: the 'Files that still assume…' list of §6.4 is implementation work (tasks 16 and 17).
- 2026-10-19 (second pass): a re-review (read-only, on `claude-sonnet-5.5`) found no blockers in the design and two text gaps, both fixed, plus should-fixes applied. Key custody: the release skill's `signing.md` Phase 4 had the agent run `gen-update-keys`; now the maintainer does. Expiry: the 14-day rule is evaluated whenever the offer is read, also with checks off and in a long offline session, a future or unparsable `confirmed_at` is expired, apply refuses an expired offer. Also: cleanup would have deleted `offer.json` (keep set, no cleanup call in `download()`, replacement order); a manifest lower than the offer (a pulled release) makes the old offer non-actionable at once, and 'differs' is defined; a check result during an apply is held and a failed apply degrades only the applied version; only three stages are persisted; the stdin write end lives in a process-lifetime `static` (the early-EOF failure that would make the new process focus the dying one), plus a read error counts as EOF, an injected reader and a 1 s `try_wait`; every failure after `rename` is `InstalledRestartManually`; the frozen contract now includes the `--version` output, platform keys and asset names; `authenticode::signature()` must only treat 'no certificate table' as unsigned; native claims (*Unknown publisher*, Defender/Smart App Control) marked as spike W1; dispatch on a tag must use `inherit`; REL-084 tooling split (xtask for minisign, the script for file set and notes); the install smoke tests set `DOCKERING_DISABLE_UPDATES=1`; the *Updated to* notice stays driven by `last_run_version`; UI DTO types are ungated in `dk-hub`; staging sweep by age; directory `fsync` on Unix only; the scan test lives in `xtask`; task 3 split into 3a/3b, task 5 before 7, new dependencies, owners clarified.
- 2026-10-19 (finish): main checked (`477f54b`, no code or workflow change since the plan was written); the script tests that read the repo (`test_ci_gate`, `test_release_verify`, `test_linux_smoke`: 7, 13 and 79 tests) pass with the doc edits. Two risks turned into observed facts, with their scope stated (§9): `rename` over a live FUSE-mounted v0.3.0 AppImage (the old mount keeps serving the old version, the path holds the new, self-tested file) and the stdin-EOF handshake (EOF on parent exit without destructors; `/dev/null` reads EOF at once, so the wait stays gated on the flag; unknown arguments are ignored by `main.rs`). One design gap closed: in the *Installed, restart* stage `RestartToUpdate` only re-runs the self-test and starts the installed file (UPD-014, task 5, new test). The plan is complete; status stays `draft` until the maintainer approves it.
