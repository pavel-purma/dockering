#!/usr/bin/env bash
# Build the two AppImages of the Linux update end-to-end test (REL-085, REL-086; plan linux-appimage-updates §6.5).
#
#   build-appimages.sh <out-dir> [--source current|worktree] [--arch x86_64|aarch64]
#
# Writes, into <out-dir>:
#   Dockering-old-<arch>.AppImage  the OLD one: `--features updater,update-e2e` (the test seam) and the build-time
#                                  version DOCKERING_E2E_VERSION=0.0.1, so it believes it is 0.0.1 and any release is newer
#   Dockering-<arch>.AppImage      the NEW one: built exactly like release.yml builds a release (`--features updater`,
#                                  the real workspace version, no seam, no DOCKERING_E2E_VERSION)
# Each is built the way release.yml does it, in two steps: `cargo build --release --locked -p dockering --target <triple>
# --features ...` and then `cargo xtask package --target <triple> --formats appimage --no-archive` (the release
# workflow's own `cargo xtask package --target <triple>` additionally writes the .deb and the .tar.gz; the AppImage is
# the same). `cargo xtask package` only packages the binary that is already built, so it takes no --features of its own.
# The NEW one is built last, so target/<triple>/release/dockering is a release-shaped binary afterwards.
#
# Then, on the unpackaged binaries, with scripts/release-evidence.sh (REL-084, the scan release.yml runs): the OLD one
# MUST contain the seam marker and the NEW one MUST NOT; and on the AppImages, `--version` (extract-and-run, no display):
# the OLD one MUST print `dockering 0.0.1`, the NEW one `dockering <workspace version>`. Any mismatch fails the script.
#
# Options:
#   --source current    (default) build the checked-out tree, with its uncommitted changes
#   --source worktree   build HEAD in a temporary `git worktree` with its own target directory, so uncommitted changes are left
#                       out; the worktree is removed on exit. Its default target directory is temporary too (a cold build,
#                       10-20 minutes): set CARGO_TARGET_DIR to keep the build cache between runs
#   --arch              x86_64 (default) or aarch64: the target is <arch>-unknown-linux-gnu; build on a host of that
#                       architecture
# env:  CARGO_TARGET_DIR  where cargo builds (default <source>/target)
#       NO_STRIP          linuxdeploy (inside cargo-packager) does not strip the libraries it bundles when this is set;
#                         the script sets it to 1 unless you do. Its bundled `strip` predates DT_RELR (`.relr.dyn`
#                         sections), which the libraries of a current distribution use (seen on Arch with glibc 2.44),
#                         and aborts the AppImage build with "Strip call failed". The binary itself is stripped by the
#                         release profile either way; only the bundled libraries stay larger.
#
# Prerequisites: a Linux host with the GPUI Kit build prerequisites (scripts/bootstrap.sh), the Rust toolchain of
# rust-toolchain.toml and cargo-packager 0.11.8 (`cargo install cargo-packager --locked --version 0.11.8`, spec 50).
# cargo-packager downloads its AppImage tooling (linuxdeploy) on first use; this script runs it with
# APPIMAGE_EXTRACT_AND_RUN=1 so that it works without FUSE. A cold build takes 10-20 minutes and several GB below the
# target directory: do not point it at a small tmpfs such as /tmp.
#
# THREE WAYS to get the two AppImages for scripts/linux-update (plan §6.5), because an AppImage needs at least the glibc of
# the machine that built it, and a locally built one may therefore fail in an Ubuntu 24.04 container with GLIBC_x.y errors
# (plan §3 decision 14):
#   1. The CI artifact (no local build): the AppImages of a `linux-update-e2e.yml` run, built on the Ubuntu 24.04 runner:
#        gh run download <run-id> --repo pavel-purma/dockering --name <artifact> --dir <out-dir>
#   2. A build INSIDE an Ubuntu 24.04 container (what `scripts/linux-update/run.sh --build` does): run this script in
#      an `ubuntu:24.04` container with the bootstrap prerequisites, Rust and cargo-packager installed, and the source
#      mounted. The result links against that image's glibc and runs in the container the scenarios use.
#   3. An Arch image: when the host is Arch (a newer glibc than Ubuntu's), build here and run the scenarios in an Arch
#      container too, so that the AppImage and the image have the same glibc.
set -euo pipefail

usage() {
  cat <<'EOF'
usage: build-appimages.sh <out-dir> [--source current|worktree] [--arch x86_64|aarch64]

Builds Dockering-old-<arch>.AppImage (updater + the update-e2e test seam, version 0.0.1) and Dockering-<arch>.AppImage
(built like a release) into <out-dir>. See the header of this script for the three ways to get the AppImages.
EOF
}
bad() {
  echo "build-appimages.sh: $*" >&2
  usage >&2
  exit 2
}

out="" source_kind=current arch=x86_64
while [ $# -gt 0 ]; do
  case $1 in
    -h | --help)
      usage
      exit 0
      ;;
    --source | --arch)
      [ $# -ge 2 ] || bad "$1 needs a value"
      if [ "$1" = --source ]; then source_kind=$2; else arch=$2; fi
      shift 2
      ;;
    --source=*)
      source_kind=${1#--source=}
      shift
      ;;
    --arch=*)
      arch=${1#--arch=}
      shift
      ;;
    -*) bad "unknown option '$1'" ;;
    *)
      [ -z "$out" ] || bad "unexpected argument '$1' (the output directory is already '$out')"
      out=$1
      shift
      ;;
  esac
done
[ -n "$out" ] || bad "the output directory is required"
case $source_kind in current | worktree) ;; *) bad "--source must be current or worktree, not '$source_kind'" ;; esac
case $arch in x86_64 | aarch64) ;; *) bad "--arch must be x86_64 or aarch64, not '$arch'" ;; esac
triple="$arch-unknown-linux-gnu"
OLD_VERSION=0.0.1

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repo=$(cd "$here/../.." && pwd)
[ ! -e "$out" ] || [ -d "$out" ] || bad "'$out' exists and is not a directory"
mkdir -p "$out"
out=$(cd "$out" && pwd)

scratch=""
worktree=""
cleanup() {
  local status=$?
  trap - EXIT
  if [ -n "$worktree" ]; then
    git -C "$repo" worktree remove --force "$worktree" >&2 || echo "build-appimages.sh: could not remove the worktree $worktree" >&2
  fi
  if [ -n "$scratch" ]; then rm -rf "$scratch"; fi
  exit "$status"
}
trap cleanup EXIT
scratch=$(mktemp -d "${TMPDIR:-/tmp}/dockering-appimages.XXXXXX")

if [ "$source_kind" = worktree ]; then
  git -C "$repo" worktree add --detach "$scratch/src" HEAD >&2
  worktree="$scratch/src"
  src="$worktree"
  export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$scratch/target}"
else
  src="$repo"
  export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$src/target}"
fi
[ -f "$src/Cargo.toml" ] || { echo "build-appimages.sh: no Cargo.toml in $src" >&2; exit 1; }

# the version of the NEW AppImage: [workspace.package] of the tree that is built
version=$(awk '/^\[workspace\.package\]/ { in_table = 1; next } /^\[/ { in_table = 0 } in_table && $1 == "version" { gsub(/"/, "", $3); print $3; exit }' "$src/Cargo.toml")
[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+([-][0-9A-Za-z.-]+)?$ ]] || { echo "build-appimages.sh: no workspace version in $src/Cargo.toml (found '$version')" >&2; exit 1; }

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d ' ' -f 1; else shasum -a 256 "$1" | cut -d ' ' -f 1; fi
}

# build <name> <cargo features> <old|new>: cargo build + cargo xtask package, then the AppImage is copied to <out-dir>/<name>
build() {
  local name=$1 features=$2 which=$3 packaged="$CARGO_TARGET_DIR/dist/$triple/Dockering-$arch.AppImage"
  echo "build-appimages.sh: [$which] cargo build --release --features $features" >&2
  if [ "$which" = old ]; then
    (cd "$src" && DOCKERING_E2E_VERSION=$OLD_VERSION cargo build --release --locked -p dockering --target "$triple" --features "$features")
  else
    # a variable of the caller's environment must not make the release-shaped build a seam build
    (cd "$src" && env -u DOCKERING_E2E_VERSION cargo build --release --locked -p dockering --target "$triple" --features "$features")
  fi
  # the unpackaged binary is what release.yml scans (REL-084): the seam must be in the OLD one and nowhere else
  bash "$here/../release-evidence.sh" scan "$CARGO_TARGET_DIR/$triple/release/dockering" "$scratch/evidence-$which.txt" >&2
  local expected_seam=false
  if [ "$which" = old ]; then expected_seam=true; fi
  if ! grep -qx "seam=$expected_seam" "$scratch/evidence-$which.txt" || ! grep -qx "updater=true" "$scratch/evidence-$which.txt"; then
    echo "build-appimages.sh: the $which binary has the wrong markers (wanted updater=true, seam=$expected_seam):" >&2
    cat "$scratch/evidence-$which.txt" >&2
    exit 1
  fi
  echo "build-appimages.sh: [$which] cargo xtask package --target $triple --formats appimage --no-archive" >&2
  rm -f "$packaged"
  (cd "$src" && APPIMAGE_EXTRACT_AND_RUN=1 NO_STRIP="${NO_STRIP-1}" cargo xtask package --target "$triple" --formats appimage --no-archive) >&2
  [ -s "$packaged" ] || { echo "build-appimages.sh: cargo xtask package did not write $packaged" >&2; exit 1; }
  cp -f "$packaged" "$out/$name"
  chmod +x "$out/$name"
}

# check_version <file> <expected version>: `--version` of the AppImage, with no display and without FUSE
check_version() {
  local file=$1 want="dockering $2" got
  if command -v timeout >/dev/null 2>&1; then
    got=$(APPIMAGE_EXTRACT_AND_RUN=1 timeout 120 "$file" --version 2>&1 | head -n 1) || true
  else
    got=$(APPIMAGE_EXTRACT_AND_RUN=1 "$file" --version 2>&1 | head -n 1) || true
  fi
  if [ "$got" != "$want" ]; then
    echo "build-appimages.sh: $(basename "$file") printed '$got', expected '$want'" >&2
    exit 1
  fi
  echo "build-appimages.sh: $(basename "$file"): $got" >&2
}

old_name="Dockering-old-$arch.AppImage"
new_name="Dockering-$arch.AppImage"
build "$old_name" updater,update-e2e old
build "$new_name" updater new
check_version "$out/$old_name" "$OLD_VERSION"
check_version "$out/$new_name" "$version"

printf 'old  %s  sha256 %s  (dockering %s, seam)\n' "$out/$old_name" "$(sha256 "$out/$old_name")" "$OLD_VERSION"
printf 'new  %s  sha256 %s  (dockering %s, release build)\n' "$out/$new_name" "$(sha256 "$out/$new_name")" "$version"
