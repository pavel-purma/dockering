# Plan: Start and set up engines on Windows

- **Slug:** `windows-engine-start`
- **Status:** in-progress (approved by the user 2026-10-21)
- **Spec:** [Start and set up engines (Windows)](../../spec/features/windows-engine-start.md) (ENG-142…164, REL-088/089 new) · [Engines](../../spec/features/engines.md) (ENG-111 changed, ENG-137 generalised) · [Backends §2/§2.1/§4.2](../../spec/20-engine-backends.md) · [Contract §8](../../spec/21-engine-api-contract.md) · [Product non-goals](../../spec/00-product.md) · [NFR-022/023](../../spec/40-non-functional.md) · [Keyboard KBD-075](../../spec/features/keyboard.md)
- **Milestone:** post-v1 (this checkout has no milestone table)
- **Requirement IDs:** ENG-142…164, REL-088, REL-089 (new) · ENG-111, ENG-137, NFR-022, NFR-023, KBD-075, `00-product` non-goal (changed) · ENG-030, ENG-106, ENG-113, ENG-139, ENG-141, KBD-002/003, NFR-001…005 (preserved)
- **Created:** 2026-10-21

## 1. Goal

On Windows, "Docker isn't running" should be one click from fixed. Three cases:

1. **Docker Desktop installed but stopped.** Today its pipe is absent, so discovery omits it and the user sees the generic first-run page.
2. **Docker installed in a WSL distro but `dockerd` not running.** Today the distro is classified `no-docker` and hidden, or fails with a hint.
3. **No engine at all.** The user can ask Dockering to create one dedicated Docker-in-WSL distro, without Docker Desktop and without administrator rights.

The mechanism extends what already exists (`StartPrompt`, `EngineFactory::start`, `HubHandle::start_engine`, the SHL-013 start page, built for WSL distros and WSLC), so most of the UI is reused.

## 2. Scope

**In:** Docker Desktop start (CLI first, `Docker Desktop.exe` fallback); `dockerd` start in a distro that has Docker; a generic `stopped_after` hook; the dedicated-distro setup (consent, download, SHA-256 check, import, start, register, cancel); first-run / no-engine page lists; palette entries; the root-filesystem build and publish pipeline; tests.

**Out:** installing Docker Desktop; installing or updating WSL, enabling Windows features, elevation (`wsl --install` needs administrator rights and a reboot, so it stays a hint, ENG-156); stopping or restarting engines; in-app removal of `dockering-engine` (deferred, manual `wsl --unregister`); VM resource limits; macOS/Linux behaviour (Colima/OrbStack/systemd starts are a possible follow-up); WSLC (its start already exists, ENG-138); Windows containers.

Product non-goal changed (recorded in `00-product.md`): the "no engine installation" line now has a narrow Windows exception for start and the single dedicated distro.

## 3. Assumptions & open questions

| # | Assumption / question | Default if unanswered |
|---|---|---|
| A1 | **Decided by the user (2026-10-21):** full scope now, including provisioning, in one plan; the setup distro is created with `wsl --import` of a Dockering-built root filesystem | Yes |
| A2 | **Changed 2026-10-21 (user request):** the user picks the image in the consent dialog. **Alpine** = Alpine + `docker`, `containerd`, `runc`, `socat`, `dockerd` started from a `wsl.conf` `[boot] command` (no systemd; smallest, ~50–70 MB compressed, ⚠ measure, W-3). **Ubuntu** = Canonical's Ubuntu LTS WSL root filesystem + `docker-ce` from Docker's apt repo at pinned versions, `systemd=true` in `wsl.conf` and `docker.service` enabled (familiar, larger, ⚠ measure, W-3). Both are built in CI (REL-088) | Both offered |
| A12 | **Decided by the user (2026-10-21, revision 2): there is no default image.** The dialog opens with nothing selected, shows no *Recommended* badge, *Set up* stays disabled until the user picks, and no choice is remembered. The choice is offered only while no Dockering-created distro exists, so one distro name (`dockering-engine`) serves both images | Yes |
| A13 | If an image has no published asset for the CPU architecture (for example Ubuntu on ARM64 before it is built), that choice is omitted; a single remaining choice is still shown and still has to be picked; with none the offer is hidden (replaces A11) | Yes |
| A3 | Hosting: GitHub Release assets under tag `rootfs-<n>` in this repo, separate from app releases, so the image can be patched without an app release. The app pins URL + size + SHA-256 in `dk-wsl` | Yes |
| A4 | Download goes through `reqwest` (already a workspace dep, used by `dk-update`). To avoid a second TLS/HTTP stack in `dk-wsl`, the download/verify code sits in a small new internal crate `dk-fetch` shared with `dk-update`, or `dk-wsl` takes `reqwest` directly (decide at task 1; `dk-core` stays free of it, ADR-0001) | `reqwest` in `dk-wsl` behind `cfg(windows)` |
| A5 | Distro name `dockering-engine`, folder `%LOCALAPPDATA%\Dockering\wsl\dockering-engine`, ≥ 2 GiB free required, WSL 2 only | Yes |
| A6 | No administrator rights are ever requested. If WSL is absent or too old the flow stops with the hint `wsl --update` / "install WSL from Microsoft Store" | Yes |
| A7 | Consent is a `Dialog` shown every time (no "don't ask again"), because a download and a distro are created | Yes |
| A8 | Docker Desktop start timeout 120 s, distro dockerd wait 30 s, setup wait 60 s (from your note; tune in spikes) | Yes |
| A9 | `docker desktop start` can wait for the user (licence, sign-in). That is reported as a timeout with a hint, not as a failure of Dockering | Yes |
| A10 | One more engine, `docker-desktop-linux` (second pipe), is not a separate start target: starting Docker Desktop brings both pipes up | Yes |

## 4. Requirements (as written into the spec)

See [windows-engine-start.md](../../spec/features/windows-engine-start.md). Summary:

| ID | Requirement | New/Changed |
|---|---|---|
| ENG-142 | Windows-only, factory-driven, no UI branching on kind | New |
| ENG-143 | Installed-but-stopped Docker Desktop is listed `Stopped` with a start prompt (registry, no spawn) | New |
| ENG-144 | Start Docker Desktop: `docker desktop start --timeout 120`, `Docker Desktop.exe` fallback, explicit action only | New |
| ENG-145 | Status via `docker desktop status`, only after a failure or in the start action; Resource Saver is not stopped | New |
| ENG-146 | `stopped_after` hook: failed connect → `Stopped` when the backend says so; ENG-137 becomes the WSLC implementation | New |
| ENG-147 | Start progress (`Connecting`), result, hint for user-gated Docker Desktop | New |
| ENG-148 | Probe exit code 14 `installed-not-running`: listed `Stopped`, not hidden | New |
| ENG-149 | Start dockerd in a distro as root via fixed argv; never edits the distro | New |
| ENG-150 | `SetupOffer`s from the factory, shown on first-run, palette and Settings | New |
| ENG-151 | Consent dialog; nothing happens before *Set up* | New |
| ENG-152 | Pinned, SHA-256-verified Docker rootfs per image choice and architecture | New |
| ENG-153 | Provisioning steps 1–7, no admin rights | New |
| ENG-154 | Progress stream, cancel, cleanup of only what this run created | New |
| ENG-155 | Never touch a distro Dockering didn't create in this run (one name serves both images) | New |
| ENG-156 | Preconditions are guidance; never install/update WSL, never elevate | New |
| ENG-157 | Nothing else is installed | New |
| ENG-158 | First-run and no-active-engine pages list startable engines and setup offers | New |
| ENG-159 | Keyboard: palette entries, focus rules | New |
| ENG-160 | Security and privacy (argv, registry paths, one HTTPS GET) | New |
| ENG-161 | Wording: no mechanism words in chrome (image labels come from factory data) | New |
| ENG-162 | **Image choice (Ubuntu or Alpine):** `SetupChoice` data from the factory, an unselected radio group in the consent dialog, *Set up* disabled until the user picks, no default and nothing remembered, a choice omitted when it has no asset | New (revision 1, 2026-10-21; no default since revision 2) |
| REL-088 | Rootfs build for Alpine and Ubuntu × two architectures, smoke test per cell, attestation, licences | New |
| REL-089 | CI pin check of the constants against the published asset | New |
| ENG-111, ENG-137, NFR-022, NFR-023, KBD-075, product non-goal | Extended | Changed |

## 5. Engine contract impact

```rust
// dk-core  (no Engine trait change, no new Capability: connection-level, like ENG-139)
pub struct SetupOffer { pub id: String, pub title: String, pub body: String, pub action_label: String, pub choices: Vec<SetupChoice> }
pub struct SetupChoice { pub id: String, pub label: String, pub description: String, pub download_bytes: u64, pub disk_bytes: u64 }  // no default: the user always chooses
pub enum SetupPhase { Checking, Downloading, Verifying, Importing, Starting }
pub enum SetupProgress { Phase { phase: SetupPhase, fraction: Option<f32>, detail: String }, Done(EngineConfig) }

trait EngineFactory {
    async fn stopped_after(&self, cfg: &EngineConfig, error: &EngineError) -> bool { false }  // ENG-146
    async fn setup_offers(&self) -> Vec<SetupOffer> { vec![] }                                 // ENG-150
    fn setup(&self, offer_id: &str, choice_id: &str) -> EngineStream<SetupProgress> { /* unsupported */ } // ENG-154/162
}
// HubHandle: setup_offers() -> HubCall<Vec<SetupOffer>> ; run_setup(offer_id, choice_id) -> HubStream<Feed<SetupProgress>> ; cancel by dropping the stream.
// start_prompt / start / start_engine already exist.
```

| Op | Docker (`DockerFactory`, Windows) | WSL distro (`WslDistroFactory`) | WSLC | Capability |
|---|---|---|---|---|
| Discover stopped | registry install path + exe exists, pipe absent → `Stopped` (ENG-143) | probe exit 14 → `Stopped` (ENG-148) | unchanged | none |
| `start_prompt` | *Docker Desktop isn't running* / *Start Docker Desktop* | *Docker isn't running in <distro>* / *Start Docker* | unchanged | none |
| `start` | `docker.exe desktop start --timeout 120`, else `Docker Desktop.exe` + pipe poll (ENG-144) | `wsl.exe -d <d> -u root --exec systemctl\|service … docker start`, wait for socket (ENG-149) | unchanged | none |
| `stopped_after` | connect failed and `docker desktop status` is not running / resource saver (ENG-146) | probe says installed-not-running | existing ENG-137 logic | none |
| `setup_offers` / `setup` | none | *Set up Docker in WSL* with the choices *Ubuntu* / *Alpine* (from `SetupChoice` data): download, verify, import, start, ping (ENG-150…154, 162) | none | none |

Non-Windows builds compile the defaults only: no start prompt, no offers.

## 6. Design

### 6.1 Data flow & threading

- **Discovery** stays one scan per start / *Rescan* (ENG-113). Docker Desktop's registry read happens inside the existing `discover()` future on the hub runtime (`tokio::task::spawn_blocking` for the registry call, as `dk-wsl` already does for Lxss). No process is spawned. A Docker Desktop that is Stopped at startup is re-pinged by the existing 30 s probe loop (pipe ping, ENG-020); when the pipe appears it moves to `Disconnected` → connect, so starting Docker Desktop from the Start menu also heals the page.
- **`stopped_after`** is called by the supervisor and `autoselect` where `is_startable_stop` is called today (`supervisor.rs:64–77, 196`); the check becomes async and covers `NotFound{Session}` (WSLC) through the same trait method. It may run `docker desktop status` (bounded 5 s, argv, `CREATE_NO_WINDOW`), but only after a failed connect, never from the probe.
- **Start** reuses `HubHandle::start_engine` (`hub.rs:799`): `Connecting` → `factory.start` on the hub runtime → `Disconnected` and wake the supervisor, or `Failed`. The Docker Desktop child runs with a 125 s outer timeout and `kill_on_drop`. The UI keeps the `HubCall` task on the shell (`self.starting` / `hub_call`, as today).
- **Setup** runs on the hub runtime as a stream (`Feed<SetupProgress>`). The shell stores the stream task and a `setup_id`; events with another id are dropped (stale guard). Dropping the task cancels the future; the setup future owns a cleanup guard that deletes the cache file and, if the import ran in this setup, runs `wsl --unregister dockering-engine` (ENG-154/155), on a spawned cleanup task so teardown never blocks the runtime. Downloads stream to disk (`tokio::fs`, hashed with `sha2` while streaming), never to memory. Nothing runs on the UI thread (NFR-001…005); the CI grep stays green because all new code is in `dk-wsl`/`dk-engine-docker`/`dk-hub`.
- **After `Done(cfg)`** the hub registers the engine (`add_engine`, origin `Discovered`-like but stored so it survives restarts via ENG-115) and activates it, as the spec's ENG-153 step 7. A `Rescan` later finds it through normal distro discovery (the distro now exists and runs Docker).

### 6.2 UI

Reuses `engine_views::start_page` and `first_run`. Additions in `crates/dockering`:

```
 first-run / no active engine
 ┌───────────────────────────────────────────────┐
 │  No engine is running                         │
 │  Available to start                            │
 │   Docker Desktop            [Start Docker Desktop] │
 │   Ubuntu-22.04              [Start Docker]         │
 │  Not installed                                │
 │   Docker in WSL, no Docker Desktop needed     │
 │                      [Set up Docker in WSL…]  │
 │  [Rescan]  [Add engine…]                      │
 └───────────────────────────────────────────────┘

 Set up Docker in WSL (dialog)
 ┌───────────────────────────────────────────────┐
 │  Choose an image                              │
 │   ( ) Ubuntu   Familiar Ubuntu LTS            │
 │                Larger download                │
 │   ( ) Alpine   Smallest and fastest to set up │
 │                                               │
 │  Choose an image to continue.                 │
 │                    [Cancel]  [Set up] (off)   │
 └───────────────────────────────────────────────┘
 After a pick, the sizes, the folder and the
 "no admin rights, other distros untouched" text
 appear and [Set up] turns on. Nothing is preselected.
```

- States: *Stopped* (button), *Starting…* (disabled + spinner), *Failed* (existing error page with the child message and the hint, *Retry* and *Start*), data. Setup dialog: image choice and consent (a radio group from `SetupChoice` data with **nothing selected**; each row shows its description; *Set up* is disabled with the hint *Choose an image to continue* until the user picks; after a pick, the sizes and the consent text follow the selection) → progress (phase text, `Progress` bar, *Cancel*) → success (activates the engine and a success toast) or error (message + hint + *Try again*). Sizes are measured in spike W-3.
- Keyboard (ENG-159): `StartEngine` and a new `SetUpEngine` action are palette entries without chords; the first-run primary action takes initial focus; the dialog's initial focus is the unselected image group (`↑`/`↓` move and select, `Space` selects), `Tab` reaches *Set up* (once enabled; a disabled button is skipped) and *Cancel*, Esc cancels; focus returns through `ui::dispatch` (KBD-002); no focus ring (KBD-003). Check that the new action is bound or in the palette (`a11y_every_action_bound_or_in_palette`).
- Strings go in `strings.rs` (no mechanism words, ENG-161); the image names and descriptions come from the factory's `SetupChoice` data, so the UI hard-codes neither (ENG-030). GPUI Kit `Dialog`, `Button`, `Progress`, `Alert`, radio group only; no custom elements.

### 6.3 Errors

| Situation | State / shown |
|---|---|
| Docker Desktop installed, pipe absent, status not running | `Stopped`, start prompt |
| `docker desktop start` exits non-zero or times out | `Failed`, message + hint (finish the Docker Desktop window, licence/sign-in) |
| `desktop` plugin missing | fall back to `Docker Desktop.exe` + poll; on timeout same hint |
| Docker Desktop running but pipe still fails | `Failed` with today's hint (not `Stopped`) |
| Resource Saver | connect normally with the longer timeout; no start offered |
| `dockerd` start fails (no systemd/service, permission) | `Failed`, command message + existing group/socket hints |
| WSL missing/too old, virtualisation off | setup hidden or preflight error with `wsl --update` hint (ENG-156) |
| Disk < 2 GiB, no network, HTTP error | setup error with hint; nothing left behind |
| SHA-256 mismatch | delete file, error "download failed verification", no import |
| Import fails / distro exists | no cleanup of a pre-existing distro (ENG-155); clear message |
| User cancels | cleanup of this run's artifacts only; no toast error |

## 7. Tasks

| # | Task | Owner agent | IDs | Verify |
|---|---|---|---|---|
| 0 | **Spikes** (below): W-1 Docker Desktop status values and start behaviour, incl. `Docker Desktop.exe` fallback and Resource Saver; W-2 start `dockerd` in Ubuntu with/without systemd; W-3 **both** rootfs images (Alpine and Ubuntu): size, import time, boot mechanism (`wsl.conf` boot command vs systemd), cgroups/iptables, ARM64; W-4 import location and `wsl --unregister` cleanup, WSL missing/old messages | windows-platform | ENG-143…149, 152, 153, 156 | spike report appended to `docs/plan/spikes/` and `⚠ verify` removed from the spec |
| 1 | Decide A4; add `SetupOffer`, `SetupPhase`, `SetupProgress`, `stopped_after`, `setup_offers`, `setup` to `dk-core` with defaults; `FakeEngine` factory support; hub: async `stopped_after` at the supervisor/autoselect call sites (ENG-137 reimplemented on it), `setup_offers`, `run_setup` with stale guard and registration, first-run data | rust-core | ENG-142, 146, 150, 154, 158 | hub tests with a fake factory |
| 2 | Docker Desktop: registry detection, `Stopped` listing, `start_prompt`, `start` (CLI + exe fallback), `stopped_after` via status, Resource Saver handling; `cfg(windows)` with stubs elsewhere | engine-integrator | ENG-143…147 | fixtures + fake `docker.exe` test binary (like `fake_wslc`) |
| 3 | Distro: probe exit 14, `Stopped` listing, `start_prompt`, `start` (systemd / `service`), `stopped_after`; unchanged `no-docker` behaviour | windows-platform | ENG-148, 149 | `dk-wsl` unit tests with the fake runner + opt-in live test |
| 4 | Dedicated distro setup in `dk-wsl`: preflight, streamed download, SHA-256, `wsl --import`, boot, ping, cleanup guard, offer logic, per-(choice, architecture) constants, `choice_id` validation (a missing or unknown id is rejected), `SetupChoice` data | windows-platform | ENG-150…157, 160, 162 | unit tests with a local HTTP server and a fake `wsl.exe`; opt-in live test on the `wsl` runner, once per image |
| 5 | UI: start rows and setup rows on first-run/no-active pages, consent dialog with the image radio group, progress view, `SetUpEngine` action + palette entry, Settings → Engines button, focus handling, strings; light/dark screenshots | gpui-ui | ENG-146…147, 150…151, 154, 158, 159, 161, 162 | view tests + screenshots + the keyboard a11y tests |
| 6 | Rootfs build for Alpine and Ubuntu (2 × 2 matrix), smoke test per cell, publish, attestation, licence notices; pin-check job | release-engineer | REL-088, REL-089, ENG-152 | `engine-rootfs.yml` green on the `wsl` runner for every cell; pin check fails on a wrong digest |
| 7 | Tests and ID→test table; release-checklist items; docs | qa-engineer | all | `cargo nextest`, checklist |
| 8 | Review | reviewer | all | verdict: approve |

Order: task 0 first and gates 2, 3, 4 and 6. Tasks 1 and 5 can start in parallel after approval (5 against the fake factory). Task 4's constants wait for task 6's first published asset (use a local test asset until then).

## 8. Test plan

| ID | Layer | Test name(s) |
|---|---|---|
| ENG-142 | CI grep, core | no UI code matches `docker desktop`, `wsl.exe`, `dockerd`; defaults compile on Linux/macOS (`eng_142_defaults_offer_nothing_off_windows`) |
| ENG-143 | engine-docker | `eng_143_installed_desktop_without_pipe_is_stopped`, `eng_143_no_install_lists_nothing`, `eng_143_discovery_spawns_no_process` |
| ENG-144 | engine-docker (fake `docker.exe`) | `eng_144_start_runs_one_fixed_argv`, `eng_144_missing_plugin_falls_back_to_exe_and_polls`, `eng_144_never_started_by_connect_probe_or_retry` |
| ENG-145 | engine-docker | `eng_145_status_json_parsing`, `eng_145_resource_saver_is_not_stopped`, `eng_145_probe_never_runs_status` |
| ENG-146 | hub | `eng_146_stopped_after_true_is_stopped_not_failed`, `eng_146_false_stays_failed`, `eng_137_*` (existing, via the hook) |
| ENG-147 | hub, UI | `eng_147_start_failure_is_failed_with_hint`, `eng_147_start_timeout_hint` |
| ENG-148 | wsl | `eng_148_exit_14_listed_stopped`, `eng_148_exit_11_still_hidden` |
| ENG-149 | wsl | `eng_149_systemd_and_service_argv`, `eng_149_waits_for_socket_then_errors`, `eng_149_never_edits_the_distro` |
| ENG-150 | wsl, hub | `eng_150_offer_only_on_windows_with_wsl2_and_no_distro`, `eng_150_offer_hidden_when_distro_exists` |
| ENG-151 | UI | `eng_151_nothing_happens_before_consent`, `eng_151_consent_text_names_size_and_folder` |
| ENG-152 | wsl | `eng_152_each_choice_and_arch_has_its_own_pin`, `eng_152_unknown_choice_id_rejected`, `eng_152_digest_mismatch_deletes_and_fails`, `eng_152_streams_to_disk_not_memory`, `eng_152_constants_are_not_user_input` |
| ENG-153 | wsl, live | `eng_153_happy_path_with_fakes`; opt-in live: `setup_live` on the `wsl` runner |
| ENG-154 | hub, wsl | `eng_154_cancel_before_import_removes_cache`, `eng_154_cancel_after_import_unregisters_own_distro`, `eng_154_stale_setup_events_dropped` |
| ENG-155 | wsl | `eng_155_existing_distro_never_touched`, `eng_155_failure_before_import_never_unregisters` |
| ENG-156 | wsl | `eng_156_missing_wsl_gives_hint_no_install`, `eng_156_low_disk_message` |
| ENG-157 | review | no `winget`/`msiexec`/`apt` invocation outside the image build (grep test) |
| ENG-158 | UI | `eng_158_first_run_lists_startable_and_offers` |
| ENG-159 | UI | `eng_159_palette_entries_and_focus`, `a11y_every_action_bound_or_in_palette` (existing) |
| ENG-160 | review, wsl | `eng_160_argv_only_and_registry_paths` |
| ENG-161 | UI | `eng_161_strings_have_no_mechanism_words` |
| ENG-162 | wsl, hub, UI | `eng_162_choices_omit_missing_assets`, `eng_162_dialog_opens_with_nothing_selected`, `eng_162_set_up_disabled_until_a_choice_is_made`, `eng_162_single_choice_must_still_be_picked`, `eng_162_no_choice_is_remembered_between_opens`, `eng_162_no_recommended_marker`, `eng_162_setup_without_choice_id_rejected`, `eng_162_consent_sizes_follow_selection`, `eng_162_radio_keyboard_and_focus`, `eng_162_ui_hardcodes_no_image_names` |
| REL-088/089 | CI | `engine-rootfs.yml` smoke job; `rel_089_pin_matches_published_asset` |

## 9. Risks & spikes

| Risk | Mitigation / spike |
|---|---|
| Docker Desktop's `status` values and behaviour in Resource Saver, first run and update states are unverified; `docker desktop start` may block on a UI prompt | Spike **W-1**; the start has a hard timeout and a user-facing hint; the spec stays `⚠ verify` until done. Verified already: the CLI exists locally (4.95.0), `start` has `--detach`/`--timeout`, `status` prints `Status`/`PauseType` |
| `docker.exe desktop` plugin missing on old Desktop | exe fallback + pipe polling (ENG-144) |
| Starting dockerd as root in a distro differs by init system | Spike **W-2**; fixed argv per init system, error otherwise; never edits the distro |
| **Supply chain**: a root filesystem we ship runs as the user's WSL root | Pinned SHA-256 constants in the signed app, build attestation, reproducible pinned package versions, only a GitHub asset in our own repo, user consent shows what it is (ENG-151/152/160). Releases are currently unsigned; the pin lives in the app binary users already trust |
| **Maintenance burden**: we own two patched Docker images | Independent `rootfs-<n>` cadence (REL-088), scheduled rebuild for security updates, the app offers an image but never auto-updates an existing distro (v1) |
| Alpine `dockerd` quirks in WSL (cgroups v2, iptables/nft, `/dev` mounts, no systemd) | Spike **W-3** and the CI smoke test (`hello-world`, a published port, a bind mount from Windows) for both images before task 4. If Alpine fails, its choice is dropped and Ubuntu stays the only choice (the UI already handles one entry) |
| **Two images double the build work**: two recipes, a 2 × 2 smoke matrix, two sets of pins and licence notices | Shared build script with one recipe per image, one `rootfs-<n>` tag and one checksum file; a choice can be dropped later by editing the pin table, since `SetupChoice` is data |
| The Ubuntu image is much larger and slower to download and import | The user decides: each row states its trade-off (*Larger download* / *Smallest and fastest to set up*), the sizes show as soon as one is picked, and neither is preselected or marked recommended |
| The Ubuntu image relies on systemd inside WSL (`systemd=true`) | Spike W-3 verifies it on the minimum WSL version the setup accepts; the preflight rejects older WSL with the `wsl --update` hint (ENG-156) |
| Disk, VHD and network: a 100–200 MB download and a 1 GB+ VHD | Preflight (ENG-153), consent shows sizes, cancel works, cache deleted |
| Dockering must not harm user distros | ENG-155: only `dockering-engine`, only if absent, unregister only in the run that created it. Tests with a fake `wsl.exe` assert every argv |
| Adds an HTTP download to `dk-wsl`, which is an NFR-023 exception | Documented in NFR-023, one GET from `github.com` after consent, no telemetry |
| Corporate machines: WSL disabled by policy, no GitHub access | ENG-156 guidance; the rest of the app is unaffected |
| Scope is large for one plan | Tasks 2–3 (starting) can ship first behind their own PR; tasks 4–6 (setup) follow. IDs are separate, so the spec status can be `in-progress` for the first group |
| ARM64 image availability | A13: omit a choice with no published asset for the architecture; hide the offer when none remain |

## 10. Revision log

- 2026-10-22 (review, task 8): verdict approve with changes, no blockers. Fixed in code: ENG-163 page text for a stopped `dockering-engine`, an identity check before `--unregister`, the panic-path gate order, a re-hash of the file right before the import, a flaky UI test and the ping line endings. Spec amended to match the code: ENG-145 (no longer first-connect timeout; the 30 s timeout already covers the documented 3–10 s wake-up), REL-088 (aarch64 images are built but not smoke-tested, so their pins stay unpublished and ARM64 gets no offer; "reproducible" means pinned bases and recorded package versions), spec 20 ENG-020 (the read-only re-probe of a running not-running distro). Open follow-ups: flip `--allow-unpublished` off in `ci.yml` and `release.yml` after the first `rootfs-<n>` publish (owner: whoever publishes it); an optional `release-approval` gate on the publish job.
- 2026-10-21 (spike W-1…W-4 done, task 0): [report](../spikes/2026-10-windows-engine-start.md). Spec updated from it: ENG-143/144/145/148/149/151/152/153/154/156 corrected (HKLM verified; `unknown command` is the fallback trigger; absolute `/usr/sbin/service`; poll `GET /_ping`, not the socket file; free space = `download_bytes + disk_bytes + 1 GiB`; parent dir of the import must exist; Alpine boot command needs an explicit `PATH`; cleanup rule), **ENG-163** (containers run only while Dockering or another WSL session is open: the distro idles out ~15 s after the last session) and **ENG-164** (`/etc/wsl-distribution.conf` disables the Start Menu shortcut and Terminal profile) added, hosted format `.tar.xz`, Ubuntu base 26.04.1. Docker Desktop's stopped, starting, paused and Resource Saver states were **not** observable (Docker Desktop was running) and stay `⚠ verify` with a manual acceptance item; the code treats anything but a parsed `running` as not running.
- 2026-10-21 (revision 2, user request): **no default image; the user must decide explicitly.** Removed the Alpine preselection and the remembered `setup_choice`: the dialog opens with nothing selected, *Set up* is disabled until the user picks (hint *Choose an image to continue*), no *Recommended* marker, a single remaining choice must still be picked, and `choice_id` is mandatory (`&str`, not `Option`). Removed `SetupChoice.default`. Changed A12/A13, ENG-151/159/162 wording, the dialog sketch, keyboard rules, task 4, the ENG-162 tests (removed `eng_162_default_is_alpine_then_last_choice`, `eng_162_choice_persisted_only_after_success` and `eng_162_single_choice_is_preselected_without_radio`) and the size risk row.
- 2026-10-21 (revision 1, user request): the user can **choose between an Ubuntu and an Alpine image** when there is no WSL engine to start. Added ENG-162, `SetupChoice` and a `choice_id` on `setup` / `run_setup` (ENG-142…162 became ENG-142…162). Changed A2, replaced A11 by A13, added A12. Updated ENG-151/152/153/154/155/159/161 and REL-088/089 for two images, the dialog sketch and keyboard rules, a 2 × 2 build and smoke matrix, W-3 covering both images, and three new risks (build work, size, systemd). The consent default is Alpine. No other flow changes.
- 2026-10-21: created from the request "start Docker Desktop / WSL, and offer to initialise Docker in WSL from scratch when the Windows engine isn't responding or none is found". The user chose full scope now (provisioning included, which changes the product non-goal) and an imported rootfs for the distro.
