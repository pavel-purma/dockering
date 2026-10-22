# Engine root filesystems

The root filesystems behind **Set up Docker in WSL** (REL-088, REL-089, ENG-152, ENG-164). Dockering downloads one of
them after the user agrees, checks its SHA-256 against a constant compiled into the app, and imports it as the WSL
distro `dockering-engine`. This page is for the maintainer who builds and publishes them; the feature is specified in
[windows-engine-start](spec/features/windows-engine-start.md) and planned in
[the plan](plan/features/windows-engine-start.md).

## What is in each image

Four assets per release, one per image and CPU architecture: `dockering-engine-<alpine|ubuntu>-<x86_64|aarch64>.tar.xz`
(WSL imports `.tar.xz` directly). Every image has the `docker` group, the user `root` as the default user, no package
caches, and `/etc/dockering-engine-manifest.txt` listing the installed packages.

| | Alpine | Ubuntu |
|---|---|---|
| Base | Alpine 3.24.2 minirootfs | Canonical's Ubuntu 26.04.1 LTS WSL root filesystem |
| Docker | `docker`, `docker-engine`, `containerd`, `runc`, `socat`, `iptables`, `ip6tables` from Alpine's repositories | `docker-ce`, `docker-ce-cli`, `containerd.io` from Docker's apt repository |
| Versions | **Recorded** in the manifest (Alpine's mirrors drop old builds, so `apk add pkg=version` cannot be repeated) | **Pinned** to exact versions in `pins.toml` |
| Starts dockerd | `[boot] command` in `/etc/wsl.conf` with an explicit `PATH` and `--userland-proxy-path` (no init system) | `systemd=true` in `/etc/wsl.conf`, `docker.service`, `docker.socket` and `containerd.service` enabled |
| Measured size (xz, x86_64, 2026-10-21) | 58 994 688 bytes (59 MB) | 309 392 224 bytes (309 MB) |

Both images also carry `/etc/wsl-distribution.conf` with `[shortcut] enabled = false` and `[windowsterminal] enabled = false`,
so the import adds no Start Menu entry and no Windows Terminal profile (ENG-164). Alpine has no such file in its base,
so it is written. Ubuntu ships one for its first-run setup; the build **edits** it (keeping `[oobe]`, the icon and the
terminal template as shipped) and adds the two switches. `wsl --import` never runs the first-run setup, so the keys stay
valid and unused.

Containers run only while Dockering, or another WSL session, holds the distro open: WSL stops it about 15 seconds after the
last session (ENG-163).

## Build locally

On a Linux machine of the **same architecture** as the image (the image is assembled in a chroot; there is no emulation),
as root or with passwordless `sudo`:

```sh
ENGINE_ROOTFS_TAG=rootfs-1 bash tools/engine-rootfs/build.sh alpine x86_64 out/     # or ubuntu, aarch64
```

Needs `python3` (3.11+), `curl`, `xz`, `tar` (GNU), `gpg`, `chroot`, `mount` and network access. It downloads the pinned base
and **verifies its size and SHA-256 before unpacking anything**, installs Docker in the chroot, writes the configuration,
and produces in `out/`: the `.tar.xz`, its `.sha256`, a `.json` entry (`name`, `size`, `sha256`, `url`, `base_*`,
`packages`), a copy of the manifest, and `LICENSES-<image>-<arch>.txt`.

The same inputs give the **same bytes**: the tar is sorted, owners are numeric 0/0, times are clamped to
`source_date_epoch`, and `xz` runs with fixed settings. Check it by building twice and comparing the SHA-256. (Alpine and
Ubuntu both repeated exactly on 2026-10-21; a base or package update changes the result, as it should.)

To try an image on Windows, import it into a throwaway distro and remove it again; never use the name `dockering-engine`:

```powershell
pwsh -NoProfile -File tools/engine-rootfs/smoke.ps1 -Tar out\dockering-engine-alpine-x86_64.tar.xz -Image alpine `
  -DistroName dockering-smoke-local -WorkDir $env:TEMP\engine-rootfs-smoke-local
```

It imports, waits for `GET /_ping` (up to 60 s), runs `hello-world`, publishes a port and reads it from Windows, bind-mounts a
Windows folder, checks that no shortcut or terminal profile was added, and always unregisters the distro. It touches only the
distro it is given, whose name must start with `dockering-smoke-`.

## The pins

`tools/engine-rootfs/pins.toml` is the single source of truth: base URLs, sizes and SHA-256 of the upstream files, the Ubuntu
package versions, the Alpine package list, the fixed `source_date_epoch`, and for each of the four assets the URL, size and
SHA-256 of what a `rootfs-<n>` release published. The Rust pin table (`crates/dk-wsl/src/setup/pins.rs`) repeats the
published URL, size and SHA-256; nothing else is trusted by the app.

**Until the first release exists**, every asset is `status = "unpublished"` with a placeholder digest, and the checks run with
`--allow-unpublished`. The app then offers no image (a choice without a published asset is omitted, A13).

## Cut a `rootfs-<n>` release

Only the maintainer does this; agents never dispatch the workflow with `publish`.

1. Pick the next free number. A published `rootfs-<n>` is never changed or reused: the app pins its digests.
2. Optional rehearsal, from any branch: run **Engine rootfs** with `tag = rootfs-<n>` and `publish` off. It builds all four
   assets, smoke-tests the two x86_64 images on the self-hosted `wsl` runner, and uploads the files as 7-day artifacts.
   (The arm64 images are build-only: hosted runners have no WSL.)
3. From `main`, run it with `publish` on:
   `gh workflow run engine-rootfs.yml --ref main -f tag=rootfs-<n> -f publish=true`.
   After the builds and smoke tests it creates a **draft** release in the `release` environment, uploads the 12 files (4
   images, 4 manifests, 4 licence files) and `SHA256SUMS`, writes build provenance for them, compares every uploaded size and
   digest with the local file, and only then publishes it as a **pre-release that is not the repository's Latest release**
   (the in-app updater reads `releases/latest`, so it must never point at a rootfs).
4. Copy the `[images.*.arch.*.asset]` tables from the run's summary into `pins.toml`, set `[release] tag`, put the same URL,
   size and SHA-256 into the Rust pin table, and remove `--allow-unpublished` from `ci.yml` and `release.yml`. One PR.
5. Nothing reaches users until an app release carries those constants. The monthly scheduled run rebuilds and smoke-tests
   only (to notice upstream drift); it never publishes.

## The pin check (REL-089)

```sh
cargo xtask engine-rootfs-pin-check [--allow-unpublished] [--online]
```

- **Offline** (the `lint` job of `ci.yml`): `pins.toml` is well formed (all four cells, SHA-256 64 lowercase hex, sizes above
  zero, `https://github.com/<repo>/releases/download/rootfs-<n>/<name>` URLs) and, when `crates/dk-wsl/src/setup/pins.rs`
  exists, every published URL appears there next to the same size and SHA-256, and the table pins nothing else.
- **`--online`** (the `assemble` job of `release.yml`, before any manifest is made): the release `rootfs-<n>` exists, is not a
  draft, and every published asset has the pinned URL, size and the SHA-256 that GitHub computed for it. A release whose pinned
  asset is missing or different is not built.

A test of the check against a wrong digest is `rel_089_published_release_mismatch_is_detected` in `xtask`.

## Licences

Each release ships `LICENSES-<image>-<arch>.txt` (REL-002): every installed package with its licence, the licence texts the
packages carry, and where the source code is. Alpine's packages state SPDX expressions; Debian's copyright files name the
licences of each package, and the text of Docker's own Apache-2.0 licence is included because its packages ship none. The
images contain GPL software (BusyBox, apk-tools, iptables and others) as any Linux distribution does; the sources are the
distribution's own, listed in the file.

## Why this is safe to ship

The image runs as root inside a WSL distro, so how it is trusted matters:

- **Pinned in the app.** The URL, size and SHA-256 are constants in the signed-or-not release binary users already run; a
  download whose digest differs is deleted and never imported (ENG-152). Only the pinned HTTPS URL, on `github.com`, is fetched.
- **Built in the open.** The workflow runs on GitHub-hosted runners from this repository, from pinned base files verified by
  SHA-256, Docker's apt key checked against its fingerprint, and exact package versions for Ubuntu. Build provenance
  (`gh attestation verify <file> -R pavel-purma/dockering`) ties each asset to the workflow run.
- **Consent.** The dialog shows what will be downloaded and imported and nothing happens before the user picks an image and
  agrees (ENG-151, ENG-162). No administrator rights, no other distro touched (ENG-155).
- **Smoke-tested before publishing**, and the check above stops an app release from shipping a pin that does not exist.
