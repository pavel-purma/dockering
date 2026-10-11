#!/usr/bin/env bash
# One macOS scenario of the smoke test (REL-078 and REL-079): the published .dmg, installed, started, driven, screenshotted, quit.
# Plan: docs/plan/features/linux-install-smoke-test.md
#   run.sh <demo|upgrade|quarantine> [label]
# env:  EXPECTED_VERSION  required, for example 0.2.0
#       PKG_DIR           directory holding Dockering-<arch>.dmg (default pkg)
#       OUT_DIR           parent of the result directory (default out)
#       SMOKE_BUNDLE_SEAL=warn  a bundle whose signature seal is broken (finding F9) is INFO, not FAIL
#       SMOKE_UPGRADE_FROM  version the upgrade profile was last run with (default 0.0.1)
#       SMOKE_NOKEYS=1    self-test: key presses are discarded, so keys_cmd+2, +3, +4 and +comma must FAIL (keys_cmd+1 passes:
#                         the app never left Containers)
# Writes <OUT_DIR>/macos-<arch>-<scenario>[-<label>]/ (results.txt, captioned screenshots, app log) and exits non-zero when a
# check FAILed. Runs on the macOS 3.2 system bash.
set -u
# the scenarios replace /Applications/Dockering.app, delete the Dockering profile and kill running copies
if { [ "${GITHUB_ACTIONS:-}" != true ] || [ "${RUNNER_ENVIRONMENT:-}" != github-hosted ]; } && [ "${SMOKE_ALLOW_LOCAL:-}" != 1 ]; then
  echo "run.sh: refusing to run outside a GitHub-hosted runner: it replaces /Applications/Dockering.app, deletes the Dockering profile and kills running copies. Set SMOKE_ALLOW_LOCAL=1 on a disposable machine." >&2
  exit 2
fi
case "${1:-}" in
demo | upgrade | quarantine) ;;
*)
  echo "usage: run.sh <demo|upgrade|quarantine> [label]" >&2
  exit 2
  ;;
esac
SCENARIO=$1
LABEL="${2:-}"
: "${EXPECTED_VERSION:?set EXPECTED_VERSION}"
PKG_DIR="${PKG_DIR:-pkg}"
HERE=$(cd "$(dirname "$0")" && pwd)
case "$(uname -m)" in arm64) ARCH=aarch64 ;; *) ARCH=x86_64 ;; esac
OUT="${OUT_DIR:-out}/macos-$ARCH-$SCENARIO${LABEL:+-$LABEL}"
rm -rf "$OUT"
mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)
exec > >(tee "$OUT/run.log") 2>&1
t0=$(date +%s)
now() { echo $(($(date +%s) - t0)); }
step() { echo "--- [$(now)s] $*"; }
RESULTS="$OUT/results.txt"
: >"$RESULTS"
check() { printf '%-22s %-5s %s\n' "$1" "$2" "${3:-}" | tee -a "$RESULTS"; } # name PASS|FAIL|INFO detail
NAV_MIN_PX=1500 # a page switch changes at least this many pixels; idle noise stays under 500
ASCII_DOT=$(printf '\302\267')
TAB=$(printf '\t')

# hard time limit for one command: tmo <seconds> <command...>; 124 when it was killed. The watcher closes its output so that
# `tmo ... | tail` and `$(tmo ...)` return when the command does, not when the limit expires, and it polls in one-second steps so
# that it ends by itself soon after the command does.
tmo() {
  local seconds=$1 pid watcher status
  shift
  "$@" &
  pid=$!
  (
    exec >/dev/null 2>&1
    waited=0
    while [ "$waited" -lt "$seconds" ]; do
      sleep 1
      kill -0 "$pid" 2>/dev/null || exit 0
      waited=$((waited + 1))
    done
    kill -9 "$pid" 2>/dev/null
  ) &
  watcher=$!
  wait "$pid" 2>/dev/null
  status=$?
  kill "$watcher" 2>/dev/null
  wait "$watcher" 2>/dev/null
  [ "$status" -eq 137 ] && return 124
  return "$status"
}

# ---- the tool: window list, pixel difference, colours and captions in one compiled helper ------------
TOOL="${SMOKE_TOOL:-$OUT/../smoketool}"
if [ ! -x "$TOOL" ]; then
  tmo 300 swiftc -O "$HERE/smoketool.swift" -o "$TOOL" 2>"$OUT/swiftc.err" || {
    check tool FAIL "swiftc failed: $(tail -n 1 "$OUT/swiftc.err" | cut -c1-140)"
    exit 1
  }
fi
shot() { tmo 30 screencapture -x "$1" 2>/dev/null; }
px_diff() { "$TOOL" diff "$1" "$2" 2>/dev/null; }
colours() { "$TOOL" colours "$1" 2>/dev/null; }
winlist() { "$TOOL" windows "$1" 2>/dev/null; }
# colours inside the on-screen application window only: the Dock alone has about 10 000 and the menu bar over 200, so a count of
# the whole screen would let a blank window pass. Prints the count, or nothing when there is no window.
win_colours() {
  local bounds x y w h
  bounds=$(winlist ockering | grep 'layer=0.*onscreen=true' | head -n 1 | sed -n 's/.*bounds=\([^,]*\),\([^,]*\),\([^,]*\),\([^,]*\).*/\1 \2 \3 \4/p')
  [ -n "$bounds" ] || return 1
  read -r x y w h <<<"$bounds"
  tmo 30 screencapture -x -R "$(printf '%.0f,%.0f,%.0f,%.0f' "$x" "$y" "$w" "$h")" "$OUT/.win.png" 2>/dev/null || return 1
  colours "$OUT/.win.png"
}

# captions are added on exit, so the checks see raw frames; a frame that cannot be captioned stays as taken
caption_frames() {
  local f text ok=0 raw=0 tmp="$OUT/.caption.png"
  text="macOS $(sw_vers -productVersion) $ASCII_DOT $ARCH $ASCII_DOT $SCENARIO $ASCII_DOT dockering $EXPECTED_VERSION $ASCII_DOT Metal: ${GPU:-?}"
  for f in "$OUT"/*.png; do
    [ -f "$f" ] || continue
    if tmo 30 "$TOOL" caption "$f" "$tmp" "$text" 2>/dev/null && mv -f "$tmp" "$f"; then ok=$((ok + 1)); else raw=$((raw + 1)); rm -f "$tmp"; fi
  done
  if [ $((ok + raw)) -gt 0 ]; then check captions INFO "$ok captioned, $raw left as taken"; fi
}
trap caption_frames EXIT

# ---- install the published .dmg, as a user would -------------------------------------------------------
APP=/Applications/Dockering.app
BIN="$APP/Contents/MacOS/dockering"
DMG="$PKG_DIR/Dockering-$ARCH.dmg"
step "install $DMG"
[ -f "$DMG" ] || { check install FAIL "no $DMG"; exit 1; }
# the file the fetch job verified is the file this leg installs (REL-071)
if [ "$(grep -c '' "$PKG_DIR/packages.sha256" 2>/dev/null)" = 1 ] && (cd "$PKG_DIR" && shasum -a 256 -c packages.sha256 >"$OUT/checksum.log" 2>&1); then
  check checksum PASS "$(basename "$DMG") matches packages.sha256"
else
  check checksum FAIL "$PKG_DIR/packages.sha256 is missing, does not hold exactly one entry, or does not match $(basename "$DMG")"
  exit 1
fi
MNT=$(mktemp -d)
if tmo 120 hdiutil attach "$DMG" -nobrowse -readonly -noverify -mountpoint "$MNT" >"$OUT/hdiutil.log" 2>&1; then
  check dmg_mount PASS "$(basename "$DMG")"
else
  check dmg_mount FAIL "$(tail -n 1 "$OUT/hdiutil.log" | cut -c1-140)"
  exit 1
fi
rm -rf "$APP"
if ! ditto "$MNT/Dockering.app" "$APP" 2>"$OUT/ditto.err" || [ ! -x "$BIN" ]; then
  check install FAIL "no Dockering.app in the image: $(head -c 140 "$OUT/ditto.err")"
  exit 1
fi
tmo 60 hdiutil detach "$MNT" -force >/dev/null 2>&1

ver=$(tmo 30 "$BIN" --version 2>&1 | head -n 1)
if [ "$ver" = "dockering $EXPECTED_VERSION" ]; then check version PASS "$ver"; else check version FAIL "'$ver', expected 'dockering $EXPECTED_VERSION'"; fi
PLIST="$APP/Contents/Info.plist"
bundle_id=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$PLIST" 2>/dev/null)
short=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$PLIST" 2>/dev/null)
minos=$(/usr/libexec/PlistBuddy -c 'Print :LSMinimumSystemVersion' "$PLIST" 2>/dev/null)
if [ "$bundle_id" = dev.dockering.Dockering ] && [ "$short" = "$EXPECTED_VERSION" ]; then
  check bundle PASS "$bundle_id $short, macOS $minos or newer"
else
  check bundle FAIL "identifier '$bundle_id', version '$short'"
fi
# a bundle whose signature does not cover its resources is reported by Gatekeeper as "damaged", with no Open Anyway (finding F9)
tmo 60 codesign --verify --deep --strict "$APP" >"$OUT/codesign.log" 2>&1
seal_rc=$?
seal=$(head -n 2 "$OUT/codesign.log" | tr '\n' ' ' | cut -c1-200)
if [ "$seal_rc" -eq 0 ]; then
  check bundle_seal PASS "signature valid on disk"
elif [ "$seal_rc" -eq 124 ]; then
  check bundle_seal FAIL "codesign did not answer within 60 s"
else
  case "$seal" in
  *"not signed at all"*) check bundle_seal PASS "unsigned bundle: Gatekeeper asks for approval" ;;
  *)
    seal_status=FAIL
    [ "${SMOKE_BUNDLE_SEAL:-fail}" = warn ] && seal_status=INFO
    check bundle_seal "$seal_status" "${seal:-codesign exited $seal_rc without output}"
    ;;
  esac
fi
missing=""
for f in Contents/Resources/icon.icns Contents/Resources/LICENSE Contents/Resources/THIRD_PARTY_LICENSES.html; do [ -e "$APP/$f" ] || missing="$missing $f"; done
if [ -z "$missing" ]; then check install_files PASS "icon and licences present"; else check install_files FAIL "missing:$missing"; fi
if [ "$(lipo -archs "$BIN" 2>/dev/null)" = "$(uname -m)" ]; then check architecture PASS "$(uname -m)"; else check architecture FAIL "binary has '$(lipo -archs "$BIN" 2>/dev/null)', runner is $(uname -m)"; fi
linked=$(otool -L "$BIN" 2>/dev/null | tail -n +2 | awk '{print $1}')
foreign=$(printf '%s\n' "$linked" | grep -v -E '^(/usr/lib/|/System/)' | head -n 3 | tr '\n' ' ')
if [ -z "$linked" ]; then
  check linked_libraries FAIL "otool listed no libraries"
elif [ -z "$foreign" ]; then
  check linked_libraries PASS "system libraries only"
else
  check linked_libraries FAIL "outside /usr/lib and /System: $foreign"
fi
GPU=$(tmo 60 system_profiler SPDisplaysDataType 2>/dev/null | awk -F': ' '/Chipset Model/ {print $2; exit}')
check platform INFO "$(sw_vers -productName) $(sw_vers -productVersion); $(uname -m); ${GPU:-no GPU reported}"

# ---- keys and pointer through System Events (the runner image grants osascript the permissions) ----------
front() { tmo 20 osascript -e "tell application \"System Events\" to set frontmost of (first process whose unix id is $1) to true" >/dev/null 2>&1; sleep 1; }
send() { [ "${SMOKE_NOKEYS:-}" = 1 ] || tmo 20 osascript -e "tell application \"System Events\" to keystroke \"$2\" using {command down}" >/dev/null 2>&1; }

# ---- accessibility read through System Events: <pid> <role-or-empty>; prints "role<TAB>name<TAB>selected" ----
AX_SCRIPT="$OUT/ax.applescript"
cat >"$AX_SCRIPT" <<'EOS'
on run argv
  set thePid to (item 1 of argv) as integer
  tell application "System Events"
    set ps to every process whose unix id is thePid
    if (count of ps) is 0 then return "NO PROCESS"
    set p to item 1 of ps
    try
      set els to entire contents of window 1 of p
    on error errMsg
      return "NO WINDOW " & errMsg
    end try
    set out to ""
    repeat with e in els
      set r to ""
      set nm to ""
      set sel to ""
      try
        set r to role of e
      end try
      try
        set nm to name of e
      end try
      try
        set sel to (value of attribute "AXSelected" of e) as string
      end try
      if r is "AXRow" or r is "AXRadioButton" or r is "AXTextField" or r is "AXButton" then
        if nm is not missing value and nm is not "missing value" and nm is not "" then set out to out & r & tab & nm & tab & sel & linefeed
      end if
    end repeat
    return out
  end tell
end run
EOS
ROW_SCRIPT="$OUT/row.applescript"
cat >"$ROW_SCRIPT" <<'EOS'
on run argv
  set thePid to (item 1 of argv) as integer
  set wanted to item 2 of argv
  tell application "System Events"
    set p to first process whose unix id is thePid
    set els to entire contents of window 1 of p
    repeat with e in els
      try
        if (role of e) is "AXRow" and (name of e) is wanted then
          set pp to position of e
          set ss to size of e
          return ((item 1 of pp) as string) & " " & ((item 2 of pp) as string) & " " & ((item 1 of ss) as string) & " " & ((item 2 of ss) as string)
        end if
      end try
    end repeat
    return "NOT FOUND"
  end tell
end run
EOS
ax_dump() { tmo 60 osascript "$AX_SCRIPT" "$1" 2>/dev/null; }
# page_ok <pid> <page>: prints a short reason and returns 0 when the sidebar row of <page> is selected. Only that is asserted: the
# roles of the controls on each page were not observed on a Mac yet, so they go to ax-<page>.txt for the next review (plan §10).
page_ok() {
  local pid=$1 page=$2 dump
  dump=$(ax_dump "$pid")
  [ -n "$dump" ] || { echo "no accessibility tree"; return 1; }
  printf '%s\n' "$dump" >"$OUT/ax-$page.txt"
  printf '%s\n' "$dump" | grep -q "^AXRow${TAB}${page}${TAB}true\$" || { echo "sidebar row $page is not selected"; return 1; }
  echo "$page selected"
}
# wait_page <pid> <page> [seconds]: polls until the sidebar row is selected. A deadline, not a count: one read took 90 s on a cold
# macOS 15 runner and 14 to 17 s on every read on the Intel one (probe round 2).
wait_page() {
  local end reason
  end=$(($(date +%s) + ${3:-45}))
  while :; do
    reason=$(page_ok "$1" "$2") && { echo "$reason"; return 0; }
    [ "$(date +%s)" -ge "$end" ] && break
    sleep 1
  done
  echo "$reason"
  return 1
}

# ---- profile and launch ---------------------------------------------------------------------------------
DATA="$HOME/Library/Application Support/dev.dockering.Dockering"
pkill -9 -f "Dockering.app/Contents/MacOS" 2>/dev/null
rm -rf "$DATA" "$HOME/Library/Preferences/dev.dockering.Dockering.plist" "${TMPDIR:-/tmp}"/dockering-demo-* 2>/dev/null
ARGS=""
case "$SCENARIO" in
demo) ARGS="--demo" ;;
upgrade | quarantine)
  mkdir -p "$DATA"
  printf '{"updates":{"last_run_version":"%s"}}\n' "${SMOKE_UPGRADE_FROM:-0.0.1}" >"$DATA/state.json"
  ;;
esac

launch_wait() { # <pid>: a window of layer 0 on screen within 40 s
  for _ in $(seq 1 80); do
    winlist ockering | grep -q 'layer=0.*onscreen=true' && return 0
    kill -0 "$1" 2>/dev/null || return 1
    sleep 0.5
  done
  return 1
}
wait_stable() { # two frames 1 s apart that differ by fewer than 500 px
  local limit="${1:-30}" end ae
  end=$(($(date +%s) + limit))
  rm -f "$OUT/.a.png" "$OUT/.b.png"
  shot "$OUT/.a.png"
  while [ "$(date +%s)" -lt "$end" ]; do
    sleep 1
    shot "$OUT/.b.png"
    ae=$(px_diff "$OUT/.a.png" "$OUT/.b.png")
    if [ -n "$ae" ] && [ "$ae" -lt 500 ]; then echo stable; return 0; fi
    cp "$OUT/.b.png" "$OUT/.a.png"
  done
  echo unstable
  return 1
}

if [ "$SCENARIO" = quarantine ]; then
  # what a browser download looks like to Gatekeeper
  rm -rf /tmp/quarantined && mkdir -p /tmp/quarantined
  ditto "$APP" /tmp/quarantined/Dockering.app
  xattr -w com.apple.quarantine "0081;$(printf '%x' "$(date +%s)");Safari;" /tmp/quarantined/Dockering.app
  verdict=$(tmo 30 spctl --assess --type execute --verbose=4 /tmp/quarantined/Dockering.app 2>&1 | head -n 2 | tr '\n' ' ' | cut -c1-200)
  check gatekeeper_assessment INFO "${verdict:-no output}"
  step "open the quarantined copy through LaunchServices"
  # `open` does not return while Gatekeeper's dialog holds the launch
  open -n -a /tmp/quarantined/Dockering.app >/dev/null 2>&1 &
  OPEN_PID=$!
  dialog=""
  for _ in $(seq 1 90); do
    dialog=$(winlist CoreServicesUIAgent | grep 'layer=8.*onscreen=true' | head -n 1)
    [ -n "$dialog" ] && break
    sleep 0.5
  done
  sleep 2
  shot "$OUT/01-gatekeeper-dialog.png"
  if [ -n "$dialog" ]; then
    check gatekeeper_dialog PASS "Gatekeeper's dialog is on screen"
  else
    check gatekeeper_dialog FAIL "no Gatekeeper dialog after 45 s (a signed and notarized build opens without one: update this scenario when macOS signing is enabled)"
  fi
  if winlist ockering | grep -q 'layer=0.*onscreen=true'; then check blocked_until_approved FAIL "the app opened without approval"; else check blocked_until_approved PASS "no application window while the dialog is up"; fi
  kill -9 $OPEN_PID 2>/dev/null
  pkill -9 -f quarantined/Dockering.app 2>/dev/null
  pkill -9 -f CoreServicesUIAgent 2>/dev/null
  rm -rf /tmp/quarantined
  step "done"
  echo "== $OUT: $(grep -c ' PASS ' "$RESULTS") passed, $(grep -c ' FAIL ' "$RESULTS") failed"
  [ "$(grep -c ' FAIL ' "$RESULTS")" -eq 0 ]
  exit $?
fi

step "launch $BIN $ARGS"
RUST_LOG=info "$BIN" $ARGS >"$OUT/app.stdout" 2>"$OUT/app.stderr" &
APP_PID=$!
tw=$(date +%s)
if ! launch_wait $APP_PID; then
  if kill -0 $APP_PID 2>/dev/null; then
    check window FAIL "no window after 40 s and the process is still running"
    kill -9 $APP_PID 2>/dev/null
  else
    wait $APP_PID
    check window FAIL "no window: the app exited with status $?"
  fi
  tail -n 12 "$OUT/app.stderr"
  shot "$OUT/00-no-window.png"
  echo "== $OUT: $(grep -c ' PASS ' "$RESULTS") passed, $(grep -c ' FAIL ' "$RESULTS") failed"
  exit 1
fi
check window PASS "appeared after $(($(date +%s) - tw))s"
painted=""
for _ in $(seq 1 60); do
  c=$(win_colours)
  if [ "${c:-0}" -gt 200 ]; then painted=1; break; fi
  kill -0 $APP_PID 2>/dev/null || break
  sleep 0.5
done
shot "$OUT/01-first-frame.png"
if [ -n "$painted" ]; then check first_paint PASS "$c colours in the window"; else check first_paint FAIL "no content after 30 s (${c:-0} colours in the window)"; fi
res=$(wait_stable 30)
if [ "$res" = stable ]; then check first_frame_stable PASS "after $(now)s"; else check first_frame_stable FAIL "$res after $(now)s"; fi
shot "$OUT/01-first-frame.png"
if kill -0 $APP_PID 2>/dev/null; then check alive PASS; else check alive FAIL "exited after the first frame"; fi

# ---- UI walk (demo): keys in, accessibility tree and picture out -----------------------------------------
if [ "$SCENARIO" = demo ]; then
  front $APP_PID
  if reason=$(wait_page $APP_PID Containers); then check atspi_tree PASS "$reason"; else check atspi_tree FAIL "$reason"; fi
  prev="$OUT/01-first-frame.png"
  current=Containers
  n=1
  # the last entry is Containers again, so the walk ends where it began and a discarded key shows as a FAIL on every other chord
  for entry in "2 Images" "3 Volumes" "4 Networks"; do
    key=${entry% *}
    page=${entry#* }
    n=$((n + 1))
    f="$OUT/0${n}-$(printf '%s' "$page" | tr '[:upper:]' '[:lower:]').png"
    status=FAIL
    for attempt in 1 2; do
      send $APP_PID "$key"
      if reason=$(wait_page $APP_PID "$page" 30); then
        status=PASS
        break
      fi
    done
    [ "$status" = PASS ] && [ "$attempt" -gt 1 ] && reason="$reason (second try)"
    check "keys_cmd+$key" "$status" "$reason"
    wait_stable 10 >/dev/null
    shot "$f"
    px=$(px_diff "$prev" "$f")
    cols=$(win_colours)
    if [ "$status" = PASS ] && [ "$page" != "$current" ]; then
      if [ "${px:-0}" -ge "$NAV_MIN_PX" ] && [ "${cols:-0}" -gt 200 ]; then
        check "render_cmd+$key" PASS "$px px changed, $cols colours in the window"
      else
        check "render_cmd+$key" FAIL "the tree shows $page but the frame changed ${px:-0} px (need $NAV_MIN_PX) and the window has ${cols:-0} colours"
      fi
    else
      check "px_cmd+$key" INFO "${px:-?} px changed"
    fi
    [ "$status" = PASS ] && current=$page
    prev="$f"
  done
  # Settings: Cmd+, replaces the sidebar with the Settings sections
  status=FAIL
  for attempt in 1 2; do
    send $APP_PID ","
    end=$(($(date +%s) + 30))
    while [ "$(date +%s)" -lt "$end" ]; do
      dump=$(ax_dump $APP_PID)
      if printf '%s\n' "$dump" | grep -q "^AXRow${TAB}General${TAB}true\$" && printf '%s\n' "$dump" | grep -q "^AXRow${TAB}Back to "; then
        status=PASS
        break
      fi
      sleep 1
    done
    [ "$status" = PASS ] && break
  done
  if [ "$status" = PASS ]; then check keys_cmd+comma PASS "Settings navigation shown, General selected"; else check keys_cmd+comma FAIL "Settings navigation not found"; fi
  shot "$OUT/06-settings.png"
  # Cmd+1 returns to Containers, which is also where a discarded key leaves the app: it is checked after Settings, so that
  # it cannot pass without the key
  status=FAIL
  for attempt in 1 2; do
    send $APP_PID "1"
    if reason=$(wait_page $APP_PID Containers 30); then
      status=PASS
      break
    fi
  done
  [ "$status" = PASS ] && [ "$attempt" -gt 1 ] && reason="$reason (second try)"
  check keys_cmd+1 "$status" "$reason"
  shot "$OUT/05-containers.png"
  # a sidebar row clicked with a pointer event at its accessibility position
  pos=$(tmo 60 osascript "$ROW_SCRIPT" "$APP_PID" Volumes 2>/dev/null)
  case "$pos" in
  [0-9]*)
    read -r row_x row_y row_w row_h <<<"$pos"
    px_x=$(($(printf '%.0f' "$row_x") + $(printf '%.0f' "$row_w") / 2))
    px_y=$(($(printf '%.0f' "$row_y") + $(printf '%.0f' "$row_h") / 2))
    front $APP_PID
    if [ "${SMOKE_NOKEYS:-}" != 1 ]; then tmo 20 osascript -e "tell application \"System Events\" to click at {$px_x, $px_y}" >/dev/null 2>&1; fi
    if reason=$(wait_page $APP_PID Volumes 30); then check click_Volumes PASS "$reason"; else check click_Volumes FAIL "$reason"; fi
    shot "$OUT/07-click-volumes.png"
    ;;
  *) check click_Volumes FAIL "could not locate the Volumes row: ${pos:-no answer}" ;;
  esac
fi

# ---- upgrade: the post-update notice (UPD-008) ---------------------------------------------------------------
if [ "$SCENARIO" = upgrade ]; then
  front $APP_PID
  seen=""
  end=$(($(date +%s) + 45))
  while [ "$(date +%s)" -lt "$end" ]; do
    if ax_dump $APP_PID | grep -q "^AXButton${TAB}What's new${TAB}"; then
      seen=1
      break
    fi
    sleep 1
  done
  if [ -n "$seen" ]; then check notice_whats_new PASS "What's new button shown"; else check notice_whats_new FAIL "no What's new button: the post-update notice did not appear"; fi
  shot "$OUT/02-update-notice.png"
fi

# ---- quit: demo runs exit with status 0 on Cmd+Q; a real-mode run is stopped the way the OS would ------------
step "quit"
if kill -0 $APP_PID 2>/dev/null; then
  front $APP_PID
  send $APP_PID "q"
  for _ in $(seq 1 40); do kill -0 $APP_PID 2>/dev/null || break; sleep 0.25; done
  if kill -0 $APP_PID 2>/dev/null; then
    check clean_quit FAIL "still running 10 s after Cmd+Q"
    kill -9 $APP_PID 2>/dev/null
    wait $APP_PID 2>/dev/null
  else
    wait $APP_PID
    rc=$?
    if [ $rc -eq 0 ]; then check clean_quit PASS "exit 0"; else check clean_quit FAIL "exit $rc"; fi
  fi
else
  wait $APP_PID
  check clean_quit FAIL "the app was gone before the quit command (exit $?)"
fi

# ---- state, crash and log assertions ----------------------------------------------------------------
# the real-mode profile path contains a space: paths go through find -exec, never through word splitting
if [ "$SCENARIO" = demo ]; then
  ROOT=$(find "${TMPDIR:-/tmp}" -maxdepth 1 -name 'dockering-demo-*' 2>/dev/null | head -n 1)
  STATE="$ROOT/data/state.json"
  LOG_DIR="$ROOT/logs"
  CRASH_DIR="$ROOT/data"
else
  STATE="$DATA/state.json"
  LOG_DIR="$DATA/logs"
  CRASH_DIR="$DATA"
fi
saved=$(grep -o '"last_run_version": *"[^"]*"' "${STATE:-/nonexistent}" 2>/dev/null | sed 's/.*: *"//; s/"//')
if [ "$saved" = "$EXPECTED_VERSION" ]; then check state_saved PASS "last_run_version $saved"; else check state_saved FAIL "last_run_version '${saved:-none}', expected $EXPECTED_VERSION"; fi
: >"$OUT/dockering.log"
find "$LOG_DIR" -maxdepth 1 -name 'dockering.*.log' -exec cat {} + >>"$OUT/dockering.log" 2>/dev/null
find "$CRASH_DIR" -maxdepth 1 -name 'crash-*.txt' -exec cp {} "$OUT/" \; 2>/dev/null
crashes=$(find "$OUT" -maxdepth 1 -name 'crash-*.txt' | head -n 1)
if [ -z "$crashes" ] && ! grep -q 'panicked' "$OUT/app.stderr" 2>/dev/null; then check no_crash PASS; else check no_crash FAIL "${crashes:-panic on stderr}"; fi
if grep -q 'starting version="[^"]*"' "$OUT/dockering.log"; then check log_start PASS "$(grep -o 'starting version="[^"]*"' "$OUT/dockering.log" | head -n 1)"; else check log_start FAIL "no startup line in the log (missing or empty log?)"; fi
if [ "$SCENARIO" = demo ]; then
  if grep -q 'engine connected engine=demo' "$OUT/dockering.log"; then check demo_engine PASS "engine connected engine=demo"; else check demo_engine FAIL "the demo engine never connected"; fi
fi
# known macOS noise: two duplicate system fonts that the text system skips
bad=$(grep -E ' (ERROR|WARN) ' "$OUT/dockering.log" | grep -v -E ' WARN gpui_macos::text_system: skipping duplicate font "System Font (Bold|Regular Italic Bold Italic)"' | head -n 5)
if [ -z "$bad" ]; then check log_clean PASS; else
  check log_clean FAIL "$(printf '%s\n' "$bad" | head -n 1 | cut -c1-160)"
  printf '%s\n' "$bad" | cut -c1-200
fi
step "done"
echo "== $OUT: $(grep -c ' PASS ' "$RESULTS") passed, $(grep -c ' FAIL ' "$RESULTS") failed"
[ "$(grep -c ' FAIL ' "$RESULTS")" -eq 0 ]
