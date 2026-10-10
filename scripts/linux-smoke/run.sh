#!/usr/bin/env bash
# Scenario runner of the Linux install-and-launch smoke test (REL-070..077), run inside a leg's container.
# Plan: docs/plan/features/linux-install-smoke-test.md. Started by entry.sh once the package is installed.
#   run.sh <deb|tar|appimage> <demo|upgrade> <x11|wayland> [label]
# env:  EXPECTED_VERSION  required, for example 0.2.0
#       PKG_DIR           where the packages are mounted (default /pkg)
#       SMOKE_LIBC_FLOOR  warn: a missing .deb glibc floor is reported as INFO (default: FAIL)
#       SMOKE_NOKEYS=1    self-test: key presses are discarded, so every keys_* check must FAIL
#       SMOKE_NOKEYS=first  self-test: the first press of each chord is discarded, so each keys_* check needs its retry
# Writes /out/<distro>-<kind>-<scenario>-<display>[-label]/ (results.txt, captioned screenshots, logs)
# and exits non-zero when a check FAILed.
set -u
# one session bus per run: GPUI publishes its accessibility tree over AT-SPI, which lives on it
if [ -z "${DBUS_SESSION_BUS_ADDRESS:-}" ]; then exec dbus-run-session -- bash "$0" "$@"; fi
KIND="${1:?deb|tar|appimage}"
SCENARIO="${2:?demo|upgrade}"
DISPLAY_KIND="${3:-x11}"
LABEL="${4:-}"
: "${EXPECTED_VERSION:?set EXPECTED_VERSION}"
PKG_DIR="${PKG_DIR:-/pkg}"
HERE=$(cd "$(dirname "$0")" && pwd)
LEAD_MS=600 # Wayland: a fresh virtual keyboard needs time before the compositor routes its first key

# the distro's file, outside this repo
# shellcheck source=/dev/null
. /etc/os-release
OUT="/out/${ID}-${KIND}-${SCENARIO}-${DISPLAY_KIND}${LABEL:+-$LABEL}"
rm -rf "$OUT"
mkdir -p "$OUT"
exec > >(tee "$OUT/run.log") 2>&1
t0=$(date +%s.%N)
now() { echo "$(date +%s.%N) $t0" | awk '{printf "%.1f", $1 - $2}'; }
step() { echo "--- [$(now)s] $*"; }
RESULTS="$OUT/results.txt"
: >"$RESULTS"
check() { printf '%-22s %-5s %s\n' "$1" "$2" "${3:-}" | tee -a "$RESULTS"; } # name PASS|FAIL|INFO detail
first() { local f; for f in "$@"; do [ -e "$f" ] && { printf '%s\n' "$f"; return 0; }; done; return 1; } # first existing path

# captions are added on exit, so the checks see raw frames; a frame that cannot be captioned stays as taken
if command -v magick >/dev/null 2>&1; then IM=(magick); else IM=(convert); fi
caption_frames() {
  local pkg disp gpu v dot text f ok=0 raw=0 tmp="$OUT/.caption.png"
  case "$KIND" in deb) pkg=deb ;; tar) pkg=tar.gz ;; *) pkg=AppImage ;; esac
  case "$DISPLAY_KIND" in x11) disp=X11 ;; *) disp=Wayland ;; esac
  gpu=$(grep -o -E 'Selected GPU adapter: "[^"]*"' "$OUT/dockering.log" 2>/dev/null | tail -n 1 | sed -E 's/^[^"]*"//; s/"$//; s/ \(.*//')
  v=${ver:-dockering ?}
  dot=$(printf '\xc2\xb7') # U+00B7 as bytes, so this file stays ASCII
  text="${PRETTY_NAME%% (*} $dot $pkg $dot $disp $dot $SCENARIO $dot ${v:0:40}${gpu:+ $dot $gpu}"
  text=$(printf '%s' "$text" | tr -d '\\%@') # escapes for ImageMagick's text
  for f in "$OUT"/*.png; do
    [ -f "$f" ] || continue
    if "${IM[@]}" "$f" -depth 8 -background '#1f2430' -gravity North -splice 0x28 -gravity NorthWest \
      -font DejaVu-Sans -pointsize 15 -fill '#e6e9f0' -annotate +10+6 "$text" "$tmp" 2>/dev/null && mv -f "$tmp" "$f"; then
      ok=$((ok + 1))
    else
      rm -f "$tmp"
      raw=$((raw + 1))
    fi
  done
  if [ $((ok + raw)) -gt 0 ]; then check captions INFO "$ok captioned, $raw left as taken"; fi
}
trap caption_frames EXIT

export LANG=C.UTF-8 HOME=/root XDG_RUNTIME_DIR=/tmp/xdg-runtime
mkdir -p "$XDG_RUNTIME_DIR" && chmod 700 "$XDG_RUNTIME_DIR"
rm -rf "$HOME/.config/dockering" "$HOME/.local/share/dockering" /tmp/dockering-demo-*

# ---- the package under test ----------------------------------------------------------------------
case "$KIND" in
deb) BIN=(/usr/bin/dockering) ;;
tar)
  rm -rf /opt/dk && mkdir -p /opt/dk && tar -xzf "$PKG_DIR/Dockering-$(uname -m).tar.gz" -C /opt/dk
  BIN=("$(echo /opt/dk/dockering-*/dockering)")
  ;;
appimage)
  cp "$PKG_DIR/Dockering-$(uname -m).AppImage" /opt/Dockering.AppImage && chmod +x /opt/Dockering.AppImage
  export APPIMAGE_EXTRACT_AND_RUN=1 # a container has no /dev/fuse: unpack instead of mounting
  BIN=(/opt/Dockering.AppImage)
  ;;
esac

ver=$("${BIN[@]}" --version 2>&1 | head -n 1)
if [ "$ver" = "dockering $EXPECTED_VERSION" ]; then
  check version PASS "$ver"
else
  check version FAIL "'$ver', expected 'dockering $EXPECTED_VERSION'"
fi
if [ "$KIND" != appimage ]; then
  unresolved=$(ldd "${BIN[0]}" 2>&1 | grep -c 'not found')
  if [ "$unresolved" -eq 0 ]; then
    check libraries PASS "ldd resolves every library"
  else
    check libraries FAIL "$unresolved unresolved"
  fi
  case "$KIND" in
  deb)
    files=(/usr/bin/dockering /usr/lib/dockering/LICENSE /usr/lib/dockering/THIRD_PARTY_LICENSES.html
      /usr/share/applications/dockering.desktop /usr/share/icons/hicolor/512x512/apps/dockering.png)
    desktop=/usr/share/applications/dockering.desktop
    ;;
  tar)
    d=$(dirname "${BIN[0]}")
    desktop=$(echo "$d"/share/applications/*.desktop)
    files=("${BIN[0]}" "$d/LICENSE" "$d/THIRD_PARTY_LICENSES.html" "$desktop" "$(echo "$d"/share/icons/hicolor/512x512/apps/*.png)")
    ;;
  esac
  missing=()
  for f in "${files[@]}"; do [ -e "$f" ] || missing+=("$f"); done
  if [ ${#missing[@]} -eq 0 ]; then
    check install_files PASS "${#files[@]} files present"
  else
    check install_files FAIL "missing: ${missing[*]}"
  fi
  if desktop-file-validate "$desktop" >/dev/null 2>&1; then
    check desktop_entry PASS "$(basename "$desktop")"
  else
    check desktop_entry FAIL "$desktop"
  fi
fi
if [ "$KIND" = deb ]; then
  need=$(objdump -T "${BIN[0]}" 2>/dev/null | grep -o 'GLIBC_[0-9.]*' | sed 's/GLIBC_//' | sort -V | tail -n 1)
  floor=$(dpkg-query -W -f='${Depends}' dockering | sed -n -E 's/.*libc6 \(>= ([0-9.]+)\).*/\1/p')
  declared="plain libc6"
  [ -n "$floor" ] && declared="libc6 (>= $floor)"
  if [ -n "$need" ] && [ -n "$floor" ] && dpkg --compare-versions "$floor" ge "$need"; then
    check libc_floor PASS "$declared"
  else
    [ "${SMOKE_LIBC_FLOOR:-fail}" = warn ] && st=INFO || st=FAIL
    check libc_floor "$st" "declares $declared, the binary needs glibc ${need:-?}"
  fi
fi

# ---- scenario ------------------------------------------------------------------------------------
ARGS=()
case "$SCENARIO" in
demo) ARGS=(--demo) ;; # a populated FakeEngine: no container engine needed
upgrade)
  # a profile from an older version: the first launch shows the "Updated to" notice (UPD-008)
  mkdir -p "$HOME/.local/share/dockering"
  echo '{"updates":{"last_run_version":"0.0.1"}}' >"$HOME/.local/share/dockering/state.json"
  ;;
esac

# ---- accessibility bus (AT-SPI) -----------------------------------------------------------------
LAUNCHER=$(first /usr/libexec/at-spi-bus-launcher /usr/lib/at-spi-bus-launcher /usr/lib/at-spi2-core/at-spi-bus-launcher)
"$LAUNCHER" --launch-immediately >"$OUT/atspi-bus.log" 2>&1 &
sleep 1.5
gdbus call --session --dest org.a11y.Bus --object-path /org/a11y/bus \
  --method org.freedesktop.DBus.Properties.Set org.a11y.Status IsEnabled '<true>' >/dev/null 2>&1
# one atspi.py subcommand into ATSPI_OUT; no output counts as a FAIL
atspi_run() {
  ATSPI_OUT=$(python3 -W ignore "$HERE/atspi.py" "$@" 2>"$OUT/atspi.err")
  if [ -z "$ATSPI_OUT" ]; then
    ATSPI_OUT="atspi_$1 FAIL atspi.py printed nothing: $(tail -n 1 "$OUT/atspi.err" | cut -c1-120)"
    return 1
  fi
  ! grep -q ' FAIL ' <<<"$ATSPI_OUT"
}
record() { printf '%s\n' "$1" | tee -a "$RESULTS"; }

# ---- display server and input ---------------------------------------------------------------------
if [ "$DISPLAY_KIND" = x11 ]; then
  export DISPLAY=:99
  unset WAYLAND_DISPLAY
  rm -f /tmp/.X99-lock /tmp/.X11-unix/X99
  Xvfb :99 -screen 0 1280x800x24 -nolisten tcp >"$OUT/xvfb.log" 2>&1 &
  DISP=$!
  for _ in $(seq 1 100); do
    xdpyinfo >/dev/null 2>&1 && break
    kill -0 $DISP 2>/dev/null || { check xvfb FAIL "Xvfb exited: $(head -c 200 "$OUT/xvfb.log")"; exit 1; }
    sleep 0.2
  done
  key() { xdotool key --delay 80 "$@"; }
  typ() { xdotool type --delay 60 "$1"; }
  shot() { import -window root "$1" 2>/dev/null; }
  find_window() { xdotool search --name '^Dockering$' 2>/dev/null | head -n 1; }
  export SMOKE_SHOT="import -window root"
else
  # a copy of sway drops its file capabilities, which a container cannot grant
  [ -x /usr/local/bin/sway-nocap ] || cp "$(command -v sway)" /usr/local/bin/sway-nocap
  export WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 WLR_RENDERER=pixman XDG_SESSION_TYPE=wayland
  printf 'output HEADLESS-1 resolution 1280x800 position 0 0\ndefault_border none\n' >/tmp/sway.conf
  sway-nocap --unsupported-gpu -c /tmp/sway.conf >"$OUT/sway.log" 2>&1 &
  DISP=$!
  for _ in $(seq 1 60); do first "$XDG_RUNTIME_DIR"/wayland-[0-9] >/dev/null && break; sleep 0.2; done
  WAYLAND_DISPLAY=$(basename "$(first "$XDG_RUNTIME_DIR"/wayland-[0-9])")
  SWAYSOCK=$(first "$XDG_RUNTIME_DIR"/sway-ipc.*.sock)
  export WAYLAND_DISPLAY SWAYSOCK
  unset DISPLAY
  key() { # xdotool-style chords (ctrl+shift+p) through wtype; one process per chord
    for chord in "$@"; do
      IFS='+' read -ra p <<<"$chord"
      last="${p[${#p[@]} - 1]}"
      a=()
      b=()
      for m in "${p[@]:0:${#p[@]}-1}"; do a+=(-M "$m"); b+=(-m "$m"); done
      [ "$last" = comma ] && last=","
      case "$last" in
      Escape | Return | F6 | Tab) wtype -s "$LEAD_MS" "${a[@]}" -k "$last" "${b[@]}" ;;
      *) wtype -s "$LEAD_MS" "${a[@]}" "$last" "${b[@]}" ;;
      esac
    done
  }
  typ() { wtype -s "$LEAD_MS" "$1"; }
  shot() { grim "$1" 2>/dev/null; }
  find_window() { swaymsg -t get_tree 2>/dev/null | grep -c '"app_id": *"dev.dockering.Dockering"' | grep -v '^0$'; }
  export SMOKE_SHOT=grim
fi
export SMOKE_OUT="$OUT"
nav_key() { # self-test hooks: SMOKE_NOKEYS=1 discards every chord, SMOKE_NOKEYS=first the first press of each (the retry must save it)
  case "${SMOKE_NOKEYS:-}" in
  1) return 0 ;;
  first) [ "${attempt:-1}" -gt 1 ] || return 0 ;;
  esac
  key "$@"
}
px_diff() { compare -metric AE "$1" "$2" null: 2>&1 | awk '$1 ~ /^[0-9]+(\.[0-9]+)?(e\+?[0-9]+)?$/ { printf "%d\n", $1; exit }'; }
colours() { identify -format '%k' "$1" 2>/dev/null; }
wait_stable() { # two frames 1 s apart that differ by fewer than 500 px (the demo's spinner moves ~70)
  local limit="${1:-30}" end ae
  end=$(($(date +%s) + limit))
  shot /tmp/_a.png
  while [ "$(date +%s)" -lt "$end" ]; do
    sleep 1
    shot /tmp/_b.png
    ae=$(px_diff /tmp/_a.png /tmp/_b.png)
    [ -n "$ae" ] && [ "$ae" -lt 500 ] && { echo stable; return 0; }
    cp /tmp/_b.png /tmp/_a.png
  done
  echo unstable
  return 1
}
stop_app() { # SIGTERM, then SIGKILL: an app stuck without a window must not hang the job
  kill "$APP" 2>/dev/null
  for _ in $(seq 1 20); do kill -0 "$APP" 2>/dev/null || break; sleep 0.25; done
  kill -9 "$APP" 2>/dev/null
  wait "$APP" 2>/dev/null
}
collect_logs() {
  local f
  LOG=$(find "$HOME" /tmp -maxdepth 6 -name 'dockering*.log*' 2>/dev/null | head -n 1)
  [ -n "$LOG" ] && cp "$LOG" "$OUT/dockering.log"
  for f in "$HOME"/.local/share/dockering/crash-*.txt /tmp/dockering-demo-*/data/crash-*.txt; do
    [ -f "$f" ] && { cp "$f" "$OUT/"; crash="$f"; }
  done
}
crash=""

# ---- launch ---------------------------------------------------------------------------------------
step "launch ${BIN[*]} ${ARGS[*]} on $DISPLAY_KIND"
RUST_LOG=info "${BIN[@]}" "${ARGS[@]}" >"$OUT/app.stdout" 2>"$OUT/app.stderr" &
APP=$!
tw=$(date +%s.%N)
wid=""
for _ in $(seq 1 160); do # 40 s
  wid=$(find_window)
  [ -n "$wid" ] && break
  kill -0 $APP 2>/dev/null || break
  sleep 0.25
done
if [ -z "$wid" ]; then
  if kill -0 $APP 2>/dev/null; then
    check window FAIL "no window after 40 s and the process is still running (no Vulkan driver? see dockering.log)"
    stop_app
  else
    wait $APP
    check window FAIL "no window: the app exited with status $?"
  fi
  tail -n 15 "$OUT/app.stderr"
  collect_logs
  grep -E ' (ERROR|WARN) ' "$OUT/dockering.log" 2>/dev/null | grep -v -E 'xinput mouse|cursor icon|system locale' | head -n 3 | cut -c1-200
  kill $DISP 2>/dev/null
  echo "== $OUT: $(grep -c ' PASS ' "$RESULTS") passed, $(grep -c ' FAIL ' "$RESULTS") failed"
  exit 1
fi
check window PASS "appeared after $(echo "$(date +%s.%N) $tw" | awk '{printf "%.1f", $1 - $2}')s"
if [ "$DISPLAY_KIND" = x11 ]; then
  xdotool mousemove 640 400 # no window manager: keys go to the window under the pointer
  for _ in $(seq 1 50); do # WM_CLASS is set about 70 ms after the window gets its name
    xprop -id "$wid" WM_CLASS 2>/dev/null | grep -q 'dev.dockering.Dockering' && break
    sleep 0.1
  done
  if xprop -id "$wid" WM_CLASS 2>/dev/null | grep -q 'dev.dockering.Dockering'; then
    check window_identity PASS "WM_CLASS dev.dockering.Dockering"
  else
    check window_identity FAIL "$(xprop -id "$wid" WM_CLASS 2>&1)"
  fi
else
  check window_identity PASS "app_id dev.dockering.Dockering"
fi
# a mapped window is not yet a painted one
painted=""
tp=$(date +%s.%N)
for _ in $(seq 1 60); do # 30 s
  shot "$OUT/01-first-frame.png"
  c=$(colours "$OUT/01-first-frame.png")
  [ "${c:-0}" -gt 200 ] && { painted=1; break; }
  kill -0 $APP 2>/dev/null || break
  sleep 0.5
done
if [ -n "$painted" ]; then
  check first_paint PASS "$c colours after $(echo "$(date +%s.%N) $tp" | awk '{printf "%.1f", $1 - $2}')s"
else
  check first_paint FAIL "no content after 30 s (${c:-0} colours)"
fi
res=$(wait_stable 30)
if [ "$res" = stable ]; then
  check first_frame_stable PASS "after $(now)s"
else
  check first_frame_stable FAIL "$res after $(now)s"
fi
shot "$OUT/01-first-frame.png"
if kill -0 $APP 2>/dev/null; then check alive PASS; else check alive FAIL "exited after the first frame"; fi

# ---- UI walk (demo profile): keys in, accessibility tree out ---------------------------------------
if [ "$SCENARIO" = demo ]; then
  atspi_run tree
  record "$ATSPI_OUT"
  atspi_run page Containers
  record "$ATSPI_OUT"
  # each chord must land on its page (checked in the accessibility tree); one retry for a dropped key
  prev="$OUT/01-first-frame.png"
  n=1
  for entry in "ctrl+2 Images" "ctrl+3 Volumes" "ctrl+4 Networks" "ctrl+comma Settings" "ctrl+1 Containers"; do
    chord=${entry% *}
    page=${entry#* }
    n=$((n + 1))
    f="$OUT/0${n}-${page,,}.png"
    if [ "$page" = Settings ]; then args=(settings); else args=(page "$page"); fi
    for attempt in 1 2; do
      nav_key "$chord"
      ATSPI_TIMEOUT=6 atspi_run "${args[@]}" && break
    done
    ATSPI_OUT=$(sed "1s/^[A-Za-z_]*/keys_$chord/" <<<"$ATSPI_OUT")
    [ "$attempt" -gt 1 ] && grep -q ' PASS ' <<<"$ATSPI_OUT" && ATSPI_OUT="$ATSPI_OUT (second try)"
    record "$ATSPI_OUT"
    wait_stable 10 >/dev/null
    shot "$f"
    check "px_$chord" INFO "$(px_diff "$prev" "$f") px changed"
    prev="$f"
  done
  # every sidebar item activated through the accessibility API (no keys, no coordinates)
  atspi_run walk
  record "$ATSPI_OUT"
fi

# ---- quit ---------------------------------------------------------------------------------------
step "quit"
if [ "$DISPLAY_KIND" = x11 ]; then
  key ctrl+shift+p
  sleep 1.2
  typ quit # the command palette's "Quit Dockering"
  sleep 1.2
  shot "$OUT/90-quit-palette.png"
  key Return
else
  # what a titlebar click sends (xdg_toplevel close)
  shot "$OUT/90-before-close.png"
  swaymsg '[app_id="dev.dockering.Dockering"] kill' >/dev/null 2>&1
fi
for _ in $(seq 1 40); do kill -0 $APP 2>/dev/null || break; sleep 0.25; done
if kill -0 $APP 2>/dev/null; then
  check clean_quit FAIL "still running 10 s after the close request"
  stop_app
else
  wait $APP
  rc=$?
  if [ $rc -eq 0 ]; then check clean_quit PASS "exit 0"; else check clean_quit FAIL "exit $rc"; fi
fi

# ---- state, crash and log assertions ----------------------------------------------------------------
STATE=$(first "$HOME"/.local/share/dockering/state.json /tmp/dockering-demo-*/data/state.json)
saved=$(grep -o '"last_run_version": *"[^"]*"' "${STATE:-/nonexistent}" 2>/dev/null | sed 's/.*: *"//; s/"//')
if [ "$saved" = "$EXPECTED_VERSION" ]; then
  check state_saved PASS "last_run_version $saved"
else
  check state_saved FAIL "last_run_version '${saved:-none}', expected $EXPECTED_VERSION"
fi
collect_logs
if [ -z "$crash" ] && ! grep -q 'panicked' "$OUT/app.stderr"; then check no_crash PASS; else check no_crash FAIL "${crash:-panic on stderr}"; fi
# known headless noise: no pointer on Xvfb, no cursor theme, no locale, no desktop portal
bad=$(grep -E ' (ERROR|WARN) ' "$OUT/dockering.log" 2>/dev/null |
  grep -v -E 'Found no xinput mouse pointers|error loading cursor icon|failed to get system locale|task_name="org\.freedesktop\.portal\.Settings proxy caching"' | head -n 5)
if [ -z "$bad" ]; then check log_clean PASS; else
  check log_clean FAIL "$(echo "$bad" | head -n 1 | cut -c1-160)"
  echo "$bad" | cut -c1-200
fi
check platform INFO "$PRETTY_NAME; glibc $(ldd --version | head -n 1 | awk '{print $NF}'); $(grep -o -E 'Selected GPU adapter: .*' "$OUT/dockering.log" 2>/dev/null | tail -n 1 | cut -c1-90)"
kill $DISP 2>/dev/null
step "done"
echo "== $OUT: $(grep -c ' PASS ' "$RESULTS") passed, $(grep -c ' FAIL ' "$RESULTS") failed"
[ "$(grep -c ' FAIL ' "$RESULTS")" -eq 0 ]
