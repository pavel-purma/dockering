#!/usr/bin/env bash
# One fresh container is one scenario on one leg of the Linux update end-to-end test: install the harness tools, make an
# unprivileged user and run the scenario as that user. Plan: docs/plan/features/linux-appimage-updates.md §6.5 (REL-085).
# Started by run.sh.
#   entry.sh <scenario>        s1 | s2 | s3 | s4   (all four run scripts/linux-update/s1.sh with E2E_SCENARIO set)
# env:  E2E_LEG       extract | fuse (default extract)
#       E2E_VERSION   required, the version the manifest names: the NEW AppImage's, for example 0.3.0 (s2: a claimed one)
#       E2E_NEW_VERSION  s2: what the NEW AppImage really prints
#       E2E_SCENARIO  optional; when set it must equal the argument
#       E2E_FIRST_CHECK_SECS, E2E_QUIT_DELAY_SECS, E2E_WAIT_*  optional, forwarded to the scenario
#       HOST_UID, HOST_GID  the owner of the files in /out when the container is done (default: leave them as they are)
# mounts: /scripts (this directory, read-only), /in/old.AppImage and /in/new.AppImage (read-only), /fixture/root and
#         /fixture/public-key.txt (read-only: `cargo xtask update-e2e-fixture` ran on the host), /out (writable evidence)
# Why a user: the smoke test runs as root, and root ignores chmod a-w, which scenario S3 (a read-only install directory) needs.
# The app runs as `dk` with HOME=/home/dk and nothing else in its environment but what a desktop session would have.
set -u
SCENARIO="${1:?scenario (s1, s2, s3 or s4)}"
: "${E2E_VERSION:?set E2E_VERSION}"
case "$SCENARIO" in s1 | s2 | s3 | s4) ;; *) echo "entry.sh: unknown scenario '$SCENARIO'" >&2; exit 2 ;; esac
[ -z "${E2E_SCENARIO:-}" ] || [ "$E2E_SCENARIO" = "$SCENARIO" ] || { echo "entry.sh: E2E_SCENARIO=$E2E_SCENARIO disagrees with the scenario argument '$SCENARIO'" >&2; exit 2; }
# the distro's file, outside this repo
# shellcheck source=/dev/null
. /etc/os-release
export LANG=C.UTF-8 DEBIAN_FRONTEND=noninteractive
t0=$(date +%s)
at() { echo $(($(date +%s) - t0)); }
die() { echo "SETUP FAILED: $*"; exit 3; }
retry() { local n=0; until "$@"; do n=$((n + 1)); [ "$n" -ge 3 ] && return 1; sleep 15; done; }

# the runtime libraries of the AppImage (the README lists them), FUSE for the mounted leg, and the harness: a virtual display,
# a session bus, AT-SPI with its Python bindings, a software Vulkan driver, ImageMagick for screenshots
case "$ID" in
ubuntu)
  retry apt-get update -qq || die "apt-get update"
  retry apt-get install -y -qq --no-install-recommends libvulkan1 libwayland-client0 libxkbcommon0 libxkbcommon-x11-0 \
    libx11-xcb1 libxcb1 libfontconfig1 libfreetype6 libzstd1 fuse3 mesa-vulkan-drivers xvfb xauth imagemagick x11-utils \
    dbus dbus-daemon at-spi2-core python3 python3-gi gir1.2-atspi-2.0 libglib2.0-bin fonts-dejavu-core procps util-linux \
    ca-certificates >/tmp/apt.log 2>&1 || { tail -n 20 /tmp/apt.log; die "apt packages"; }
  # an older AppImage runtime links libfuse2 dynamically (24.04 names it libfuse2t64); the current one is static
  apt-get install -y -qq --no-install-recommends libfuse2t64 >>/tmp/apt.log 2>&1 || apt-get install -y -qq --no-install-recommends libfuse2 >>/tmp/apt.log 2>&1 || true
  ;;
arch)
  retry pacman -Syu --noconfirm --needed vulkan-icd-loader vulkan-swrast wayland libxkbcommon libxkbcommon-x11 libxcb libx11 \
    fontconfig freetype2 zstd fuse3 xorg-server-xvfb xorg-xauth imagemagick xorg-xdpyinfo xorg-xprop xorg-xwininfo xorg-xkbcomp \
    xkeyboard-config dbus at-spi2-core python python-gobject glib2 ttf-dejavu procps-ng util-linux >/tmp/pacman.log 2>&1 ||
    { tail -n 20 /tmp/pacman.log; die "pacman packages"; }
  ;;
*) die "unsupported distro $ID (the images are ubuntu and archlinux)" ;;
esac
echo "== $PRETTY_NAME: setup done after $(at)s"

for f in /in/old.AppImage /in/new.AppImage /fixture/root /fixture/public-key.txt; do [ -e "$f" ] || die "$f is not mounted"; done
useradd -m -s /bin/bash dk || die "useradd dk"
uid=$(id -u dk)
runtime=/run/user/$uid
mkdir -p "$runtime" /work
chown dk: "$runtime" /work
chmod 700 "$runtime"

# the owner of the evidence: files made by root in the bind mount would otherwise be root's on the host
# shellcheck disable=SC2317,SC2329 # runs from the EXIT trap (older shellchecks say SC2317, newer ones SC2329)
finish() {
  local rc=$?
  cp -a /work/out/. /out/ 2>/dev/null
  if [ -n "${HOST_UID:-}" ]; then chown -R "${HOST_UID}:${HOST_GID:-$HOST_UID}" /out 2>/dev/null; fi
  exit "$rc"
}
trap finish EXIT

echo "== $PRETTY_NAME: glibc $(ldd --version | head -n 1 | awk '{print $NF}'); leg ${E2E_LEG:-extract}; running $SCENARIO as dk (uid $uid) after $(at)s"
env_args=()
for name in E2E_FIRST_CHECK_SECS E2E_QUIT_DELAY_SECS E2E_NEW_VERSION E2E_WAIT_TREE E2E_WAIT_BUTTON E2E_WAIT_OLD_EXIT E2E_WAIT_NEW E2E_ALIVE E2E_SETTLE; do
  [ -z "${!name:-}" ] || env_args+=("$name=${!name}")
done
runuser -u dk -- env -i HOME=/home/dk USER=dk LOGNAME=dk SHELL=/bin/bash PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
  LANG=C.UTF-8 XDG_RUNTIME_DIR="$runtime" \
  E2E_SCENARIO="$SCENARIO" E2E_OLD=/in/old.AppImage E2E_NEW=/in/new.AppImage E2E_FIXTURE=/fixture E2E_VERSION="$E2E_VERSION" \
  E2E_LEG="${E2E_LEG:-extract}" E2E_OUT=/work/out "${env_args[@]}" \
  bash "/scripts/s1.sh"
rc=$?
echo "== $PRETTY_NAME: $SCENARIO (${E2E_LEG:-extract}) finished with rc=$rc after $(at)s"
exit "$rc"
