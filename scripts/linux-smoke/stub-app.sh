#!/usr/bin/env bash
# Test double for the Dockering binary. selftest-entry.sh runs it as the AppImage of a scenario to inject one known defect
# (plan task 3). It is not part of the product, and the smoke test itself never runs it.
#   SMOKE_STUB=<mode> Dockering-x86_64.AppImage [args]
# The window modes paint on the display that run.sh started. Such a window quits with status 0 and saves
# last_run_version as soon as run.sh has typed the quit command (the screenshot $SMOKE_OUT/90-quit-palette.png exists),
# so each mode passes every check except the one it is meant to break. The wrapper modes run the real application from
# the installed .deb and then break one thing.
#   ok          a healthy window and a healthy quit
#   hang        no window and no exit: the invisible process of the plan's finding F2        -> window
#   exit        panics before any window, leaves a crash file, exits 101 (finding F3)         -> window
#   wrongclass  ok, but WM_CLASS is not the application id                                   -> window_identity
#   blank       ok, but the window shows a single colour                                     -> first_paint
#   flicker     ok, but the rest of the screen changes colour every 50 ms                     -> first_frame_stable
#   strobe      the same with a 100x100 window, so that over 1,000,000 px change at once      -> first_frame_stable
#   mortal      ok, but it exits as soon as run.sh has judged the first frame                -> alive
#   crashfile   the real application, then a crash file in its data directory                -> no_crash
#   panic       the real application, then a panic message on stderr                         -> no_crash
#   logerror    the real application, then an ERROR line in its log                          -> log_clean
#   badexit     the real application, but the process exits with status 3                   -> clean_quit
set -u
APP_ID=dev.dockering.Dockering
REAL=/usr/bin/dockering
MODE="${SMOKE_STUB:?set SMOKE_STUB}"
DATA="$HOME/.local/share/dockering"
LOG="${SMOKE_OUT:-/tmp}/stub-display.log"

if [ "${1:-}" = --version ]; then
  case "$MODE" in
  crashfile | panic | logerror | badexit) exec "$REAL" --version ;;
  *)
    echo "dockering ${EXPECTED_VERSION:-0.0.0}"
    exit 0
    ;;
  esac
fi

# ImageMagick 6 has the tools as commands, ImageMagick 7 as subcommands of magick
if command -v magick >/dev/null 2>&1; then
  IM=(magick)
  SHOW=(magick display)
else
  IM=(convert)
  SHOW=(display)
fi
tmp=$(mktemp -d)
child=""
flick=""
save=""
cleanup() {
  [ -z "$flick" ] || kill "$flick" 2>/dev/null
  [ -z "$child" ] || kill "$child" 2>/dev/null
  if [ -n "$save" ]; then
    mkdir -p "$DATA"
    printf '{"updates":{"last_run_version":"%s"}}\n' "${EXPECTED_VERSION:-0}" >"$DATA/state.json"
  fi
  rm -rf "$tmp"
}
trap cleanup EXIT
trap 'exit 143' TERM

# paint <right|wrong> <command...>: run a program that opens a window named Dockering; with "right" the window also carries
# the application id as its WM_CLASS. A program that cannot open the display is started again: the X server drops a client
# that connects while it resets after run.sh's window polling disconnected (see the report of the self-test).
paint() {
  local class=$1 wid="" attempt tick
  shift
  for attempt in 1 2 3 4 5; do
    "$@" >>"$LOG" 2>&1 &
    child=$!
    for tick in $(seq 1 30); do
      wid=$(xdotool search --name '^Dockering$' 2>/dev/null | head -n 1)
      [ -n "$wid" ] && break 2
      kill -0 "$child" 2>/dev/null || break
      sleep 0.1
    done
    kill "$child" 2>/dev/null
    wait "$child" 2>/dev/null
  done
  echo "stub-app.sh: window poll attempts used: $attempt, last tick $tick" >>"$LOG"
  if [ "$class" = right ] && [ -n "$wid" ]; then
    xdotool set_window --classname "$APP_ID" --class "$APP_ID" "$wid"
  fi
}
# wait up to five minutes for <file> to exist, then <settle> seconds more
await() {
  local _
  for _ in $(seq 1 3000); do
    if [ -e "$1" ]; then
      sleep "$2"
      return 0
    fi
    sleep 0.1
  done
}
# the real application with the arguments of the launch; its status is kept in rc
real() {
  "$REAL" "$@"
  rc=$?
}

case "$MODE" in
ok | wrongclass | blank | mortal)
  save=1
  if [ "$MODE" = blank ]; then
    "${IM[@]}" -size 1280x800 xc:white "$tmp/picture.png"
  else
    "${IM[@]}" -size 1280x800 plasma:fractal "$tmp/picture.png"
  fi
  if [ "$MODE" = wrongclass ]; then class=wrong; else class=right; fi
  paint "$class" "${SHOW[@]}" -title Dockering -geometry +0+0 "$tmp/picture.png"
  if [ "$MODE" = mortal ]; then
    # run.sh's second comparison frame (wait_stable) is the moment the first frame has been judged
    rm -f /tmp/_b.png
    await /tmp/_b.png 0
  else
    await "${SMOKE_OUT:-/nonexistent}/90-quit-palette.png" 0.5
  fi
  exit 0
  ;;
flicker | strobe)
  save=1
  # a small window, while a loop repaints the whole screen behind it with a new colour, so that two frames 1 s apart differ
  # by the screen minus the window: 768,000 px for flicker, 1,014,000 px for strobe, the size at which
  # `compare -metric AE` prints exponent notation (1.014e+06) instead of a count
  if [ "$MODE" = flicker ]; then size=640x400; else size=100x100; fi
  "${IM[@]}" -size "$size" plasma:fractal "$tmp/picture.png"
  paint right "${SHOW[@]}" -title Dockering -geometry +0+0 "$tmp/picture.png"
  (
    n=1
    while kill -0 "$child" 2>/dev/null; do
      n=$(((n * 7919 + 13) % 16777216))
      "${SHOW[@]}" -window root -size 1280x800 "xc:#$(printf '%06x' "$n")" >/dev/null 2>&1
      sleep 0.05
    done
  ) &
  flick=$!
  await "${SMOKE_OUT:-/nonexistent}/90-quit-palette.png" 0.5
  exit 0
  ;;
hang)
  # a recognisable command line, so that selftest-entry.sh can tell whether run.sh left it running
  exec -a dockering-selftest-hang sleep 600
  ;;
exit)
  mkdir -p "$DATA"
  printf 'Dockering %s crashed\nmessage: component window state is missing; call gpui_component::init before gpui_kit::open_window\n' \
    "${EXPECTED_VERSION:-0}" >"$DATA/crash-0.txt"
  echo "thread 'main' panicked at crates/dockering/src/main.rs:1:1:" >&2
  echo "component window state is missing; call gpui_component::init before gpui_kit::open_window" >&2
  exit 101
  ;;
crashfile)
  real "$@"
  mkdir -p "$DATA"
  printf 'Dockering %s crashed\nmessage: injected by the self-test\n' "${EXPECTED_VERSION:-0}" >"$DATA/crash-1.txt"
  exit "$rc"
  ;;
panic)
  real "$@"
  echo "thread 'main' panicked at crates/dockering/src/main.rs:1:1:" >&2
  exit "$rc"
  ;;
logerror)
  real "$@"
  log=$(find "$DATA/logs" -name 'dockering.*.log' 2>/dev/null | head -n 1)
  [ -z "$log" ] || echo "2026-10-09T00:00:00.000000Z ERROR dockering::stub: injected by the self-test" >>"$log"
  exit "$rc"
  ;;
badexit)
  real "$@"
  exit 3
  ;;
*)
  echo "stub-app.sh: unknown mode '$MODE'" >&2
  exit 2
  ;;
esac
