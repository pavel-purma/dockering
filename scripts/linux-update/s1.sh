#!/usr/bin/env bash
# Scenarios S1-S4 of the Linux update end-to-end test (REL-085; plan linux-appimage-updates §6.5), one scenario and one leg.
# Runs INSIDE the container, as the unprivileged user, started by entry.sh; selftest.sh runs it on the host against stubs.
#   s1.sh      (the file keeps the name of the scenario it started as; E2E_SCENARIO picks the scenario)
# env (set by entry.sh or selftest.sh, never by a person):
#   E2E_SCENARIO  s1 (default) the happy path; s2 the new AppImage fails its --version self-test (the manifest names a version it
#                 does not print); s3 a read-only install directory (a Download offer, nothing is fetched); s4 the predecessor
#                 exits late (DOCKERING_UPDATE_E2E_QUIT_DELAY_SECS)
#   E2E_OLD       the OLD AppImage (updater + seam, believes it is 0.0.1)
#   E2E_NEW       the NEW AppImage, byte for byte what the fixture serves
#   E2E_FIXTURE   directory with root/ (the server root) and public-key.txt (`cargo xtask update-e2e-fixture` wrote them)
#   E2E_VERSION   the version the manifest names (and the title-bar button says): the one the NEW AppImage prints with --version,
#                 except in s2, where the manifest claims another version on purpose
#   E2E_NEW_VERSION  s2 only, optional: the version the NEW AppImage really prints (the fixture's --version)
#   E2E_LEG       extract (APPIMAGE_EXTRACT_AND_RUN=1) or fuse (a really mounted AppImage); default extract
#   E2E_OUT       evidence directory (default /work/out)
#   E2E_FIRST_CHECK_SECS  the seam's first automatic check (default 3)
#   E2E_QUIT_DELAY_SECS   s4 only: the seam's delay of the old app's exit, 2-20 (default 6)
#   E2E_WAIT_TREE/BUTTON/OLD_EXIT/NEW  limits in seconds (45, 45, 30 (s4: 30 plus the delay), 45); E2E_ALIVE (3) and E2E_SETTLE (6) are fixed waits
#   E2E_STUB=1    self-test: no display, session bus or AT-SPI; the AT-SPI helper is stub-atspi.py (see selftest.sh)
# Layout, like a user's machine: ~/.local/bin/dockering is the OLD AppImage, ~/.local/share/applications/dockering.desktop
# has that Exec, and the app is started the way the desktop entry starts it, in REAL mode (--demo disables the updater) with
# the seam variables pointing at serve.py on 127.0.0.1. Prints one `name PASS|FAIL|SKIP|INFO detail` line per check (the
# format of scripts/linux-smoke/run.sh), writes results.txt and the evidence into E2E_OUT, exits 1 on any FAIL.
# SKIP is only recorded after a FAIL that makes the check impossible (no button, so nothing to click).
# The scenarios share the bring-up and the first check (button, toast, requests); they differ in what follows:
#   s1, s4  click the button; the old app exits, the new one is relaunched as the primary instance with the saved window state
#           (s4: the old app needs E2E_QUIT_DELAY_SECS to exit, and the relaunched one has to wait for that)
#   s2      click the button; the install fails before the swap: nothing is replaced, the app keeps running, the offer is *Download*
#   s3      ~/.local/bin is made read-only; no click (Download opens a browser); a Download offer, no asset request
# Not `set -e`: a failed step is a result, not an abort (like scripts/linux-smoke/run.sh).
set -u
HERE=$(cd "$(dirname "$0")" && pwd)
: "${E2E_OLD:?set E2E_OLD}" "${E2E_NEW:?set E2E_NEW}" "${E2E_FIXTURE:?set E2E_FIXTURE}" "${E2E_VERSION:?set E2E_VERSION}"
STUB=${E2E_STUB:-}
SCENARIO=${E2E_SCENARIO:-s1}
case "$SCENARIO" in s1 | s2 | s3 | s4) ;; *) echo "s1.sh: E2E_SCENARIO must be s1, s2, s3 or s4, not '$SCENARIO'" >&2; exit 2 ;; esac
# one session bus per run: GPUI publishes its accessibility tree over AT-SPI, which lives on it
if [ -z "$STUB" ] && [ -z "${DBUS_SESSION_BUS_ADDRESS:-}" ]; then exec dbus-run-session -- bash "$0" "$@"; fi

LEG=${E2E_LEG:-extract}
VERSION=$E2E_VERSION
NEW_VERSION=${E2E_NEW_VERSION:-}
OUT=${E2E_OUT:-/work/out}
FIRST=${E2E_FIRST_CHECK_SECS:-3}
W_TREE=${E2E_WAIT_TREE:-45}
W_BUTTON=${E2E_WAIT_BUTTON:-45}
# s4: the old app's exit is delayed by the seam, and the relaunched one gives up waiting for it after 30 s (UPD-017)
QUIT_DELAY=0
if [ "$SCENARIO" = s4 ]; then
  QUIT_DELAY=${E2E_QUIT_DELAY_SECS:-6}
  if ! [[ $QUIT_DELAY =~ ^[0-9]+$ ]] || [ "$QUIT_DELAY" -lt 2 ] || [ "$QUIT_DELAY" -gt 20 ]; then
    echo "s1.sh: E2E_QUIT_DELAY_SECS must be 2-20 seconds (the relaunch wait gives up after 30 s), not '$QUIT_DELAY'" >&2
    exit 2
  fi
fi
W_OLD_EXIT=${E2E_WAIT_OLD_EXIT:-$((30 + QUIT_DELAY))}
W_NEW=${E2E_WAIT_NEW:-45}
ALIVE=${E2E_ALIVE:-3}
SETTLE=${E2E_SETTLE:-6}
if [ "$QUIT_DELAY" -gt 0 ] && [ "$W_OLD_EXIT" -le "$QUIT_DELAY" ]; then
  echo "s1.sh: E2E_WAIT_OLD_EXIT ($W_OLD_EXIT s) must exceed the quit delay ($QUIT_DELAY s): the old app could not exit in time" >&2
  exit 2
fi
UA="Dockering/0.0.1 (linux; x86_64)"
OLD_VERSION=0.0.1 # the old AppImage's build-time version (build-appimages.sh)
BINDIR="$HOME/.local/bin"
INSTALL="$BINDIR/dockering"
DESKTOP="$HOME/.local/share/applications/dockering.desktop"
DATA="$HOME/.local/share/dockering"
SOCKET="$DATA/instance.sock"
OFFER="$DATA/updates/offer.json"
REQUESTS="$OUT/requests.jsonl"
RESTART=""
case "$SCENARIO" in s1 | s4) RESTART=1 ;; esac # the scenarios in which the new AppImage replaces the old one and is relaunched
case "$LEG" in extract | fuse) ;; *) echo "s1.sh: E2E_LEG must be extract or fuse, not '$LEG'" >&2; exit 2 ;; esac

# a profile with Dockering data in it is not clean, and this script deletes and replaces files below $HOME
if [ -e "$DATA" ] || [ -e "$INSTALL" ] || [ -e "$DESKTOP" ]; then
  echo "s1.sh: refusing to run in a profile that already has Dockering files ($HOME): it needs a clean one (a container's)" >&2
  exit 2
fi
rm -rf "$OUT"
mkdir -p "$OUT"
exec > >(tee "$OUT/run.log") 2>&1
t0=$(date +%s.%N)
now() { echo "$(date +%s.%N) $t0" | awk '{printf "%.1f", $1 - $2}'; }
step() { echo "--- [$(now)s] $*"; }
RESULTS="$OUT/results.txt"
: >"$RESULTS"
record() { printf '%s\n' "$1" | tee -a "$RESULTS"; }
check() { printf '%-22s %-5s %s\n' "$1" "$2" "${3:-}" | tee -a "$RESULTS"; } # name PASS|FAIL|SKIP|INFO detail
skip() { check "$1" SKIP "$2"; }
emit() { local line; while IFS= read -r line; do record "$line"; done; } # the check lines of a helper into the results
status_of() { awk -v n="$1" '$1 == n { s = $2 } END { print s }' "$RESULTS"; }
first() { local f; for f in "$@"; do [ -e "$f" ] && { printf '%s\n' "$f"; return 0; }; done; return 1; }
in_list() { [ -n "$1" ] && case " $2 " in *" $1 "*) return 0 ;; esac; return 1; } # in_list <word> <space-separated words>

PROBE=(python3 -B "$HERE/e2e_probe.py")
if [ -n "$STUB" ]; then ATSPI=(python3 -B "$HERE/stub-atspi.py"); else ATSPI=(python3 -W ignore -B "$HERE/atspi_update.py"); fi
atspi() { timeout 180 "${ATSPI[@]}" "$@" 2>>"$OUT/atspi.err"; }
probe() { "${PROBE[@]}" "$@" 2>>"$OUT/probe.err"; }
# run a probe until it passes or <seconds> are up (state.json is saved a moment after the change), record the last answer
poll_check() {
  local limit=$1 out rc end
  shift
  end=$(($(date +%s) + limit))
  while :; do
    out=$("$@" 2>>"$OUT/probe.err")
    rc=$?
    { [ "$rc" -eq 0 ] || [ "$(date +%s)" -ge "$end" ]; } && break
    sleep 0.5
  done
  record "${out:-poll_check FAIL no output from: $*}"
  return "$rc"
}
sha() { sha256sum "$1" | cut -d ' ' -f 1; }
count_requests() { if [ -f "$REQUESTS" ]; then wc -l <"$REQUESTS" | tr -d ' '; else echo 0; fi; }
family() { probe family "$INSTALL" "$@"; }

shot() { [ -n "$STUB" ] || [ -z "${DISPLAY:-}" ] || ! command -v import >/dev/null 2>&1 || import -window root "$OUT/$1.png" 2>/dev/null; }
collect_logs() {
  local f
  : >"$OUT/dockering.log"
  for f in "$DATA"/logs/*; do [ -f "$f" ] && cat "$f" >>"$OUT/dockering.log"; done
  for f in "$DATA"/crash-*.txt; do [ -f "$f" ] && { cp "$f" "$OUT/"; crash="$f"; }; done
}
crash=""
XVFB="" BUS="" SERVE=""
cleanup() {
  local p
  chmod u+w "$BINDIR" 2>/dev/null # s3 made it read-only: the evidence copy and the removal of a profile need it writable again
  for p in $(family 2>/dev/null); do kill "$p" 2>/dev/null; done
  sleep 0.5
  for p in $(family 2>/dev/null); do kill -9 "$p" 2>/dev/null; done
  for p in $SERVE $XVFB $BUS; do kill "$p" 2>/dev/null; done
  return 0
}
trap cleanup EXIT

export LANG=C.UTF-8
# the app as a desktop session would start it; the updater is on (DOCKERING_DISABLE_UPDATES is the smoke tests' setting)
unset DOCKERING_DISABLE_UPDATES APPIMAGE APPDIR ARGV0 OWD WAYLAND_DISPLAY

# ---- display, session bus and AT-SPI (the bring-up of scripts/linux-smoke/run.sh, with HOME and XDG_RUNTIME_DIR given) ----
if [ -z "$STUB" ]; then
  : "${XDG_RUNTIME_DIR:?set XDG_RUNTIME_DIR}"
  LAUNCHER=$(first /usr/libexec/at-spi-bus-launcher /usr/lib/at-spi-bus-launcher /usr/lib/at-spi2-core/at-spi-bus-launcher) ||
    { check atspi_bus FAIL "no at-spi-bus-launcher in this image"; exit 1; }
  "$LAUNCHER" --launch-immediately >"$OUT/atspi-bus.log" 2>&1 &
  BUS=$!
  sleep 1.5
  gdbus call --session --dest org.a11y.Bus --object-path /org/a11y/bus \
    --method org.freedesktop.DBus.Properties.Set org.a11y.Status IsEnabled '<true>' >/dev/null 2>&1
  export DISPLAY=:99
  rm -f /tmp/.X99-lock /tmp/.X11-unix/X99
  Xvfb :99 -screen 0 1280x800x24 -nolisten tcp >"$OUT/xvfb.log" 2>&1 &
  XVFB=$!
  for _ in $(seq 1 100); do
    xdpyinfo >/dev/null 2>&1 && break
    kill -0 "$XVFB" 2>/dev/null || { check xvfb FAIL "Xvfb exited: $(head -c 200 "$OUT/xvfb.log")"; exit 1; }
    sleep 0.2
  done
fi

# ---- the server: the fixture over loopback, a request log and a fault switch ---------------------------------------------
python3 -B "$HERE/serve.py" --root "$E2E_FIXTURE/root" --port 0 --log "$REQUESTS" --control "$OUT/serve-control" >"$OUT/serve.out" 2>"$OUT/serve.log" &
SERVE=$!
PORT=""
for _ in $(seq 1 100); do
  PORT=$(sed -n 's/^serve\.py: listening on 127\.0\.0\.1:\([0-9]*\)$/\1/p' "$OUT/serve.out")
  [ -n "$PORT" ] && break
  kill -0 "$SERVE" 2>/dev/null || break
  sleep 0.1
done
[ -n "$PORT" ] || { check server FAIL "serve.py did not start: $(head -c 200 "$OUT/serve.log")"; exit 1; }
check server PASS "serve.py on 127.0.0.1:$PORT serves $E2E_FIXTURE/root"

# ---- the layout of a user's machine ------------------------------------------------------------------------------------
mkdir -p "$HOME/.local/bin" "$HOME/.local/share/applications"
if ! cp "$E2E_OLD" "$INSTALL" || ! chmod +x "$INSTALL"; then
  check layout FAIL "cannot install $E2E_OLD as $INSTALL (is it readable by this user?)"
  exit 1
fi
{
  echo "[Desktop Entry]"
  echo "Type=Application"
  echo "Name=Dockering"
  echo "Exec=$INSTALL"
  echo "Icon=dockering"
  echo "Terminal=false"
  echo "Categories=Development;"
  echo "StartupWMClass=dev.dockering.Dockering"
} >"$DESKTOP"
if [ "$LEG" = extract ]; then export APPIMAGE_EXTRACT_AND_RUN=1; else unset APPIMAGE_EXTRACT_AND_RUN; fi
# the command line of the desktop entry (field codes such as %U would be dropped by the launcher)
EXEC=()
while read -r word; do [[ $word =~ ^%[A-Za-z]$ ]] || EXEC+=("$word"); done < <(sed -n 's/^Exec=//p' "$DESKTOP" | head -n 1 | tr ' ' '\n')
if [ "${#EXEC[@]}" -eq 0 ] || [ "${EXEC[0]}" != "$INSTALL" ]; then
  check layout FAIL "the desktop entry's Exec is '${EXEC[*]:-}', not $INSTALL"
  exit 1
fi
if [ "$SCENARIO" = s3 ]; then
  # an install directory the user cannot write to (a package manager's, a read-only mount): the offer is a Download one (UPD-006).
  # The harness user is unprivileged, so this holds; root ignores chmod a-w, so a write is tried instead of trusting the mode
  chmod a-w "$BINDIR"
  if : >"$BINDIR/.e2e-write-probe" 2>/dev/null; then
    rm -f "$BINDIR/.e2e-write-probe"
    check layout FAIL "$BINDIR is still writable after chmod a-w (is this process root? root ignores it)"
    exit 1
  fi
  check layout PASS "$INSTALL (old AppImage), $DESKTOP; $BINDIR is not writable (mode $(stat -c %a "$BINDIR"))"
else
  check layout PASS "$INSTALL (old AppImage), $DESKTOP"
fi

# ---- s2: the premise: the new file really prints another version than the manifest names ---------------------------------
if [ "$SCENARIO" = s2 ]; then
  new_says=$(timeout 120 "$E2E_NEW" --version 2>&1 | head -n 1)
  if [ -z "$new_says" ] || [[ $new_says != "dockering "* ]]; then
    check premise FAIL "the new AppImage does not run: '--version' printed '${new_says:0:120}'"
  elif [ "$new_says" = "dockering $VERSION" ]; then
    check premise FAIL "the new AppImage prints '$new_says', the version the manifest names: the self-test would pass, this is not scenario s2"
  elif [ -n "$NEW_VERSION" ] && [ "$new_says" != "dockering $NEW_VERSION" ]; then
    check premise FAIL "the new AppImage prints '$new_says', expected 'dockering $NEW_VERSION'"
  else
    check premise PASS "the new AppImage prints '$new_says' but the manifest names $VERSION: its --version self-test must fail"
  fi
fi

# ---- start the old app like the desktop entry does -----------------------------------------------------------------------
step "launch ${EXEC[*]} ($LEG leg)"
KEY=$(tr -d '\n' <"$E2E_FIXTURE/public-key.txt")
t_start=$(date +%s.%N)
(
  cd "$HOME" || exit 1
  export DOCKERING_UPDATE_E2E_URL="http://127.0.0.1:$PORT" DOCKERING_UPDATE_E2E_KEY="$KEY" \
    DOCKERING_UPDATE_E2E_FIRST_CHECK_SECS="$FIRST" DOCKERING_UPDATE_E2E_INTERVAL_SECS=3600 RUST_LOG=info
  # s4: the seam delays the old app's exit until after run() returned and its destructors ran
  [ "$QUIT_DELAY" -eq 0 ] || export DOCKERING_UPDATE_E2E_QUIT_DELAY_SECS="$QUIT_DELAY"
  exec setsid "${EXEC[@]}" </dev/null >"$OUT/app.stdout" 2>"$OUT/app.stderr"
) &
sleep 0.3

# ---- before the first check ----------------------------------------------------------------------------------------------
step "before the first check (due $FIRST s after the start)"
atspi tree --name window --timeout "$W_TREE" | emit
if [ "$(status_of window)" = PASS ]; then
  shot 01-window
  atspi dump >"$OUT/atspi-tree-window.txt"
  n_before=$(count_requests)
  if [ "$n_before" -gt 0 ]; then
    check before_button INFO "the window came up after the first request ($n_before made): nothing to prove before the first check"
  else
    atspi absent "$VERSION" --name before_button | emit
  fi
else
  skip before_button "no accessibility tree to look in"
fi
old_pids=$(family | tr '\n' ' ')
family -v >"$OUT/processes-old.txt"
# evidence for the mounted leg: how the runtime's mount looks and where each process stands (a process whose cwd is inside
# the mount would keep it busy after the app has exited)
{ echo "== /proc/mounts (fuse)"; grep -E ' fuse' /proc/mounts || true; for p in $old_pids; do echo "== pid $p ppid=$(awk '/^PPid:/ {print $2}' "/proc/$p/status" 2>/dev/null) cwd=$(readlink "/proc/$p/cwd" 2>/dev/null)"; done; } >"$OUT/mounts-old.txt"
old_owner=$(probe owner "$SOCKET" | head -n 1)
read -ra OLD <<<"$old_pids"
if in_list "$old_owner" "$old_pids"; then
  check running PASS "old app pids: ${old_pids}; pid $old_owner holds the single-instance socket"
else
  check running FAIL "old app pids: '${old_pids}'; the single-instance socket $SOCKET is held by '${old_owner:-nobody}'"
fi
# The type 2 runtime's mount is listed with the AppImage's BASE NAME as its source and the type `fuse.<base name>` (observed
# 2026-10-20, runtime 8f39b89: `dockering /tmp/.mount_dockerXXXXXX fuse.dockering ro,...`), not with the file's path.
base=$(basename "$INSTALL")
mounted=$(awk -v b="$base" '$1 == b && $3 == "fuse." b && $2 ~ /^\/tmp\/\.mount_/ { print $2; exit }' /proc/mounts)
if [ "$LEG" = fuse ]; then
  if [ -n "$mounted" ]; then check leg PASS "fuse: the AppImage is mounted at $mounted and running from it"; else check leg FAIL "fuse: no FUSE mount of $base (see mounts-old.txt)"; fi
else
  if [ -z "$mounted" ]; then check leg PASS "extract-and-run: APPIMAGE_EXTRACT_AND_RUN=1, no FUSE mount of the AppImage"; else check leg FAIL "extract-and-run, but $base is FUSE-mounted at $mounted"; fi
fi

# ---- the first check, the button and the toast ---------------------------------------------------------------------------
# s3: the toast offers *Download* and the payload is never fetched (asset 0); the others fetch it once and offer *Restart now*
TOAST=restart ASSETS=1
if [ "$SCENARIO" = s3 ]; then TOAST=download ASSETS=0; fi
step "waiting for the update button"
atspi watch "$VERSION" --timeout "$W_BUTTON" --toast "$TOAST" | emit
shot 02-button
atspi dump >"$OUT/atspi-tree-button.txt"
sleep 1 # a stray second request would arrive about now
probe schedule "$REQUESTS" --start "$t_start" --first-check "$FIRST" | emit
probe requests "$REQUESTS" --version "$VERSION" --asset "$ASSETS" | emit
probe user_agent "$REQUESTS" --expect "$UA" | emit
poll_check 8 "${PROBE[@]}" state "$DATA/state.json" --version "$VERSION" --want toast
# what the retained offer says (UPD-017): s2 a verified install offer waiting to be applied, s3 a Download offer without a payload
case "$SCENARIO" in
  s2) poll_check 8 "${PROBE[@]}" offer "$OFFER" --version "$VERSION" --want ready --name offer_ready ;;
  s3) poll_check 8 "${PROBE[@]}" offer "$OFFER" --version "$VERSION" --want download --name offer_download ;;
esac

blocked=""
[ "$(status_of button)" = PASS ] || blocked="no update button was shown, so there was nothing to click"

# ---- click the button (the toast's Restart now: the same RestartToUpdate action); s3 has no click: Download opens a browser ----
if [ "$SCENARIO" != s3 ]; then
  if [ -z "$blocked" ]; then
    step "activate the title-bar button"
    atspi activate "$VERSION" | emit
    [ "$(status_of activate)" = PASS ] || blocked="the button could not be activated"
  else
    skip activate "$blocked"
  fi
fi
new_pids=""
if [ -n "$RESTART" ] && [ -z "$blocked" ]; then
  # the old process exits (the new one waits for that, UPD-017)
  end=$(($(date +%s) + W_OLD_EXIT))
  t_click=$(date +%s.%N)
  while :; do
    now_pids=$(family | tr '\n' ' ')
    left=""
    for p in "${OLD[@]}"; do in_list "$p" "$now_pids" && left+="$p "; done
    [ -z "$left" ] && break
    [ "$(date +%s)" -ge "$end" ] && break
    sleep 0.25
  done
  took=$(echo "$(date +%s.%N) $t_click" | awk '{printf "%.1f", $1 - $2}')
  if [ -z "$left" ]; then
    check old_exit PASS "the old app's processes (${old_pids% }) are gone ${took}s after the click"
  else
    check old_exit FAIL "still running ${W_OLD_EXIT}s after the click: ${left% }"
    # who pins the old mount: the runtime's daemon exits once nothing uses its mount any more
    oldmount=$(awk '/ fuse\./ { print $2; exit }' "$OUT/mounts-old.txt" 2>/dev/null || true)
    [ -z "$oldmount" ] || { echo "== holders of the old mount $oldmount"; probe holders "$oldmount"; } >"$OUT/old-mount-holders.txt"
  fi
  # s4: the delay was real. The seam sleeps after run() returned, and the apply and the quit take time of their own, so the old
  # app can't be gone sooner than the delay after the click (one second of slack for the click and the polling)
  if [ "$SCENARIO" = s4 ]; then
    floor=$((QUIT_DELAY - 1))
    if [ -n "$left" ]; then
      skip exit_delayed "the old app did not exit within ${W_OLD_EXIT}s"
    elif awk -v t="$took" -v n="$floor" 'BEGIN { exit !(t >= n) }'; then
      check exit_delayed PASS "the old app needed ${took}s to exit after the click (the seam delays its exit by ${QUIT_DELAY}s; at least ${floor}s expected)"
    else
      check exit_delayed FAIL "the old app was gone ${took}s after the click, expected at least ${floor}s: the exit delay (DOCKERING_UPDATE_E2E_QUIT_DELAY_SECS=$QUIT_DELAY) did not apply"
    fi
  fi

  # a new process of the app, started with --update-relaunch=1 (UPD-014)
  end=$(($(date +%s) + W_NEW))
  while :; do
    new_pids=$(family --exclude "${OLD[@]}" --arg=--update-relaunch=1 | tr '\n' ' ')
    { [ -n "$new_pids" ] || [ "$(date +%s)" -ge "$end" ]; } && break
    sleep 0.25
  done
  t_new=$(date +%s)
  if [ -n "$new_pids" ]; then
    family --exclude "${OLD[@]}" -v >"$OUT/processes-new.txt"
    { echo "== /proc/mounts (fuse)"; grep -E ' fuse' /proc/mounts || true; for p in $new_pids ${left:-}; do echo "== pid $p ppid=$(awk '/^PPid:/ {print $2}' "/proc/$p/status" 2>/dev/null) cwd=$(readlink "/proc/$p/cwd" 2>/dev/null)"; done; } >"$OUT/mounts-new.txt"
    step "new process: ${new_pids% }"
    # the notice is shown at the first paint and hides itself: look for it at once
    atspi notice "$VERSION" --timeout "$W_NEW" | emit
    shot 03-after-update
    atspi dump >"$OUT/atspi-tree-after.txt"
    # still running a moment after it came up
    rest=$((ALIVE - ($(date +%s) - t_new)))
    [ "$rest" -gt 0 ] && sleep "$rest"
    alive=$(family --exclude "${OLD[@]}" --arg=--update-relaunch=1 | tr '\n' ' ')
    if [ -n "$alive" ]; then
      check relaunch PASS "new process ${alive% } (--update-relaunch=1) is running $(($(date +%s) - t_new))s after it was first seen"
    else
      check relaunch FAIL "the new process ${new_pids% } died within $(($(date +%s) - t_new))s: $(tail -n 3 "$OUT/app.stderr" | tr '\n' ' ' | cut -c1-160)"
    fi
  else
    check relaunch FAIL "no new process started with --update-relaunch=1 within ${W_NEW}s of the old one's exit"
    skip notice "no new process"
  fi
fi

# ---- the file on disk ----------------------------------------------------------------------------------------------------
if [ -z "$RESTART" ]; then
  : # s2 and s3 never swap: what they check about the file comes after the settle, below
elif [ -z "$blocked" ]; then
  want=$(sha "$E2E_NEW")
  got=$(sha "$INSTALL")
  ver=$(timeout 120 "$INSTALL" --version 2>&1 | head -n 1)
  why=()
  [ "$got" = "$want" ] || why+=("sha256 ${got:0:12} is not the new AppImage's ${want:0:12}")
  [ -x "$INSTALL" ] || why+=("not executable")
  [ "$ver" = "dockering $VERSION" ] || why+=("--version prints '$ver', expected 'dockering $VERSION'")
  if [ ${#why[@]} -eq 0 ]; then
    check swap PASS "$INSTALL is the new AppImage byte for byte (sha256 ${got:0:12}), executable, prints '$ver'"
  else
    joined=$(printf '%s; ' "${why[@]}")
    check swap FAIL "${joined%; }"
  fi
else
  skip old_exit "$blocked"
  skip relaunch "$blocked"
  skip swap "$blocked"
  skip notice "$blocked"
  [ "$SCENARIO" != s4 ] || skip exit_delayed "$blocked"
fi

# ---- the new instance ----------------------------------------------------------------------------------------------------
if [ -z "$RESTART" ]; then
  : # s2 and s3 start no new instance
elif [ -n "$new_pids" ]; then
  end=$(($(date +%s) + W_NEW))
  owner=""
  while :; do
    owner=$(probe owner "$SOCKET" | head -n 1)
    in_list "$owner" "$new_pids" && break
    [ "$(date +%s)" -ge "$end" ] && break
    sleep 0.5
  done
  if in_list "$owner" "$new_pids"; then
    check primary_instance PASS "pid $owner (a new process) holds $SOCKET"
  else
    check primary_instance FAIL "$SOCKET is held by '${owner:-nobody}', not by the new process (${new_pids% })"
  fi
  atspi absent "$VERSION" --name no_button_after --for 4 | emit
  poll_check 8 "${PROBE[@]}" state "$DATA/state.json" --version "$VERSION" --want kept
else
  for name in primary_instance no_button_after state_kept; do skip "$name" "${blocked:-no new process}"; done
fi

# ---- s2: the install fails before the swap and nothing changes --------------------------------------------------------------
if [ "$SCENARIO" = s2 ]; then
  if [ -z "$blocked" ]; then
    step "the install must fail, and the old app must go on"
    # the failure toast hides itself: look for it at once (best effort); the file on disk and the offer are the real evidence
    atspi seen "Couldn't start the update" --button "Copy details" --name failure_toast_atspi --timeout 10 | emit
    # UPD-014 (3)/UPD-017 (6): a failed self-test degrades the offer to Download for that version, for good, and removes the payload
    poll_check 15 "${PROBE[@]}" offer "$OFFER" --version "$VERSION" --want failed --name offer_degraded
    shot 03-after-failure
    atspi dump >"$OUT/atspi-tree-after.txt"
    atspi present "$VERSION" --name button_after_failure --timeout 10 | emit
  else
    for name in offer_degraded button_after_failure; do skip "$name" "$blocked"; done
  fi
fi

# ---- nothing else happens ------------------------------------------------------------------------------------------------
step "settling ${SETTLE}s: no second check"
sleep "$SETTLE"
probe requests "$REQUESTS" --version "$VERSION" --asset "$ASSETS" --name no_second_check | emit

# ---- s2, s3: the old app and its file are untouched, and nothing was relaunched -----------------------------------------------
relaunched=""
if [ "$SCENARIO" = s2 ] || [ "$SCENARIO" = s3 ]; then
  now_pids=$(family | tr '\n' ' ')
  gone=""
  for p in "${OLD[@]}"; do in_list "$p" "$now_pids" || gone+="$p "; done
  if [ -z "$gone" ]; then
    check old_alive PASS "the old app's processes (${old_pids% }) are all still running"
  else
    check old_alive FAIL "the old app's processes are gone: ${gone% } (it was running as ${old_pids% })"
  fi
  want=$(sha "$E2E_OLD")
  got=$(sha "$INSTALL")
  ver=$(timeout 120 "$INSTALL" --version 2>&1 | head -n 1)
  why=()
  [ "$got" = "$want" ] || why+=("sha256 ${got:0:12} is not the old AppImage's ${want:0:12}: the file was replaced")
  [ -x "$INSTALL" ] || why+=("not executable")
  [ "$ver" = "dockering $OLD_VERSION" ] || why+=("--version prints '$ver', expected 'dockering $OLD_VERSION'")
  if [ ${#why[@]} -eq 0 ]; then
    check install_untouched PASS "$INSTALL is still the old AppImage byte for byte (sha256 ${got:0:12}), executable, prints '$ver'"
  else
    joined=$(printf '%s; ' "${why[@]}")
    check install_untouched FAIL "${joined%; }"
  fi
  # a staged copy (.dockering-update-<n>) next to the install must not outlive a failed or refused update (UPD-014)
  listing=$(find "$BINDIR" -mindepth 1 -maxdepth 1 -printf '%f\n' | LC_ALL=C sort | tr '\n' ' ')
  if [ "${listing% }" = dockering ]; then
    check no_staging_left PASS "$BINDIR holds exactly: dockering"
  else
    check no_staging_left FAIL "$BINDIR holds '${listing% }', expected exactly 'dockering'"
  fi
  relaunched=$(family --arg=--update-relaunch=1 | tr '\n' ' ')
fi

# ---- stop, logs, crash ---------------------------------------------------------------------------------------------------
step "stop"
for p in $(family); do kill "$p" 2>/dev/null; done
for _ in $(seq 1 20); do [ -z "$(family)" ] && break; sleep 0.25; done
collect_logs
if [ -z "$RESTART" ]; then
  : # no relaunch happens: a process with --update-relaunch=1 or a relaunch line in the log would be one
elif [ -n "$new_pids" ]; then
  # s4: the relaunched process had to wait for the late predecessor: at least the delay minus 1.5 s (the old process's own
  # quit, the saves and the polling), and it never asked the old instance to focus
  if [ "$SCENARIO" = s4 ]; then HANDSHAKE_ARGS=(--min-ms $((QUIT_DELAY * 1000 - 1500))); else HANDSHAKE_ARGS=(); fi
  probe handshake "$OUT/dockering.log" "${HANDSHAKE_ARGS[@]}" | emit
else
  skip handshake "${blocked:-no new process}"
fi
if [ "$SCENARIO" = s2 ] || [ "$SCENARIO" = s3 ]; then
  relines=$(grep -c 'update relaunch:' "$OUT/dockering.log" 2>/dev/null)
  if [ -z "$relaunched" ] && [ "${relines:-0}" -eq 0 ]; then
    check no_relaunch PASS "no process was started with --update-relaunch=1 and the log has no relaunch line"
  else
    check no_relaunch FAIL "a relaunch happened: processes '${relaunched% }', ${relines:-0} relaunch log lines"
  fi
fi
if [ -z "$crash" ] && ! grep -q 'panicked' "$OUT/app.stderr" "$OUT/dockering.log" 2>/dev/null; then check no_crash PASS; else check no_crash FAIL "${crash:-panic in the output}"; fi
check log_lines INFO "$(grep -c ' ERROR ' "$OUT/dockering.log" 2>/dev/null) ERROR and $(grep -c ' WARN ' "$OUT/dockering.log" 2>/dev/null) WARN lines (a real launch without an engine logs some; read dockering.log)"

# a skipped or truncated block must not read as green: every check has to have reported, and a SKIP needs a FAIL behind it
# (per scenario: the checks every scenario has, then the ones of its own)
common="server layout window running leg before_button before_requests requests user_agent button toast no_second_check no_crash"
case "$SCENARIO" in
  s1) required="$common activate old_exit swap relaunch notice handshake primary_instance no_button_after state_kept" ;;
  s4) required="$common activate old_exit exit_delayed swap relaunch notice handshake primary_instance no_button_after state_kept" ;;
  s2) required="$common premise offer_ready activate offer_degraded button_after_failure old_alive install_untouched no_staging_left no_relaunch" ;;
  s3) required="$common offer_download old_alive install_untouched no_staging_left no_relaunch" ;;
esac
for name in $required; do
  awk -v n="$name" '$1 == n && ($2 == "PASS" || $2 == "FAIL" || $2 == "SKIP" || $2 == "INFO") { found = 1 } END { exit !found }' "$RESULTS" ||
    check "$name" FAIL "no result was recorded for this check"
done
if grep -q -E '^[^ ]+ +SKIP ' "$RESULTS" && ! grep -q -E '^[^ ]+ +FAIL ' "$RESULTS"; then check skipped FAIL "a check was skipped although nothing failed"; fi
if [ -z "$STUB" ]; then
  # shellcheck source=/dev/null
  . /etc/os-release
  check platform INFO "$PRETTY_NAME; glibc $(ldd --version | head -n 1 | awk '{print $NF}'); leg $LEG"
fi
step "done"
passed=$(awk '$2 == "PASS"' "$RESULTS" | wc -l | tr -d ' ')
failed=$(awk '$2 == "FAIL"' "$RESULTS" | wc -l | tr -d ' ')
skipped=$(awk '$2 == "SKIP"' "$RESULTS" | wc -l | tr -d ' ')
echo "== $OUT: $passed passed, $failed failed, $skipped skipped"
[ "$failed" -eq 0 ]
