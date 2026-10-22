# Spike report: Windows engine start and setup (W-1 … W-4)

- **Date:** 2026-10-21. **Plan:** [windows-engine-start](../features/windows-engine-start.md) task 0. **Spec:** [windows-engine-start](../../spec/features/windows-engine-start.md), [20 §2.1/§4](../../spec/20-engine-backends.md).
- **Machine:** Windows 11 Pro 10.0.26300, WSL 3.0.3.0 (kernel 6.18.54.1-1), Docker Desktop 4.95.0 **running** (server 29.8.2, API 1.56), distros `Ubuntu-22.04` (default, systemd, Desktop-integrated) and `docker-desktop`.
- **Safety record:** Docker Desktop was never stopped, restarted or modified (`docker desktop status` was `running` again at the very end). On `Ubuntu-22.04` only read-only commands ran (`cat`, `ls`, `command -v`, `id` as root, and the candidate probe script as the default user). All disposable distros were named `dockering-spike-<alpine|ubuntu>-<n>`, imported under `%TEMP%\dockering-spike\`, and unregistered. Cleanup evidence is in "Cleanup confirmation" at the end.
- **Legend:** *observed* = measured here; *documented* = from a public source (cited); *inferred* = deduced (flagged); *not observed* = needs a manual check.
- Raw outputs below are trimmed. Scripts used lived in `%TEMP%\dockering-spike\scripts\` (deleted); the ones worth keeping are reproduced in this report.

---

## 0. Executive summary

| Topic | Result |
|---|---|
| Docker Desktop start (W-1) | `docker desktop start` exists and returns **exit 0 / "Docker Desktop is already running"** when running (observed). **Not observed** (Desktop was running): the stopped, starting, paused and Resource Saver states, the pipe being absent when stopped, what `start` does while stopped, its timeout exit code. All other `Status` values are unverified (public docs list none). |
| dockerd start in a distro (W-2) | Works without a password (`wsl.exe -u root --exec`, observed). systemd: `systemctl start docker` (1.2 s). No systemd: `/usr/sbin/service docker start` (0.2 s). **Bare `service` fails under `--exec`** (PATH); use the absolute path. **A socket file is not a readiness signal** (systemd socket activation, stale sockets); use `/_ping`. |
| Probe exit 14 (W-2) | A tested script (below) classifies 11/12/13/14/0 correctly in 19 states. A `docker` client alone (incl. Docker Desktop's dangling symlink) stays 11. |
| Alpine image (W-3) | **Works** (hello-world, published port, `/mnt/c` bind mount, cgroup v2, iptables-nft). Needs an explicit `PATH` in the boot command. 58 MB (xz) / 94 MB (gz), 364 MB VHDX, dockerd ready 1.4 s after boot. |
| Ubuntu image (W-3) | **Works** with `systemd=true`. 355 MB (xz) / 507 MB (gz), 1.7 GB VHDX, ready 3.1 s after boot. |
| Behavioural surprises | (1) The distro **idles out ~15 s after the last `wsl.exe` session and kills all containers**; a boot-command process does not prevent it. (2) Import adds a **Start Menu shortcut + Windows Terminal profile** unless the image ships `/etc/wsl-distribution.conf`. (3) `--import` needs the **parent** dir to exist. (4) Intermittent 8–25 s first-exec latency after an idle stop (cause unknown). (5) ≥ 2 GiB free is too low for Ubuntu. |
| Cancel/cleanup (W-4) | Killing `wsl.exe --import` at 0.3–3.0 s left **nothing** (no key, no dir, no list entry) in 7 attempts; at ≥ 3.1 s the import had already finished and the distro was complete and usable. `wsl --unregister` removes registry key, directory, Start Menu shortcut and Terminal fragment. |

### Decisions the plan needs

1. **D1 Lifetime:** accept (and document in consent/hints) that `dockering-engine` and its containers run only while Dockering, or another WSL session, holds the distro (§W-3.7). Or require a global `.wslconfig` change (rejected: touches the user's configuration).
2. **D2 Readiness:** ENG-149/153 must wait for `GET /_ping`, not for the socket file.
3. **D3 Free space:** replace "≥ 2 GiB" by a per-image figure (`download_bytes + disk_bytes + 1 GiB`: about 3 GiB Ubuntu, 1.5 GiB Alpine).
4. **D4 Compression:** host `.tar.xz` (WSL imports it directly, 1.6× smaller than gz for Alpine).
5. **D5 Start Menu/Terminal clutter:** ship `/etc/wsl-distribution.conf` disabling both (verified on Alpine; Ubuntu needs the same file edited; see §W-3.8).
6. **D6 Ubuntu base:** 26.04.1 LTS (Resolute) or 24.04.5 (Noble)? Both work with Docker's apt repo (26.04.1 tested). The plan assumed "current LTS" = 26.04.1.
7. **D7 Alpine reproducibility:** Alpine's repos do not keep old package versions, so "pinned versions" means a recorded manifest of the built image, not a re-runnable `apk add pkg=ver`.

---

## W-1 Docker Desktop (read-only on the live instance)

### W-1.1 Install identification

Commands: `reg query` on HKLM, HKCU and HKLM\WOW6432Node `...\Uninstall\Docker Desktop`; `ls` of the install folder.

```
HKLM\Software\Microsoft\Windows\CurrentVersion\Uninstall\Docker Desktop
    DisplayIcon    REG_SZ    C:\Program Files\Docker\Docker\Docker Desktop Installer.exe
    DisplayName    REG_SZ    Docker Desktop
    DisplayVersion REG_SZ    4.95.0
    Version        REG_SZ    242831
    InstallLocation REG_SZ   C:\Program Files\Docker\Docker          (no trailing backslash)
    InstallType    REG_SZ    exe
    Publisher      REG_SZ    Docker Inc.
    UninstallString REG_SZ   "C:\Program Files\Docker\Docker\Docker Desktop Installer.exe" "uninstall"
    EstimatedSize  REG_DWORD 0x3469e2        (NoModify/NoRepair = 1)
HKCU\...\Uninstall\Docker Desktop                     -> key not found
HKLM\SOFTWARE\WOW6432Node\...\Uninstall\Docker Desktop -> key not found
```

- *Observed:* an all-users install is **HKLM only**. *Documented* ([install docs](https://docs.docker.com/desktop/setup/install/windows-install/)): `--user` installs per user into `%LOCALAPPDATA%\Programs\DockerDesktop` with HKCU registry keys. **Not observed** (no such install here), so the HKCU-then-HKLM lookup in ENG-143 stays right but is unverified for HKCU.
- Files (observed):
  - `<InstallLocation>\Docker Desktop.exe` (the GUI/backend launcher; running as PID 26644 in the session).
  - `<InstallLocation>\resources\bin\docker.exe` (the CLI; also `docker-compose.exe`, `kubectl.exe`, `docker-credential-*.exe`).
  - The `desktop` CLI plugin: `<InstallLocation>\resources\cli-plugins\docker-desktop.exe` (21 157 296 bytes) **and** a hard-link copy at `C:\Program Files\Docker\cli-plugins\docker-desktop.exe`. `docker.exe info` reports every plugin from `C:\Program Files\Docker\cli-plugins\` (Docker CLI's standard system plugin dir), **not** from `resources\bin` or `resources\cli-plugins`. `%USERPROFILE%\.docker\cli-plugins` is empty.
- Plugin version: `docker desktop version` → `Docker Desktop CLI plugin version: 4.95.0`, exit 0.

### W-1.2 `docker desktop` CLI

```
> docker.exe desktop --help      (trimmed)
Management Commands: auth, disable, enable, engine (Windows only), kubernetes
Commands: diagnose, logs, restart, start, status, stop, update, version
> docker.exe desktop start --help
  -d, --detach            Do not synchronously wait for the requested operation to complete.
      --timeout seconds   Terminate the running command after the specified timeout with a non-zero
                          exit code. A value of zero (the default) or -1 means no timeout.
> docker.exe desktop status --help
      --format string   Format the output. Accepted values are: pretty, json (default "pretty")
> docker.exe desktop engine ls   -> "linux *   Linux engine"
```

`status` output (observed, running):

```
--format json:                          --format pretty:
{                                       Name                Value
    "PauseType": "",                    Status              running
    "SessionID": "4de90c9a-…-5f517f55bec5",   SessionID           4de90c9a-…
    "Status": "running"
}
```

| Command (running instance) | stdout/stderr | Exit | Time |
|---|---|---|---|
| `docker desktop status --format json` | JSON above | 0 | 134–160 ms (3 runs) |
| `docker desktop start --timeout 20` | `Docker Desktop is already running` | **0** | 180 ms (a no-op, safe) |
| `docker desktop status --format bogus` | `format value "bogus" could not be parsed` | 1 | n/a |
| `docker notaplugin` (stand-in for a missing plugin) | `docker: unknown command: docker notaplugin` | 1 | n/a |
| `docker desktop nosuchsub` | prints usage | **0** | n/a (so exit 0 does not prove the subcommand ran; fixed argv makes this moot) |

### W-1.3 Pipes and `/_ping`

28 pipes matching `docker*` exist while Desktop runs, among them: `docker_engine`, `dockerDesktopLinuxEngine`, `dockerDesktopWindowsEngine`, `docker_engine_linux`, `dockerDesktopEngine`, `docker_cli`, `dockerBackendApiServer`, … ENG-006 should keep naming only the first two.

`GET /_ping` over both `\\.\pipe\docker_engine` and `\\.\pipe\dockerDesktopLinuxEngine` (a .NET `NamedPipeClientStream`; `curl` here has no `--npipe`):

```
HTTP/1.1 200 OK
Api-Version: 1.56
Docker-Experimental: false
Ostype: linux
Server: Docker/29.8.2 (linux)
Transfer-Encoding: chunked      (body "OK", chunked: 2\r\nOK\r\n0)
```

### W-1.4 Status values and `start` exit codes: documented vs observed

The public docs ([`docker desktop status`](https://docs.docker.com/reference/cli/docker/desktop/status/), [`start`](https://docs.docker.com/reference/cli/docker/desktop/start/), [Desktop CLI](https://docs.docker.com/desktop/features/desktop-cli/), [`docker/docs` data YAML](https://github.com/docker/docs/blob/main/data/cli/desktop/docker_desktop_status.yaml)) list **no status values and no exit codes**. They only say: `status` "Displays whether Docker Desktop is running or stopped", `--format` is `pretty|json`, `--timeout` ends the command "with a non-zero exit code".

| Item | Value | Basis |
|---|---|---|
| `Status` = `running` | `PauseType` = `""` | **observed** |
| `Status` for stopped, starting, paused, resource saver, failed | unknown | **not observed, not documented** |
| String scan of `docker-desktop.exe` (21 MB Go binary; *inferred*, not proof) | messages `Docker Desktop is not running`, `Docker Desktop is still starting`, `Docker Desktop is already running`, `Failed to start Docker Desktop`, `Failed to stop Docker Desktop`, `Forcing Docker Desktop to stop`; a green-coloured label `Paused (manual)`; the tokens `resource-saver` / `Resource Saver` next to `PauseError`; JSON keys `Status`, `PauseType`, `SessionID` | *inferred*: suggests `PauseType` carries `manual` / `resource-saver`-like values, and that `start` has a "still starting" outcome |
| Resource Saver behaviour | enabled by default; after 5 min idle it stops the Linux VM; on Windows/WSL it "pauses only the Docker Engine in `docker-desktop`"; container work wakes it automatically (3–10 s, WSL immediate); the status bar shows a moon icon | **documented** ([Pause / Resource Saver](https://docs.docker.com/desktop/use-desktop/pause/)); here `UseResourceSaver=false`, so **not observed** |
| `start` exit 0 when already running | 0 | **observed** |
| `start` non-zero on `--timeout` expiry | non-zero (exact value unknown) | **documented**, value **not observed** |
| `start` exit codes for licence/sign-in wait, failure | unknown | **not observed** |
| `docker desktop` command introduced in version X | unknown (release notes show `docker desktop start` present by 4.70.0; the spec's "4.37+" is unverified) | partly **documented** |

### W-1.5 What could NOT be observed (manual check on a machine where Docker Desktop is stopped)

1. `docker desktop status --format json|pretty` while **stopped** (value text, exit code, stdout/stderr split), while **starting**, **paused (manual)**, and in **Resource Saver**.
2. Whether `\\.\pipe\docker_engine` and `\\.\pipe\dockerDesktopLinuxEngine` are **absent** when Desktop is stopped (the "Docker Desktop Service" might own a pipe) — ENG-143 depends on this.
3. What `docker desktop start --timeout 120` does while stopped: does it open the GUI window, how long until the pipe answers `/_ping`, exit code on success, on timeout and when a licence/sign-in/update dialog waits.
4. The Resource Saver wake-up: pipe present while paused? first `/_ping` latency?
5. The `Docker Desktop.exe` direct-launch fallback: window behaviour, time to pipe, whether it needs an interactive session.
6. A Docker Desktop without the `desktop` plugin (old versions) to confirm the "unknown command, exit 1" fallback trigger.
7. A per-user install (`--user`): the HKCU key and plugin location.
8. Windows-container mode: which engine answers on `docker_engine` (`docker desktop engine ls/use`).

### W-1 spec impact

- **ENG-143:** `InstallLocation` has no trailing backslash; `<InstallLocation>\Docker Desktop.exe` exists (observed). The HKCU path is documented only.
- **ENG-144:** `<InstallLocation>\resources\bin\docker.exe` confirmed. Say that `docker.exe` finds the `desktop` plugin in `%ProgramFiles%\Docker\cli-plugins\`, so the fallback trigger is the CLI result (`unknown command`, exit 1), not a file check under `resources\bin`. A second `start` while running prints `Docker Desktop is already running`, exit 0.
- **ENG-145:** record the observed JSON shape and the table form; state that **only `running` is verified**; parse tolerantly (anything unparsable or non-zero = not running); the status call costs ~150 ms. Keep `⚠ verify` for stopped/paused/resource-saver values until the manual check (§W-1.5) is done.
- **20 §2.1 first bullet:** replace "(`running` seen; the other values are spike W-1)" with "(`running` seen; other values unverified; docs list none)".

---

## W-2 Starting dockerd in a distro

### W-2.1 Read-only look at `Ubuntu-22.04`

```
comm=systemd   /run/systemd/system exists   user=pavel(1000)   /etc/wsl.conf: [boot] systemd=true
dockerd=none  containerd=none  runc=none  socat=none   docker=/usr/bin/docker  service=/usr/sbin/service  systemctl=/usr/bin/systemctl
/usr/bin/docker -> /mnt/wsl/docker-desktop/cli-tools/usr/bin/docker        (Docker Desktop WSL integration)
/var/run/docker.sock  srw-rw---- root:docker   (a Docker Desktop proxy socket, not a local dockerd)
dpkg: no docker*/containerd* packages
```

Consequence: an integration distro never has a local `dockerd`, so *Start Docker* must never be offered for it. The candidate probe returns 0 here while Desktop runs (socket present, no `dockerd` binary), and 11 when the socket is gone (test C2 below: dangling symlink).

### W-2.2 Start commands in a disposable Ubuntu 26.04.1 distro (docker-ce installed)

All as `wsl.exe -d <d> -u root --exec <argv>` (never needed a password; `id` printed `uid=0`; also true on `Ubuntu-22.04`).

| State | argv | exit | stderr / stdout | time |
|---|---|---|---|---|
| systemd on, both units stopped | `systemctl start docker` | 0 | none | 1.2 s wall; `docker version` works immediately after |
| systemd on, socket unit active, service stopped | (client connects) | n/a | socket activation starts dockerd on first connect | 1.3 s |
| systemd on, unit missing | `systemctl start doesnotexist` | **5** | `Failed to start doesnotexist.service: Unit doesnotexist.service not found.` | |
| systemd **off** | `systemctl start docker` | **1** | `System has not been booted with systemd as init system (PID 1). Can't operate.` + `Failed to connect to system scope bus via local transport: Host is down` | |
| systemd off | `service docker start` | **1** | `<3>WSL (657 - Relay) ERROR: CreateProcessCommon:875: execvpe(service) failed: No such file or directory` | 110 ms |
| systemd off | `/usr/sbin/service docker start` | 0 | ` * Starting Docker: docker\n ...done.` | socket at ~190 ms |
| systemd off, already running | `/etc/init.d/docker start` | **1** | ` * Starting Docker: docker` | |
| systemd on | `service docker start` | 1 (same execvpe error) | | |
| systemd on | `/usr/sbin/service docker start` / `/usr/bin/systemctl start docker` | 0 / 0 | | |

Findings:

- `--exec` resolves the program with the relay's own `PATH`, which lacks `/usr/sbin`. `service` and `usermod` fail bare, `systemctl` works (`/usr/bin`), absolute paths always work. The in-child `PATH` shown by `env` includes `/usr/sbin`, so a `sh -c` wrapper also works, but absolute paths avoid a shell.
- The Ubuntu docker-ce package enables `docker.service`, `docker.socket` and `containerd.service`. `docker.socket` creates `/var/run/docker.sock` **at boot, before dockerd runs**, and the file **survives `systemctl stop docker.socket docker.service`** as a stale socket (`test -S` stays true, `docker version` → "Cannot connect to the Docker daemon"). Hence the socket file is **not** a readiness signal; `GET /_ping` is.
- `systemctl is-system-running` reports `degraded` right after boot (`getty@tty1.service` fails) and `running` later; do not use it for readiness.
- Without systemd the init.d script starts `dockerd -p /var/run/docker.pid`, logging to `/var/log/docker.log`.
- Start is idempotent for systemctl (0) but not for `service … start` (1 if already running): ping first, then start, then ping.

**Best fixed argv + readiness check**

```
if /run/systemd/system exists (the existing probe fact):   systemctl start docker                       (rc 0 = started)
else:                                                       /usr/sbin/service docker start              (rc 0 = started)
then poll GET /_ping through the bridge (docker system dial-stdio, 250 ms steps) up to 30 s.
```

Timings: systemd 1.2 s, sysv 0.2 s; 30 s is ample. Report the argv's stderr verbatim on a non-zero exit or on timeout.

### W-2.3 Probe script for exit 14

Current script (crates/dk-wsl/src/discovery.rs:18, not edited):

```sh
test -S /var/run/docker.sock || exit 11; t=$(command -v docker || command -v socat) || exit 12; echo "$t"; test -r … && test -w … || exit 13
```

Problems it cannot solve: no socket means 11 for every case, and a socket does not prove dockerd is running (stale socket after a crash or `docker.socket` alone).

**Proposed script (tested in 19 states, bash→`wsl.exe --exec sh -c`; dash on Ubuntu, busybox ash on Alpine):**

```sh
d=/var/run/docker.sock
if test -S $d; then
  if command -v dockerd >/dev/null 2>&1; then
    run=
    for c in /proc/[0-9]*/comm; do read -r n < "$c" 2>/dev/null && test "$n" = dockerd && run=1 && break; done
    if test -z "$run"; then
      if test -d /run/systemd/system && systemctl is-active --quiet docker.socket 2>/dev/null; then :; else exit 14; fi
    fi
  fi
  t=$(command -v docker || command -v socat) || exit 12
  echo "$t"
  test -r $d && test -w $d || exit 13
else
  if command -v dockerd >/dev/null 2>&1 || test -e /etc/init.d/docker || test -e /usr/lib/systemd/system/docker.service || test -e /lib/systemd/system/docker.service; then exit 14; fi
  exit 11
fi
```

Design: a distro whose socket comes from somewhere else (Docker Desktop integration: no `dockerd` binary) keeps today's behaviour; a distro with a `dockerd` binary but no live `dockerd` process is `14` unless systemd socket activation (`docker.socket` active) can start it on connect (then it is simply available). No `pgrep` (absent in minimal images); stdout stays "tool path on the last line" so `classify_probe` is unchanged.

| # | State | Exit (expected) | Exit (observed) |
|---|---|---|---|
| C1 | bare Alpine | 11 | 11 |
| C2 | dangling `/usr/bin/docker` symlink only (Desktop-integration distro with Desktop stopped) | 11 | 11 |
| C3 | `docker-cli` client only, no dockerd | 11 | 11 |
| C4 | `dockerd` installed, no socket | 14 | 14 |
| C5 | `dockerd` running (Alpine, manual start) | 0 | 0 |
| C6 | `dockerd` killed `-9`, stale socket left | 14 | 14 |
| S1 | sysv, dockerd running | 0 | 0 |
| S2 | sysv, cleanly stopped (socket removed) | 14 | 14 |
| S3 | sysv, `kill -9` | 14 | 14 |
| S4 | sysv, stale socket file, no dockerd | 14 | 14 |
| S5 | systemd, service + socket active | 0 | 0 |
| S6 | systemd, service stopped, socket unit active | 0 (activation) | 0 |
| S7 | systemd, both stopped, stale socket file | 14 | 14 |
| S8 | systemd, both stopped, socket removed | 14 | 14 |
| P1 | default user not in `docker` group, running | 13 | 13 |
| P2 | same, service stopped, socket active | 13 | 13 |
| P3 | same, both stopped | 14 | 14 |
| P4 | user in `docker` group after fresh boot | 0 | 0 |
| real | real `Ubuntu-22.04` (Docker Desktop running), default user | 0 | 0 (`/usr/bin/docker`) |

(Hardening idea, untested: wrap the read as `{ read -r n < "$c"; } 2>/dev/null` so a PID vanishing mid-loop cannot print a shell error; the classifier ignores stderr for 0/11–14 anyway.)

### W-2 spec impact

- **ENG-148 / 20 §4.2 table row 14:** replace "the `docker` client or `dockerd` is installed but there is no socket" with "a `dockerd` binary, `/etc/init.d/docker` or a `docker.service` unit exists and no `dockerd` process owns the socket (or there is no socket)". State that the `docker` client alone, including Docker Desktop's integration symlink, stays 11. State that a systemd distro with `docker.socket` active is `available` (socket activation).
- **ENG-149 / 20 §2.1 bullet 2:** replace `service docker start` by the **absolute** `/usr/sbin/service docker start` (bare `service` fails under `--exec` with `execvpe(service) failed`); `systemctl start docker` may stay bare or be `/usr/bin/systemctl`. Replace "waits up to 30 s for the socket" with "polls `GET /_ping` up to 30 s" (socket file exists before dockerd with systemd and can be stale). Add the observed exit codes (systemctl 5 = unit missing, 1 = not systemd; `service` 1 when already running) and that `-u root --exec` never prompts.
- **ENG-149 scope note:** *Start Docker* is never offered for a Desktop-integration distro (no `dockerd`).

---

## W-3 Both root filesystems

### W-3.1 Sources, URLs, checksums

**Alpine** (official CDN, `latest-releases.yaml` for the `alpine-minirootfs` flavor, branch v3.24; versioned path used, `latest-stable` is a moving alias):

| Arch | URL | Size (bytes) | SHA-256 (published; I verified x86_64) |
|---|---|---|---|
| x86_64 | `https://dl-cdn.alpinelinux.org/alpine/v3.24/releases/x86_64/alpine-minirootfs-3.24.2-x86_64.tar.gz` | 3 701 382 | `c5ca053cfe1d85c5b96dff8b9bc57045f7f184a30ffb6b65776409ca90388677` |
| aarch64 | `https://dl-cdn.alpinelinux.org/alpine/v3.24/releases/aarch64/alpine-minirootfs-3.24.2-aarch64.tar.gz` | 4 028 030 | `9bf70a7f18ea44094cbb5f70c58f9af129c8214745743db0e68e5502cc2ce773` |

Release date 2026-09-17. Download of the x86_64 file: 0.39 s. `apk` packages exist for aarch64 too (community has `docker-engine-29.8.2-r1`), but the arm64 image was **not run**.

**Ubuntu** (Canonical's official WSL root filesystem, same URLs as `wsl --list --online` / [microsoft/WSL `DistributionInfo.json`](https://github.com/microsoft/WSL/blob/master/distributions/DistributionInfo.json); `wsl --list --online` offers `Ubuntu-26.04`, `Ubuntu-24.04`, `Ubuntu-22.04`):

| Arch | URL | Size (bytes) | SHA-256 (matches `https://releases.ubuntu.com/26.04.1/SHA256SUMS`, and I verified x86_64 after download) |
|---|---|---|---|
| amd64 | `https://releases.ubuntu.com/26.04.1/ubuntu-26.04.1-wsl-amd64.wsl` | 418 495 746 | `48d56724b5c8e60f24893e83e73bbb58c60b3ca22fba3da977075420acd54104` |
| arm64 | `https://cdimages.ubuntu.com/releases/26.04.1/release/ubuntu-26.04.1-wsl-arm64.wsl` (301 → 200) | 412 840 386 | `c7d893d7ccb291009e6096c5a946937337b6fd4c426f11b6541f6cd2bf894f7d` (cdimages `SHA256SUMS`) |

Alternative LTS: `https://releases.ubuntu.com/24.04.5/ubuntu-24.04.5-wsl-amd64.wsl` (388 975 696 bytes, sha256 `bb415d824822c4b878125729af451a5d18fb13d1cf5cbed9a7393ad64ac6039e`); arm64 `…cdimages…/24.04.5/release/ubuntu-24.04.5-wsl-arm64.wsl` (388 300 496 bytes, `74478b5024d9047397ba79d17c27f73caf85ada4988f88cffc2892d65e6a806b`). Docker's apt repo has `dists/resolute`, `noble`, `jammy`, with both `amd64` and `arm64` pools. The `.wsl` file is a gzip tar (magic `1f 8b`), and imports directly. Download: 26 s for 418 MB (~16 MB/s on this link).

The Ubuntu rootfs ships `/etc/wsl.conf` (`[boot] systemd=true`) and `/etc/wsl-distribution.conf` (`[oobe] command=/usr/lib/wsl/wsl-setup`, `defaultUid=1000`, `[shortcut]`, `[windowsterminal]`). With `wsl --import` **no OOBE runs**: the default user is root, and a default `wsl -d X -- cmd` launch ran without prompting or hanging.

### W-3.2 Build recipes and package versions (REL-088 proposals)

**Alpine** (from the minirootfs, via `wsl --import` + `wsl -d <d> -u root --exec`):

```sh
apk add --no-cache docker docker-engine containerd runc socat iptables ip6tables     # 33 packages, 270 MiB, 3.6-4.1 s
```
Installed: `docker-29.8.2-r1`, `docker-engine-29.8.2-r1`, `docker-cli-29.8.2-r1`, `docker-cli-buildx-0.34.1-r2`, `containerd-2.3.7-r0`, `runc-1.4.3-r2`, `socat-1.8.1.3-r0`, `iptables-1.8.13-r0` (nf_tables variant; `/usr/sbin/iptables -> xtables-nft-multi`), BusyBox 1.37.0-r31. Minirootfs has no OpenRC (`/etc/init.d` absent), so the boot command is the only start mechanism. The in-image `docker` group exists (gid 101). Alpine's mirrors keep only the current build per branch, so version-pinned `apk add pkg=ver` will break after an update (known Alpine property, not tested here); record the installed versions in a manifest instead (D7). `docker-cli-buildx` is pulled in by the `docker` meta package; dropping `docker` and installing `docker-engine docker-cli` would shed it (not tested).

**Ubuntu** (from the official `.wsl`, as root in the distro):

```sh
apt-get update; apt-get install -y --no-install-recommends ca-certificates curl
install -m 0755 -d /etc/apt/keyrings; curl -fsSL https://download.docker.com/linux/ubuntu/gpg -o /etc/apt/keyrings/docker.asc
echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/docker.asc] https://download.docker.com/linux/ubuntu resolute stable" > /etc/apt/sources.list.d/docker.list
apt-get update; apt-get install -y --no-install-recommends docker-ce docker-ce-cli containerd.io      # 17.3 s
```
Installed: `docker-ce 5:29.9.2-1~ubuntu.26.04~resolute`, `docker-ce-cli 5:29.9.2-1~ubuntu.26.04~resolute`, `containerd.io 2.4.2-3~ubuntu.26.04~resolute`, `iptables 1.8.11` (`iptables-nft` via update-alternatives), `nftables 1.1.6`. The postinst enables `docker.service`, `docker.socket`, `containerd.service` (all `enabled`). Docker keeps old debs in its pool, so exact `docker-ce=5:29.9.2-…` pinning is possible. Cleanup before export: `docker system prune -af; apt-get clean; rm -rf /var/lib/apt/lists/* …`.

### W-3.3 `wsl.conf` contents

**Alpine.** Variants tried against the same image:

| # | `[boot] command` | Result |
|---|---|---|
| 1 | `dockerd -H unix:///var/run/docker.sock … > /var/log/dockerd.log 2>&1` | dockerd exits: `invalid userland-proxy-path: userland-proxy is enabled, but userland-proxy-path is not set` |
| 2 | `/usr/bin/dockerd -H … --userland-proxy-path=/usr/bin/docker-proxy …` | dockerd starts, then dies: `failed to create NAT chain DOCKER: iptables not found` (also `runc`/`nft` not found) |
| 3 | `/bin/sh -c 'echo PATH=$PATH …'` | prints `PATH=/sbin:/usr/sbin:/bin:/usr/bin`, uid 0: the boot command has **no inherited environment** (inferred: this is BusyBox sh's built-in default) |
| 4 | `/usr/bin/env PATH=… /bin/sh -c 'exec /usr/bin/dockerd … >/var/log/dockerd.log 2>&1'` | **works** |

Working file (also tested end to end on the exported and re-imported tarball):

```ini
[boot]
command = "/usr/bin/env PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin /bin/sh -c 'exec /usr/bin/dockerd -H unix:///var/run/docker.sock --userland-proxy-path=/usr/bin/docker-proxy >/var/log/dockerd.log 2>&1'"

[user]
default = root

[interop]
appendWindowsPath = false
```

**Ubuntu** (tested end to end):

```ini
[boot]
systemd=true

[user]
default=root

[interop]
appendWindowsPath=false
```

Notes: the default user is root so the bridge (`wsl.exe -d dockering-engine --exec docker system dial-stdio`) never hits the `docker`-group probe code 13 (a non-root user in the group also works: P4). `[interop] enabled=false` as extra hardening and `[automount] enabled=false` (would break `/mnt/c` bind mounts) were **not** tested.

### W-3.4 Functional checks

| Check | Alpine 3.24.2 (docker 29.8.2) | Ubuntu 26.04.1 (docker-ce 29.9.2) |
|---|---|---|
| dockerd up after `--terminate` + first `--exec` | **yes**, healthy 1.4 s after boot (`docker version`) | **yes**, 3.1–3.5 s (first client call waits for socket activation) |
| `docker version` | Server 29.8.2, API 1.56 | Server 29.9.2, API 1.56 |
| `docker run --rm hello-world` | OK (2.7 s incl. pull) | OK |
| published port from Windows | `-p 18080:80 nginx:alpine` → `http://127.0.0.1:18080/` **200** and `http://[::1]:18080/` **200** | `-p 18090:80` → 127.0.0.1 **200** |
| bind mount from `/mnt/c` | read a Windows file, wrote a file visible in Windows | same |
| cgroups | v2, driver `cgroupfs`; `WARNING: No swap limit support` (WSL kernel) | v2, driver `systemd` |
| storage | `overlayfs` (containerd snapshotter) | `overlayfs` |
| firewall | iptables 1.8.13 **nf_tables**; `nft` binary absent → harmless "Failed to find nft tool" warnings; `iptables -t nat -S DOCKER` shows the DNAT rule | iptables 1.8.11 nft; `nft` present |
| bridge path | `docker system dial-stdio` + HTTP `GET /_ping` → `200 OK` (Server Docker/29.8.2) | same (`Docker/29.9.2`) |
| WSL kernel features | `CONFIG_NF_TABLES=y`, `OVERLAY_FS=y`, `VETH=y`, `MEMCG=y`, `CGROUP_PIDS=y`, `IP_NF_IPTABLES=m` | same kernel |

Concerns:

- **Alpine first-call race:** right after a boot, `wsl.exe -d X --exec docker version` returns **rc 1 in ~250 ms** ("failed to connect to the docker API", boot command still starting); it is healthy ~1.3–1.5 s after the first call (3 of 3 cycles). Ubuntu with systemd does not show this (socket activation blocks the client). The first connect of a "Start & connect" flow must poll (D2).
- **Intermittent slow first call after an idle stop:** early in the session `docker info` right after an idle shutdown took 22.7, 24.9 and 25.5 s (and once 8 s, 12.8 s), while dockerd's own log showed it was ready within ~1.3 s of the start. The runs happened with a published-port container left running at idle shutdown; later repetitions (10+ cycles, containers present or not) took 1.3–1.5 s. **Cause not found** (host load unknown). The plan's 30 s (Docker in distro) and 60 s (setup) timeouts cover it; do not shorten them, and show progress.
- `docker-cli-buildx` and the `docker compose` plugin: Alpine ships buildx via `docker`; Ubuntu has neither (`--no-install-recommends`). The Run dialog does not need them.

### W-3.5 Size and time table

Download size = `wsl --export` tarball compressed (what the project would host). x86_64 only. "VHDX on import" is `ext4.vhdx` right after `wsl --import` (dynamic disk, 1007 GB maximum virtual size).

| | Alpine | Ubuntu 26.04.1 |
|---|---|---|
| Base download (upstream) | 3.70 MB (0.39 s) | 418.5 MB (26 s) |
| Base import (upstream rootfs) | 0.62 s | 5.8 s |
| VHDX after base import | 79.7 MB | 1.455 GB |
| Docker install | 3.6–4.1 s (apk) | 17.3 s (apt) |
| In-guest `du -sx /` after install (after cleanup) | 281 MB (383 MB with 3 images pulled) | 1.57 GB (1.9 GB right after install) |
| VHDX after install | 381.7 MB | 1.76–2.26 GB (grows with apt and images) |
| `wsl --export` (time, size) | 1.7 s, 284 364 800 B | 12.4 s, 1 529 149 440 B |
| `.tar.gz` (level 9 / 6) | **93 989 161 B** (74 s) | **506 850 541 B** (63 s) |
| `.tar.xz` (level 9e / 6, `-T0`) | **58 477 916 B** (108 s) | **355 357 616 B** (156 s) |
| Import of the final `.tar.gz` | 1.5 s | 8.6 s |
| Import of the final `.tar.xz` | 3.2 s | not tested |
| First boot to dockerd answering (`--terminate` then poll) | 1.4–1.5 s | 3.1–3.5 s |
| Cold start after idle (typical) | 1.3–1.5 s (occasionally 8–25 s, see above) | 1.5 s |
| Boot-only (`--exec true`) after idle | 0.27–0.8 s (once 2.3 s) | n/a |

Conclusions: the plan's "~50–70 MB compressed" for Alpine holds for xz (58 MB), not for gz (94 MB). Ubuntu at 355–507 MB is **not** much bigger than Canonical's own 418 MB base because xz repacks the rootfs better. Total user time on a 16 MB/s link: Alpine ≈ 4–6 s download + 2–3 s import + 1.5 s boot ≈ 10 s; Ubuntu ≈ 22–32 s download + ~9 s import + 3.5 s boot ≈ 40–45 s.

### W-3.6 `wsl --import` argv forms and failure behaviour

All via `wsl.exe` (argv, no shell). Exit codes were measured with a .NET `Process` (Rust `ExitStatus::code()` sees `-1`); git-bash shows `127`/`255`.

| Call | Result |
|---|---|
| `wsl.exe --import <name> <dir> <tar> --version 2` | works for `.tar`, `.tar.gz`, `.tar.xz` and the `.wsl` gzip tar; stdout `The operation completed successfully.` (UTF-16LE), exit 0 |
| `--version 3` | `A version number could not be parsed.` `Error code: Wsl/ERROR_VERSION_PARSE_ERROR` |
| **name already exists** (second spike name, never a real distro) | `A distribution with the supplied name already exists. Use --name to choose a different name.` + `Error code: Wsl/Service/RegisterDistro/ERROR_ALREADY_EXISTS`, exit -1. The existing distro was untouched: a marker file written before was still there and `docker --version` still worked; no new directory created. |
| install dir already used by a registered distro | `The supplied install location is already in use.` `Error code: Wsl/Service/RegisterDistro/ERROR_FILE_EXISTS`; nothing registered; existing distro intact |
| install dir holds a stray `ext4.vhdx` | `Failed to create disk '…\ext4.vhdx': The file exists.` `…ERROR_FILE_EXISTS`; nothing registered |
| install dir is an existing **empty** dir | works |
| install dir does not exist, **parent exists** | works (WSL creates the leaf dir) |
| install dir whose **parent does not exist** (`a\b\c\d`) | `The system cannot find the path specified.` `Error code: Wsl/ERROR_PATH_NOT_FOUND` — the app must create the parents |
| missing tarball | `The system cannot find the file specified.` `Error code: Wsl/ERROR_FILE_NOT_FOUND` |
| not a tar | `bsdtar: Missing type keyword in mtree specification` … `Importing the distribution failed.` `Error code: Wsl/Service/RegisterDistro/WSL_E_IMPORT_FAILED`; no dir, no key |
| invalid name (`dockering spike/bad`) | `Invalid distribution name: "…".` `Error code: Wsl/Service/RegisterDistro/E_INVALIDARG` |
| paths with spaces and non-ASCII (`with space ěš`) | works |
| `%LOCALAPPDATA%\<dir>\wsl\<name>`-style path | imports and starts (I did not re-list the directory contents; registry/`wsl -d` confirmed the distro, `--unregister` cleaned it) |
| `wsl.exe --unregister <missing>` | `There is no distribution with the supplied name.` `Error code: Wsl/Service/WSL_E_DISTRO_NOT_FOUND`, exit -1 (treat as "already clean") |
| `wsl.exe -d <missing> --exec true` | exit **127**; the message is **UTF-16LE on stdout** (stderr empty) |
| `--exec` exit code pass-through | `sh -c 'exit 14'` → 14 (so probe codes work) |
| elevation / UAC | none (the whole session ran as a standard user) |

The `Error code: Wsl/…` token is stable and English-looking, while the sentence before it is localised; classify by the token.

**Not observed / described only:**

- **Low disk:** not tested (not filling the disk). The VHDX is dynamic and grows with content; WSL would fail either at VHDX creation or while extracting (`WSL_E_IMPORT_FAILED`-style or a disk-full HRESULT; code unverified). Free-space needs are in the Decisions (D3).
- **WSL missing / old** (from WSL's own `Resources.resw`, verified in source, not triggered here):
  - `This application requires the Windows Subsystem for Linux Optional Component. Install it by running: wsl.exe --install --no-distribution. The system may need to be restarted so the changes can take effect.`
  - `WSL2 is unable to start since virtualization is not enabled on this machine. Please ensure the "Virtual Machine Platform" optional component is enabled and virtualization is turned on in your computer's firmware settings. Enable "Virtual Machine Platform" by running: wsl.exe --install --no-distribution`
  - `Windows version {} does not support the packaged version of Windows Subsystem for Linux. …`
  - The behaviour of the bare System32 `wsl.exe` stub on a machine with no WSL package was not observed (here `C:\Windows\System32\wsl.exe` is 10.0.26100.8875 and forwards to the packaged 3.0.3.0).

### W-3.7 Idle shutdown and container lifetime (important for UX)

Observed on both images:

- A distro stops **13–16 s after its last `wsl.exe` session ends** (`wsl -l --running --quiet` empties). Docs: `[general] instanceIdleTimeout` default 15000 ms in `.wslconfig`, `vmIdleTimeout` 60000 ms.
- A background process started from the boot command does **not** keep it alive: `[boot] command` spawning `sleep 100000` was running, and the distro still idled out (running=1 at +10 s, 0 at +20 s). A held `wsl.exe --exec sleep 45` kept it running (the Dockering bridge child does the same).
- When the distro stops, dockerd dies with it: a container started with `-p` shows `Exited (255)` afterwards and the published port stops answering. So **containers stop ~15 s after Dockering closes** (containers with a restart policy would be restarted on the next start; not tested).
- This matches spec 20 §4.4, but it is a visible difference from Docker Desktop. It also means `dockering-engine` is normally `Stopped` at Dockering start; the existing "stopped distros are listed as *Start & connect*" path (ENG-106) boots it and the boot command starts dockerd (poll, per D2). Exit 14 will mostly not occur for it.

### W-3.8 Start Menu shortcut and Windows Terminal profile

Every imported distro got `ShortcutPath` (`%APPDATA%\Microsoft\Windows\Start Menu\<name>.lnk`) and `TerminalProfilePath` (`%LOCALAPPDATA%\Microsoft\Windows Terminal\Fragments\Microsoft.WSL\{guid}.json`) values in its `Lxss` key (also `Flavor`, `OsVersion`, `Modern=1`). `wsl --unregister` removes both files. To avoid a stray "dockering-engine" entry, the image can carry `/etc/wsl-distribution.conf`:

```ini
[shortcut]
enabled = false

[windowsterminal]
enabled = false
```

Verified on Alpine (write the file, export, re-import: no `ShortcutPath`, no `TerminalProfilePath`, no `.lnk`). The setting only affects imports done **after** the file is in the tarball. For Ubuntu the existing `/etc/wsl-distribution.conf` (OOBE, shortcut icon, terminal template) must be replaced or edited the same way; **not tested on Ubuntu**.

### W-3.9 Versions and `wsl --version`

- `wsl --version` prints UTF-16LE: `WSL version: 3.0.3.0\r\nKernel version: …`. The label is **localised**; parse the first line for `\d+(\.\d+){1,3}`, or read `C:\Program Files\WSL\wslservice.exe` file version (3.0.3.0 here, same as `wsl.exe`), which is the spec 20 §5.2 approach. `wsl --status` is also localised (`Default Distribution: …`).
- Minimum for `systemd=true`: **0.67.6** ([blog](https://devblogs.microsoft.com/commandline/systemd-support-is-now-available-in-wsl/), [wsl.conf docs](https://learn.microsoft.com/en-us/windows/wsl/wsl-config)); the docs add that `[boot]` (incl. `command`) is **Windows 11 / Server 2022 only**. Only 3.0.3.0 was exercised here, so a conservative gate (for example ≥ 2.0.0, the Store-version GA) is a plan choice.
- Docs also list `wsl --version` as the way to detect Store-WSL: it is not recognised on the inbox build ("older inbox version … must be updated"). Not observed.

---

## W-4 Cancel and cleanup

Method: start `wsl.exe --import dockering-spike-ubuntu-kill<n> <dir> <1.5 GB tar> --version 2` from PowerShell (`Start-Process -PassThru`), `Stop-Process -Force` that PID after N ms, then check `wsl -l -v`, the `Lxss` key, the directory and the processes.

| Kill after | Result (checked after 1 s and again after 6–9 s) |
|---|---|
| 300 ms, 800 ms, 1.5 s, 2.2 s, 2.8 s, 2.9 s, 3.0 s | **no** distro in `wsl -l`, **no** `Lxss` key, **no** install directory, **no** `.lnk` |
| 3.1 s – 8 s (import had already finished, process exited rc 0 before the kill; once also a kill at 3.4 s that landed after completion) | distro registered (`State=1`), directory with `ext4.vhdx` (1.72 GB) and `shortcut.ico`, **complete and usable** (`docker --version` worked); `wsl --unregister` removed it |

- The import is done by the WSL service (`wslservice.exe`), not by the client; killing `wsl.exe` either lets it finish or the service rolls back. **No half-created state was ever observed** (~13 attempts over the window). Do not assume this is a guarantee for a crash of the service or a power loss.
- Detection if a half-created distro is ever seen: `HKCU\Software\Microsoft\Windows\CurrentVersion\Lxss\{GUID}` with `DistributionName == "dockering-engine"` and `State != 1`. `LxssDistributionState` (WSL source, [`wslservice.idl`](https://github.com/microsoft/WSL/blob/master/src/windows/service/inc/wslservice.idl)): Installed = 1, Installing = 3, Uninstalling = 4, Converting = 5. I observed only `State = 1`; the other values are *documented*, not observed. A key without `BasePath`/`ext4.vhdx` would also be suspect.
- **`wsl --unregister <name>` cleans everything it created**: the registry key, the directory (including `ext4.vhdx`), the Start Menu `.lnk` and the Terminal fragment JSON (observed by listing before and after). It exits 0 with `The operation completed successfully.`; on a name that is not registered it exits non-zero with `WSL_E_DISTRO_NOT_FOUND` (harmless).
- Leftovers outside WSL's control: WSL created an empty `%LOCALAPPDATA%\Microsoft\Windows Terminal\Fragments\Microsoft.WSL` folder on my first import and left it after the last `--unregister`; I removed it at the end (it was created by this session). The app's own cache file (`%LOCALAPPDATA%\Dockering\cache\…`) is the app's to delete, as designed.
- Practical rule for ENG-154: the safe cleanup is "if this run called `--import` and it returned or may have returned, `wsl --unregister <name>` (ignore `WSL_E_DISTRO_NOT_FOUND`)". Cancelling before the import call needs no unregister; killing the child mid-import needs one only when the import completed in the race window.

---

## Proposed constants for the rootfs build (REL-088, ENG-152)

```text
Alpine base (x86_64): https://dl-cdn.alpinelinux.org/alpine/v3.24/releases/x86_64/alpine-minirootfs-3.24.2-x86_64.tar.gz
                      size 3701382  sha256 c5ca053cfe1d85c5b96dff8b9bc57045f7f184a30ffb6b65776409ca90388677
Alpine base (aarch64): …/aarch64/alpine-minirootfs-3.24.2-aarch64.tar.gz
                      size 4028030  sha256 9bf70a7f18ea44094cbb5f70c58f9af129c8214745743db0e68e5502cc2ce773
Alpine packages (as built 2026-10-21, v3.24.2; record in a manifest, do not pin by apk version):
  docker-29.8.2-r1 docker-engine-29.8.2-r1 docker-cli-29.8.2-r1 docker-cli-buildx-0.34.1-r2
  containerd-2.3.7-r0 runc-1.4.3-r2 socat-1.8.1.3-r0 iptables-1.8.13-r0
Ubuntu base (amd64): https://releases.ubuntu.com/26.04.1/ubuntu-26.04.1-wsl-amd64.wsl
                      size 418495746  sha256 48d56724b5c8e60f24893e83e73bbb58c60b3ca22fba3da977075420acd54104
Ubuntu base (arm64): https://cdimages.ubuntu.com/releases/26.04.1/release/ubuntu-26.04.1-wsl-arm64.wsl
                      size 412840386  sha256 c7d893d7ccb291009e6096c5a946937337b6fd4c426f11b6541f6cd2bf894f7d
Docker apt repo: https://download.docker.com/linux/ubuntu (suite resolute, component stable, key /linux/ubuntu/gpg)
Ubuntu packages (pin exactly): docker-ce=5:29.9.2-1~ubuntu.26.04~resolute
                               docker-ce-cli=5:29.9.2-1~ubuntu.26.04~resolute
                               containerd.io=2.4.2-3~ubuntu.26.04~resolute
Published asset format: .tar.xz (+ .sha256), one per (image, arch); hosted sizes measured: Alpine 58 477 916 B, Ubuntu 355 357 616 B (x86_64, 2026-10-21 build).
Disk to require beyond download: Alpine VHDX 0.37 GB, Ubuntu 1.8 GB (grows with images), plus 1 GiB working margin.
```

Image contents beyond the packages: `/etc/wsl.conf` (as in §W-3.3), `/etc/wsl-distribution.conf` (as in §W-3.8, Alpine verified), `docker` group (exists), no other users, no `/var/cache/apk` or apt lists.

---

## Spec impact (exact corrections for the parent to apply)

### `docs/spec/features/windows-engine-start.md`

1. **ENG-143:** keep the HKLM→HKCU lookup; add "HKLM verified; HKCU documented only; `InstallLocation` has no trailing backslash".
2. **ENG-144:** `docker.exe` path confirmed. Replace the fallback trigger "if the `desktop` plugin isn't there" by "if the `docker desktop` invocation fails with `unknown command` (exit 1)"; add: the plugin lives in `%ProgramFiles%\Docker\cli-plugins\`, not under `resources\bin`; `start` on a running Desktop prints `Docker Desktop is already running` and exits 0.
3. **ENG-145:** document the observed JSON (`{"PauseType","SessionID","Status"}`) and pretty table; mark values other than `running` as unverified; parse tolerantly; the status call takes ~150 ms. Leave `⚠ verify` for the stopped/starting/paused/Resource Saver behaviour until the manual check (§W-1.5).
4. **ENG-148:** redefine exit 14 as "`dockerd` binary, `/etc/init.d/docker` or `docker.service` present and no live `dockerd` (socket absent or stale), and no systemd socket activation"; the `docker` client alone, including Docker Desktop's symlink, stays `no-docker` (11). Reference the probe script of §W-2.3.
5. **ENG-149:** use absolute `/usr/sbin/service docker start` (bare `service` fails under `--exec`); wait by polling `GET /_ping` (up to 30 s), not by waiting for the socket file; add the exit codes (systemctl 5/1, `service` 1 when already running), measured 1.2 s (systemd) and 0.2 s (sysv), and that `-u root --exec` needs no password. State that *Start Docker* is not offered for a Docker Desktop integration distro.
6. **ENG-152:** replace the "versions pinned at build time" wording by: Ubuntu pins exact `docker-ce`/`containerd.io` versions; Alpine records the built versions in a manifest (its repos drop old builds); hosted format `.tar.xz`; constants from §"Proposed constants".
7. **ENG-153 step 1:** replace "≥ 2 GiB free" by `download_bytes + disk_bytes + 1 GiB` per image (about 3 GiB Ubuntu, 1.5 GiB Alpine). **Step 4:** add that the **parent** of `<dir>` must exist (create `%LOCALAPPDATA%\Dockering\wsl`), the leaf may be absent or empty, and a stray `ext4.vhdx` there fails with `ERROR_FILE_EXISTS`. **Step 5/6:** the Alpine boot command needs an explicit `PATH`; dockerd answers ~1.4 s after the first call (Ubuntu ~3.5 s); the first `docker version` right after boot can fail, so poll `/_ping` (measured worst case 25 s on a bad run, so keep the 60 s budget).
8. **ENG-154/155:** add the W-4 rule: killing the importer left no state in 7 early-kill attempts and a complete usable distro in the late ones; cleanup = `wsl --unregister dockering-engine` (accept `WSL_E_DISTRO_NOT_FOUND`) only for a run that called `--import`. `--unregister` also removes the Start Menu shortcut and Terminal profile.
9. **ENG-156:** map messages by the stable `Error code: Wsl/…` token: `…/ERROR_ALREADY_EXISTS`, `…/ERROR_FILE_EXISTS`, `…/WSL_E_IMPORT_FAILED`, `Wsl/ERROR_FILE_NOT_FOUND`, `Wsl/ERROR_PATH_NOT_FOUND`, `…/E_INVALIDARG`, `…/WSL_E_DISTRO_NOT_FOUND`, and the two "optional component"/"virtualization" texts from WSL's resources. `wsl.exe` management errors exit with -1 and print UTF-16LE on stdout; `--exec` on a missing distro exits 127 (UTF-16LE on stdout).
10. **New requirement (suggest ENG-163):** the consent text and the engine page must say that the engine and its containers run only while Dockering (or another WSL session) is open: the distro stops ~15 s after the last session and takes its containers with it (§W-3.7). A boot-command process does not prevent it.
11. **New requirement (suggest ENG-164):** the image ships `/etc/wsl-distribution.conf` disabling the Start Menu shortcut and the Windows Terminal profile (§W-3.8). Verify the same file on the Ubuntu image.
12. **Acceptance (manual):** add "Docker Desktop stopped: `status` values, `start` behaviour, pipes absent" (the checklist in §W-1.5).
13. **Verification/Contract:** the setup timeouts (A8: DD 120 s, dockerd 30 s, setup 60 s) are consistent with the measurements; 30 s covers every observed start except the unexplained 22–25 s first-call latency (§W-3.4), which still fits 60 s.

### `docs/spec/20-engine-backends.md`

- **§2.1 bullet 1:** replace "(`running` seen; the other values are spike W-1)" with "(`running` seen; other values unverified, docs list none)"; add the plugin-location fact if the bullet keeps the "falls back when the plugin is missing" text.
- **§2.1 bullet 2:** `service docker start` → `/usr/sbin/service docker start`, and "(spike W-2)" → "(verified 2026-10-21, W-2)".
- **§4.2 table row 14:** reword as in item 4 above; add one line under the table that a systemd distro with `docker.socket` active is `available`.
- **§4.2 last paragraph:** confirm "wsl.exe's own error messages are UTF-16LE even with `--exec`" (observed: stdout, exit 127 for a missing distro) and add the `Error code: Wsl/…` token.
- **§4.4:** add the observed idle behaviour (stops 13–16 s after the last session; boot-command processes do not keep it alive; containers exit with 255).

---

## Cleanup confirmation

Final commands and output:

```
> wsl -l -v
  NAME              STATE           VERSION
* Ubuntu-22.04      Running         2
  docker-desktop    Running         2

> reg query "HKCU\...\Lxss" /s | findstr /i dockering     -> 0 matches (only Ubuntu-22.04 and docker-desktop keys remain)
> dir %APPDATA%\Microsoft\Windows\Start Menu | findstr dockering-spike -> 0 matches
> %TEMP%\dockering-spike (distros, dl, out, scripts)        -> removed (3.3 GB at the time of removal); %TEMP% has no dockering-spike entry
> %LOCALAPPDATA%\dockering-spike-loc                        -> removed; no dockering-spike* entry under %LOCALAPPDATA%
> docker desktop status --format json                       -> {"PauseType":"","SessionID":"4de90c9a-…","Status":"running"}  (unchanged)
```

Distros created and unregistered during the work (all `dockering-spike-*`): about 26 (alpine base/build/import-format/collision/orphan/path-location/space-path/shortcut/probe tests, ubuntu build/final/idle/kill tests). Each was removed with `wsl --unregister`; the last sweep unregistered every remaining name matching `^dockering-spike-` and then verified that only the two original distros are left. One-time download re-fetches (the Ubuntu rootfs twice, to time and to re-test) are deleted with the rest of the directory. `crates/`, `Cargo.*` and all spec files are untouched; the only file added is this report.
