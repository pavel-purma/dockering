#!/usr/bin/env bash
# Self-test of the Linux smoke test (plan task 3; REL-072..075): the harness must FAIL when it should.
# One fresh container installs the .deb like a leg does, then runs run.sh against deliberately broken inputs. Each control
# names the checks that must FAIL; any other FAIL, a missing FAIL or a zero exit status is a failed control. A harness
# that cannot fail would pass every release, including the withdrawn 0.2.0 build (finding F3).
# Plan: docs/plan/features/linux-install-smoke-test.md. Started by selftest.sh through leg.sh (SMOKE_ENTRY=selftest-entry.sh).
#   selftest-entry.sh deb
# env:  EXPECTED_VERSION     required, for example 0.2.0
#       PKG_DIR              read-only mount holding the three packages (default /pkg)
#       SMOKE_SELFTEST_ONLY  space-separated control names to run (default: all)
# mounts: /scripts (read-only) and /out (writable, kept as the job artifact)
# Prints one `name | PASS|FAIL | detail` line per control and `== selftest: N passed, M failed`; exits non-zero when a
# control did not behave.
set -u
KIND="${1:?deb}"
[ "$KIND" = deb ] || { echo "selftest-entry.sh: only the deb leg is supported, not '$KIND'"; exit 2; }
: "${EXPECTED_VERSION:?set EXPECTED_VERSION}"
export PKG_DIR="${PKG_DIR:-/pkg}" EXPECTED_VERSION
HERE=$(cd "$(dirname "$0")" && pwd)
# the distro's file, outside this repo
# shellcheck source=/dev/null
. /etc/os-release
[ "$ID" = ubuntu ] || { echo "selftest-entry.sh: needs an Ubuntu image, not $ID"; exit 2; }
# the published 0.2.0 .deb declares plain libc6, so a missing glibc floor is INFO unless a control says otherwise
export LEG=selftest SMOKE_LIBC_FLOOR=warn LANG=C.UTF-8 DEBIAN_FRONTEND=noninteractive
DEB="$PKG_DIR/Dockering-$(uname -m).deb"
t0=$(date +%s)
at() { echo $(($(date +%s) - t0)); }

# ---- one install, then controls ---------------------------------------------------------------------------
SMOKE_SETUP_ONLY=1 bash "$HERE/entry.sh" deb || { echo "SETUP FAILED: entry.sh deb (exit $?)"; exit 3; }
[ -x /usr/bin/dockering ] || { echo "SETUP FAILED: the .deb did not install /usr/bin/dockering"; exit 3; }
echo "== selftest on $PRETTY_NAME: setup done after $(at)s"

STUBS=/tmp/selftest-stubs
mkdir -p "$STUBS"
# a package directory whose AppImage is the stub, which run.sh then launches in the mode named by SMOKE_STUB
stub_pkg() {
  mkdir -p "$STUBS/pkg-$1"
  cp "$HERE/stub-app.sh" "$STUBS/pkg-$1/Dockering-$(uname -m).AppImage"
  chmod +x "$STUBS/pkg-$1/Dockering-$(uname -m).AppImage"
  echo "$STUBS/pkg-$1"
}

# ---- bookkeeping ------------------------------------------------------------------------------------------
passed=0
failed=0
KNOWN=" "
LAST_RESULTS=""
selected() { [ -z "${SMOKE_SELFTEST_ONLY:-}" ] || [[ " ${SMOKE_SELFTEST_ONLY} " == *" $1 "* ]]; }
# the FAILed check names of a results file, in byte order, on one line
fails_of() { awk '$2 == "FAIL" { print $1 }' "$1" 2>/dev/null | LC_ALL=C sort | tr '\n' ' ' | sed 's/ $//'; }
# the same for a space-separated list
sorted() { tr ' ' '\n' <<<"$1" | grep -v '^$' | LC_ALL=C sort | tr '\n' ' ' | sed 's/ $//'; }
verdict() { # <name> <0 ok | 1 not ok> <detail>
  if [ "$2" -eq 0 ]; then
    passed=$((passed + 1))
    printf '%-44s | PASS | %s\n' "$1" "$3"
  else
    failed=$((failed + 1))
    printf '%-44s | FAIL | %s\n' "$1" "$3"
  fi
}

# control <name> <checks that must FAIL, or "" for none> <kind> <scenario> <display> [VAR=value...]
# runs run.sh with the variables and demands exactly those FAILs and a non-zero exit status (zero when none are expected)
control() {
  local name=$1 kind=$3 scenario=$4 display=$5 expect label results got rc start
  local -a vars=("${@:6}")
  KNOWN+="$name "
  selected "$name" || return 0
  expect=$(sorted "$2")
  label="ctl-$name"
  start=$(date +%s)
  env "${vars[@]}" bash "$HERE/run.sh" "$kind" "$scenario" "$display" "$label" >"/out/console-$label.txt" 2>&1
  rc=$?
  results="/out/${ID}-$kind-$scenario-$display-$label/results.txt"
  LAST_RESULTS=$results
  got=$(fails_of "$results")
  if [ ! -f "$results" ]; then
    verdict "$name" 1 "run.sh wrote no results.txt (exit $rc)"
  elif [ -z "$expect" ]; then
    if [ -z "$got" ] && [ $rc -eq 0 ]; then
      verdict "$name" 0 "no check failed, exit 0 ($(($(date +%s) - start))s)"
    else
      verdict "$name" 1 "expected a clean run, FAILs: '${got:-none}', exit $rc"
    fi
  elif [ "$got" = "$expect" ] && [ $rc -ne 0 ]; then
    verdict "$name" 0 "failed exactly: $got ($(($(date +%s) - start))s)"
  else
    verdict "$name" 1 "expected FAIL of '$expect', got '${got:-none}', exit $rc"
  fi
}
# also <control> <what> <command...>: a further assertion about a control that ran
also() {
  local name=$1 what=$2
  shift 2
  selected "$name" || return 0
  if "$@"; then verdict "$name/$what" 0 "ok"; else verdict "$name/$what" 1 "not met"; fi
}
second_tries() { [ "$(grep -c 'second try' "$LAST_RESULTS")" -eq "$1" ]; }
no_process() { ! pgrep -f "$1" >/dev/null; }
reports() { grep -q "$1" "$LAST_RESULTS"; }

# ---- the stub itself must be able to pass ---------------------------------------------------------------------
# otherwise a control below could fail for a reason of the stub and prove nothing
control stub_baseline "" appimage upgrade x11 "PKG_DIR=$(stub_pkg ok)" SMOKE_STUB=ok

# ---- keys: discarded chords fail the page checks; the retry saves one dropped press (finding F7) -----------------
# keys_ctrl+1 passes without keys: the app is still on Containers, the page that Mod+1 selects
control no_keys "keys_ctrl+2 keys_ctrl+3 keys_ctrl+4 keys_ctrl+comma" deb demo x11 SMOKE_NOKEYS=1
control first_press_dropped "" deb demo x11 SMOKE_NOKEYS=first
also first_press_dropped five_second_tries second_tries 5

# ---- no accessibility tree: the page, key and click checks must not pass on a window alone ------------------
control app_without_accessibility_tree \
  "atspi_tree page_Containers keys_ctrl+1 keys_ctrl+2 keys_ctrl+3 keys_ctrl+4 keys_ctrl+comma click_Images click_Volumes click_Networks click_Containers" \
  appimage demo x11 "PKG_DIR=$(stub_pkg ok)" SMOKE_STUB=ok

# ---- the launch: no window (REL-074; findings F2 and F3) ----------------------------------------------------------
control app_hangs_without_window window appimage upgrade x11 "PKG_DIR=$(stub_pkg hang)" SMOKE_STUB=hang
also app_hangs_without_window leaves_no_process no_process dockering-selftest-hang
control app_exits_before_window window appimage upgrade x11 "PKG_DIR=$(stub_pkg exit)" SMOKE_STUB=exit
control wayland_app_exits_before_window window appimage upgrade wayland "PKG_DIR=$(stub_pkg exit)" SMOKE_STUB=exit
control no_graphics_driver window deb upgrade x11 VK_ICD_FILENAMES=/nonexistent __EGL_VENDOR_LIBRARY_FILENAMES=/nonexistent
mkdir -p "$STUBS/no-xvfb"
printf '#!/bin/sh\nexit 1\n' >"$STUBS/no-xvfb/Xvfb"
chmod +x "$STUBS/no-xvfb/Xvfb"
control xvfb_does_not_start xvfb deb upgrade x11 "PATH=$STUBS/no-xvfb:$PATH"

# ---- what the window looks like ------------------------------------------------------------------------------
control wrong_window_class window_identity appimage upgrade x11 "PKG_DIR=$(stub_pkg wrongclass)" SMOKE_STUB=wrongclass
control blank_window first_paint appimage upgrade x11 "PKG_DIR=$(stub_pkg blank)" SMOKE_STUB=blank
control screen_never_settles first_frame_stable appimage upgrade x11 "PKG_DIR=$(stub_pkg flicker)" SMOKE_STUB=flicker
# `compare -metric AE` prints 1.014e+06 for a count of this size: it must still read as thousands of changed pixels
control screen_strobes_fully first_frame_stable appimage upgrade x11 "PKG_DIR=$(stub_pkg strobe)" SMOKE_STUB=strobe
control app_dies_after_first_frame alive appimage upgrade x11 "PKG_DIR=$(stub_pkg mortal)" SMOKE_STUB=mortal

# ---- the exit, the saved state, the crash and log assertions (REL-075) -----------------------------------------
control exit_status_not_zero clean_quit appimage upgrade x11 "PKG_DIR=$(stub_pkg badexit)" SMOKE_STUB=badexit
control crash_file_left no_crash appimage upgrade x11 "PKG_DIR=$(stub_pkg crashfile)" SMOKE_STUB=crashfile
control panic_on_stderr no_crash appimage upgrade x11 "PKG_DIR=$(stub_pkg panic)" SMOKE_STUB=panic
control error_in_log log_clean appimage upgrade x11 "PKG_DIR=$(stub_pkg logerror)" SMOKE_STUB=logerror
control wrong_expected_version "state_saved version" deb upgrade x11 EXPECTED_VERSION=9.9.9

# ---- package defects, made in the installed copy and undone afterwards (REL-072) ------------------------------------
reinstall() { dpkg -i --force-depends "$DEB" >/dev/null 2>&1; }
DESKTOP=/usr/share/applications/dockering.desktop
ICON=/usr/share/icons/hicolor/512x512/apps/dockering.png
HIDDEN=/tmp/selftest-hidden
break_desktop_entry() { printf 'Type=Bogus\n' >>"$DESKTOP"; }
hide_library() {
  mkdir -p "$HIDDEN"
  mv /usr/lib/x86_64-linux-gnu/libxkbcommon-x11.so.0* "$HIDDEN/"
}
restore_library() { mv "$HIDDEN"/libxkbcommon-x11.so.0* /usr/lib/x86_64-linux-gnu/; }
# mutated <control> <check that must FAIL> <mutation command...>: mutate the install, run the control, reinstall
mutated() {
  local name=$1 expect=$2
  shift 2
  KNOWN+="$name "
  selected "$name" || return 0
  "$@"
  control "$name" "$expect" deb upgrade x11
  reinstall
}
mutated invalid_desktop_entry desktop_entry break_desktop_entry
mutated missing_icon install_files rm -f "$ICON"
KNOWN+="missing_library "
if selected missing_library; then
  hide_library
  control missing_library "libraries version window" deb upgrade x11
  restore_library
fi

# ---- the glibc floor of the .deb decides between FAIL, INFO and a clean pass (REL-073) -------------------------------
NEED=$(objdump -T /usr/bin/dockering 2>/dev/null | grep -o 'GLIBC_[0-9.]*' | sed 's/GLIBC_//' | sort -V | tail -n 1)
LOWER=$(awk -v v="$NEED" 'BEGIN { split(v, p, "."); if (p[2] > 0) printf "%d.%d", p[1], p[2] - 1; else printf "%d", p[1] - 1 }')
HIGHER=$(awk -v v="$NEED" 'BEGIN { split(v, p, "."); printf "%d.%d", p[1], p[2] + 1 }')
# install the .deb with the given libc6 dependency, made by repacking it (empty: plain libc6, whatever the release declares)
set_floor() {
  local work=/tmp/selftest-repack depends want="libc6"
  [ -z "$1" ] || want="libc6 ($1)"
  rm -rf "$work" /tmp/selftest-floor.deb
  dpkg-deb -R "$DEB" "$work" || return 1
  sed -i -E "/^Depends:/ s/libc6( \([^)]*\))?/$want/" "$work/DEBIAN/control"
  dpkg-deb -Zgzip -z1 -b "$work" /tmp/selftest-floor.deb >/dev/null || return 1
  depends=$(dpkg-deb -f /tmp/selftest-floor.deb Depends)
  [ "${depends%%,*}" = "$want" ] || return 1 # the repack really carries the dependency under test
  dpkg -i --force-depends /tmp/selftest-floor.deb >/dev/null 2>&1
}
# floor_control <name> <libc6 constraint or ""> <libc_floor must FAIL: libc_floor or ""> <SMOKE_LIBC_FLOOR>
floor_control() {
  KNOWN+="$1 "
  selected "$1" || return 0
  if [ -z "$NEED" ]; then
    verdict "$1" 1 "cannot read the glibc version that the binary needs"
    return 0
  fi
  if ! set_floor "$2"; then
    verdict "$1" 1 "could not install the repacked .deb"
    return 0
  fi
  control "$1" "$3" deb upgrade x11 "SMOKE_LIBC_FLOOR=$4"
}
floor_control libc_floor_below_need ">= $LOWER" libc_floor fail
floor_control libc_floor_plain "" libc_floor fail
floor_control libc_floor_equal_need ">= $NEED" "" fail
floor_control libc_floor_above_need ">= $HIGHER" "" fail
floor_control libc_floor_warn_is_info "" "" warn
also libc_floor_warn_is_info reported_as_info reports '^libc_floor  *INFO'
reinstall

# ---- the requested names must all exist ---------------------------------------------------------------------------
read -r -a wanted <<<"${SMOKE_SELFTEST_ONLY:-}"
for want in "${wanted[@]}"; do
  [[ $KNOWN == *" $want "* ]] || verdict "unknown_control/$want" 1 "no control of this name"
done
echo "== selftest: $passed passed, $failed failed (after $(at)s)"
[ "$failed" -eq 0 ] && [ "$passed" -gt 0 ]
