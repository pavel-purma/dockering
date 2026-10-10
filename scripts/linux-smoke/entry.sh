#!/usr/bin/env bash
# One fresh container is one leg: install the package like a user, add the headless harness, run every scenario.
# Plan: docs/plan/features/linux-install-smoke-test.md (REL-070..077). Started by leg.sh.
#   entry.sh <deb|tar>        deb: Ubuntu (apt resolves the declared Depends); tar: Fedora and Arch
# env:  EXPECTED_VERSION  required, for example 0.2.0
#       PKG_DIR           read-only mount holding the three packages (default /pkg)
#       LEG               unique name of this leg (default: the distro id), names the contact sheet
# mounts: /scripts (this directory, read-only) and /out (writable, kept as the job artifact)
set -u
KIND="${1:?deb|tar}"
: "${EXPECTED_VERSION:?set EXPECTED_VERSION}"
export PKG_DIR="${PKG_DIR:-/pkg}" EXPECTED_VERSION
# the distro's file, outside this repo
# shellcheck source=/dev/null
. /etc/os-release
LEG="${LEG:-$ID}"
export LANG=C.UTF-8 DEBIAN_FRONTEND=noninteractive
t0=$(date +%s)
at() { echo $(($(date +%s) - t0)); }
die() { echo "SETUP FAILED: $*"; exit 3; }
retry() { local n=0; until "$@"; do n=$((n + 1)); [ "$n" -ge 3 ] && return 1; sleep 5; done; }

# runtime libraries first (the .deb's Depends, in each distro's names), then the harness tools
case "$ID" in
ubuntu)
  retry apt-get update -qq || die "apt-get update"
  if [ "$KIND" = deb ]; then
    # declared Depends only, so a missing dependency fails here
    apt-get install -y -qq --no-install-recommends "$PKG_DIR/Dockering-$(uname -m).deb" >/tmp/apt.log 2>&1 ||
      { tail -n 20 /tmp/apt.log; die "apt install of the .deb"; }
  else
    retry apt-get install -y -qq --no-install-recommends libvulkan1 libwayland-client0 libxkbcommon0 libxkbcommon-x11-0 \
      libx11-xcb1 libxcb1 libfontconfig1 libfreetype6 libzstd1 >/tmp/apt.log 2>&1 || { tail -n 20 /tmp/apt.log; die "apt runtime libraries"; }
  fi
  retry apt-get install -y -qq --no-install-recommends mesa-vulkan-drivers >>/tmp/apt.log 2>&1 || die "apt mesa-vulkan-drivers"
  retry apt-get install -y -qq --no-install-recommends xvfb xauth xdotool imagemagick x11-utils sway grim wtype \
    dbus dbus-daemon at-spi2-core python3-gi gir1.2-atspi-2.0 libglib2.0-bin \
    vulkan-tools desktop-file-utils binutils fonts-dejavu-core procps ca-certificates >>/tmp/apt.log 2>&1 || { tail -n 20 /tmp/apt.log; die "apt harness"; }
  ;;
fedora)
  retry dnf -y -q install vulkan-loader mesa-vulkan-drivers libwayland-client libxkbcommon libxkbcommon-x11 libX11-xcb \
    libxcb fontconfig freetype libzstd tar gzip >/tmp/dnf.log 2>&1 || { tail -n 20 /tmp/dnf.log; die "dnf runtime libraries"; }
  retry dnf -y -q install xorg-x11-server-Xvfb xorg-x11-xauth xdotool ImageMagick xdpyinfo xprop xwininfo xkbcomp \
    xkeyboard-config sway grim wtype dbus-daemon at-spi2-core python3-gobject gobject-introspection glib2 \
    vulkan-tools desktop-file-utils dejavu-sans-fonts dejavu-sans-mono-fonts procps-ng diffutils >>/tmp/dnf.log 2>&1 ||
    { tail -n 20 /tmp/dnf.log; die "dnf harness"; }
  ;;
arch)
  retry pacman -Syu --noconfirm --needed vulkan-icd-loader vulkan-swrast wayland libxkbcommon libxkbcommon-x11 libxcb \
    libx11 fontconfig freetype2 zstd xorg-server-xvfb xorg-xauth xdotool imagemagick xorg-xdpyinfo xorg-xwininfo \
    xorg-xprop xorg-xkbcomp xkeyboard-config sway grim wtype dbus at-spi2-core python-gobject glib2 \
    vulkan-tools desktop-file-utils ttf-dejavu procps-ng diffutils >/tmp/pacman.log 2>&1 || { tail -n 20 /tmp/pacman.log; die "pacman"; }
  ;;
*) die "unsupported distro $ID" ;;
esac
echo "== $PRETTY_NAME: setup done after $(at)s"
# selftest.sh installs once and then runs its own scenarios; leg.sh never passes this variable on
[ "${SMOKE_SETUP_ONLY:-}" = 1 ] && exit 0
{
  echo "$PRETTY_NAME"
  ldd --version | head -n 1
  vulkaninfo --summary 2>/dev/null | grep -E 'deviceName|driverInfo' | head -n 2
} >"/out/platform-$ID.txt" 2>&1

rc=0
run() {
  bash /scripts/run.sh "$@" >"/out/console-$ID-$1-$2-$3.txt" 2>&1
  local r=$?
  echo "$ID | $1 $2 $3 | rc=$r | $(grep -E '^== ' "/out/console-$ID-$1-$2-$3.txt" | sed 's#.*: ##') | t=$(at)s"
  [ $r -eq 0 ] || rc=1
}
run "$KIND" demo x11
run "$KIND" demo wayland
run "$KIND" upgrade x11
run appimage demo x11

# one contact sheet per leg, titled with the platform; cosmetic, never fails the leg
(
  cd /out || exit 0
  set -- ./"$ID"-*/01-first-frame.png
  [ -f "$1" ] || exit 0
  args=()
  for f in "$@"; do args+=(-label "$(basename "$(dirname "$f")" | sed "s/^$ID-//")" "$f"); done
  dot=$(printf '\xc2\xb7') # U+00B7 as bytes, so this file stays ASCII
  title=$(printf '%s' "${PRETTY_NAME%% (*} $dot dockering $EXPECTED_VERSION" | tr -d '\\%@')
  if command -v magick >/dev/null 2>&1; then mont=(magick montage); else mont=(montage); fi
  "${mont[@]}" -font DejaVu-Sans -pointsize 18 -title "$title" "${args[@]}" -tile 2x -geometry 640x400+6+6 -background '#f4f4f6' "contact-$LEG.png"
) 2>/dev/null || true

echo "== $PRETTY_NAME: all scenarios done after $(at)s (rc=$rc)"
exit $rc
